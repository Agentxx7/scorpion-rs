//! Passive, provider-neutral browser challenge detector.
//!
//! Owns exactly one seam: **real Chrome page → evidence-based detection →
//! canonical [`BrowserChallengeSnapshot`]** (top-level case) or a typed,
//! frame-identified detection result (child-frame case). It knows nothing
//! about CAPTCHA providers, solvers or model runtimes — see
//! [`crate::features::captcha`] / [`crate::features::captcha_browser`] for
//! those, which this module never imports. It performs no browser mutation:
//! every CDP call used here (`DOM.getDocument`, `DOM.querySelector`,
//! `DOM.getBoxModel`, `Page.captureScreenshot`) is a read/observation, never
//! an input or navigation command.
//!
//! # Evidence convention
//!
//! Detection targets exactly the [`crate::features::captcha::CaptchaChallengeKind::PointSelection`]
//! shape, the simplest kind the canonical model already represents, using
//! one disclosed, ARIA-grounded structural convention rather than page-text
//! matching:
//!
//! * a **challenge candidate**: an element carrying `role="application"`
//!   (the WAI-ARIA role for a widget that manages its own interaction model
//!   rather than being read as static content), a non-empty `aria-label`
//!   (a real accessible name, not incidental), and a non-empty `id` (needed
//!   to re-resolve the exact live element through a public
//!   `chromiumoxide::Page` selector API — see "Why selectors" below);
//! * one or more **target candidates** nested anywhere under it: elements
//!   carrying `role="button"`, a `tabindex` attribute, and a non-empty `id`.
//!
//! A page that merely contains the words "captcha"/"verify"/"human" in text
//! content never matches this convention (proven by the `text_only_false_positive`
//! fixture in the production-binding test) — only genuine ARIA-widget
//! structure does. This is one disclosed heuristic scoped to one supported
//! challenge kind, not a claim of covering every real CAPTCHA vendor's
//! markup; a later frontier may add more.
//!
//! # Why selectors, not raw node identity
//!
//! `chromiumoxide::element::Element` can only be constructed by this crate
//! through `Page::find_element`/`find_elements`/`find_element_pierced` (its
//! constructor is private to `chromey`). So evidence-matching walks the
//! already-fetched `DOM.getDocument(pierce: true)` tree in memory (a single
//! CDP round trip) to decide *whether* a supported challenge exists and
//! *which* `id`s identify it, then re-resolves those `id`s into real,
//! live `Element`s through the public pierced-selector API before handing
//! them to [`BrowserChallengeSnapshot::capture`].
//!
//! # Frame scope
//!
//! `DOM.getDocument(pierce: true)` and `find_element_pierced` both operate
//! through the top-level page's own CDP session, so both — consistently —
//! reach the top-level document, shadow roots, and genuinely same-process
//! ("`SameSessionChild`", see [`crate::features::frame_context`]) child
//! frames, but *not* a genuine out-of-process (OOPIF) child: `pierce: true`
//! only recurses through a node's `content_document`, which Chromium simply
//! never populates for an OOPIF `<iframe>` element (its content lives on a
//! separate CDP target this walk never attaches to) — so an OOPIF
//! challenge is not merely "found but unmaterializable", it is invisible to
//! this walk from the start; `scan_node` never even has a subtree to
//! recurse into.
//!
//! When `detect_browser_challenge` is given a live `browser` handle
//! (`SCORPION_CANONICAL_CAPTCHA_FRAME_ACTION_BINDING_001` /
//! `SCORPION_CANONICAL_CAPTCHA_OOPIF_SESSION_CONTEXT_BINDING_001`), a
//! same-session child match is fully materialized via
//! [`BrowserChallengeSnapshot::capture_in_frame`] — resolving the top-level
//! `TargetInfo` fresh via `Target.getTargetInfo(page.target_id())` (proven
//! to work through the page's own attached session), then
//! [`crate::features::frame_context::FrameContext::resolve_same_session_child`].
//! If the top-level walk finds *no* evidence at all, this module *also*
//! probes for a genuine OOPIF challenge: a fresh `Target.getTargets` call
//! (never a persistent event subscription — see [`probe_oopif_challenges`]'s
//! own doc comment for why that is enough) lists every currently attached
//! `"iframe"`-typed target, and for each one,
//! [`crate::features::frame_context::FrameContext::resolve_child`] — the
//! exact, already real-Turnstile-proven OOPIF constructor — attaches to its
//! own session and re-runs this module's own evidence walk *through that
//! session*. Only a candidate whose own document genuinely contains
//! matching evidence is ever materialized (never "first child", never a
//! URL/text guess) — see [`probe_oopif_challenges`]. Passing `browser:
//! None` (every existing caller/test predating this capability) preserves
//! this module's exact prior behavior: a same-session `FramedEvidence`
//! reports identity only (`materialization: FramedMaterialization::Unavailable`),
//! and a genuine OOPIF challenge is not even attempted, exactly as before.

#![cfg(feature = "chrome")]

use chromiumoxide::cdp::browser_protocol::dom::{BackendNodeId, GetDocumentParams, Node};
use chromiumoxide::cdp::browser_protocol::page::FrameId as CdpFrameId;
use chromiumoxide::cdp::browser_protocol::target::{GetTargetInfoParams, GetTargetsParams};
use chromiumoxide::{Browser, Page};

use crate::features::browser_challenge::{BrowserChallengeFailure, BrowserChallengeSnapshot};
use crate::features::frame_context::{FrameContext, FrameContextFailure};

/// Bound on [`probe_oopif_challenges`]'s poll for a genuine OOPIF child to
/// finish attaching after the top-level page's own navigation completes.
const OOPIF_ATTACH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
/// Poll interval for the same wait.
const OOPIF_ATTACH_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);

/// Typed detection failure. Distinct from "no challenge found" (`Ok(None)`):
/// this means the observation itself could not be trusted, not that the
/// page was inspected and found clean.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChallengeDetectionFailure {
    /// The browser could not be observed (e.g. `DOM.getDocument`,
    /// `Page.mainframe`, or re-resolving a matched element by selector
    /// failed).
    ObservationFailed,
    /// Evidence for a supported challenge was found, but it could not be
    /// turned into a canonical, immutable snapshot.
    MaterializationFailed(BrowserChallengeFailure),
}

/// Result of a passive detection pass that found supported challenge
/// evidence. Deliberately not `Debug` — it retains a
/// [`BrowserChallengeSnapshot`] in the top-level case, which is likewise not
/// `Debug` (its `visual_bytes` field is not meant to end up in log output).
pub enum DetectedBrowserChallenge {
    /// Evidence found in the page's top-level document. Fully materialized:
    /// screenshot captured, targets bound, ready for a later provider-router
    /// frontier to consume.
    TopLevel {
        /// The canonical, immutable snapshot.
        snapshot: BrowserChallengeSnapshot,
        /// `id` attribute of the matched challenge-candidate element. Kept
        /// alongside `snapshot` because [`BrowserChallengeSnapshot`] itself
        /// only retains stable ids for *targets*, not for the challenge
        /// element.
        challenge_element_id: String,
        /// The matched challenge candidate's own `aria-label` — the page
        /// author's real accessible name, used as the canonical challenge
        /// instruction. Never caller-supplied prompt text.
        instruction: String,
    },
    /// Evidence found inside a same-session child frame. Frame identity is
    /// always real and correct; `materialization` truthfully reports
    /// whether it could also be turned into a canonical, immutable,
    /// frame-scoped snapshot — see the module-level "Frame scope" section
    /// and [`FramedMaterialization`].
    FramedEvidence {
        /// Exact `FrameId` (as reported by Chromium) owning the matched
        /// challenge evidence.
        frame_id: String,
        /// Exact `FrameId` of the parent document, when known.
        parent_frame_id: Option<String>,
        /// `id` attribute of the matched challenge-candidate element.
        challenge_element_id: String,
        /// The matched challenge candidate's own `aria-label` — the page
        /// author's real accessible name, used as the canonical challenge
        /// instruction. Never caller-supplied prompt text.
        instruction: String,
        /// Whether this evidence was also turned into a canonical,
        /// frame-aware, action-ready snapshot.
        materialization: FramedMaterialization,
    },
}

/// Whether one detected [`DetectedBrowserChallenge::FramedEvidence`] was
/// also turned into a canonical, frame-aware, action-ready snapshot.
/// Deliberately not `Debug`/`Clone` in the `Ready` case — it retains a
/// live [`FrameContext`] pair and a [`BrowserChallengeSnapshot`], the same
/// live-handle shape [`DetectedBrowserChallenge::TopLevel`] already
/// retains.
pub enum FramedMaterialization {
    /// No live `browser` handle was supplied to `detect_browser_challenge`
    /// — this evidence's identity is real, but frame-context resolution
    /// was never attempted. Preserves this module's exact prior behavior
    /// for every caller that still passes `browser: None`.
    Unavailable,
    /// A live `browser` handle was supplied, but frame-context resolution
    /// or snapshot capture failed — real, typed evidence, never silently
    /// downgraded to `Unavailable` and never a top-level fallback.
    Failed(FrameMaterializationFailure),
    /// Fully materialized: real top-level and child [`FrameContext`]s plus
    /// a real, immutable, frame-scoped [`BrowserChallengeSnapshot`], ready
    /// for the canonical frame-aware action seam
    /// (`crate::features::captcha_browser::execute_browser_captcha_attempt_in_frame`).
    Ready {
        /// The real top-level frame's canonical context.
        top_level: FrameContext,
        /// The real child frame's canonical context — always
        /// [`crate::features::frame_context::FrameClassification::SameSessionChild`]
        /// here; a genuine out-of-process (OOPIF) child cannot be proven
        /// through this seam (see the module-level "Frame scope" section),
        /// so it always surfaces as [`FrameMaterializationFailure::FrameContext`]
        /// instead of a `Ready` result claiming a classification this seam
        /// never actually observed.
        frame: FrameContext,
        /// The canonical, immutable, frame-scoped snapshot.
        snapshot: BrowserChallengeSnapshot,
    },
}

/// Why a supplied `browser` handle could not turn one
/// [`DetectedBrowserChallenge::FramedEvidence`] into a
/// [`FramedMaterialization::Ready`] snapshot. Never a guess: each variant
/// names the exact CDP/canonical-seam step that failed.
#[derive(Debug)]
pub enum FrameMaterializationFailure {
    /// `Target.getTargetInfo` for the top-level page's own target failed.
    TopLevelTargetInfoUnavailable,
    /// [`FrameContext`] resolution (top-level or same-session child)
    /// failed — includes the case where the evidence's frame turns out not
    /// to be a genuine same-session child at all (e.g. a real out-of-process
    /// OOPIF, which `resolve_same_session_child` cannot prove ownership of
    /// through the parent's own session).
    FrameContext(FrameContextFailure),
    /// Frame-context resolution succeeded, but
    /// [`BrowserChallengeSnapshot::capture_in_frame`] itself failed.
    Snapshot(BrowserChallengeFailure),
}

/// One matched challenge candidate, gathered from an in-memory walk of a
/// single `DOM.getDocument(pierce: true)` snapshot.
struct Evidence {
    frame_id: String,
    parent_frame_id: Option<String>,
    challenge_id: String,
    /// Real CDP backend-node identity of the matched challenge candidate —
    /// carried alongside the string `id` so a same-session-child frame's
    /// evidence can be materialized (`BrowserChallengeSnapshot::capture_in_frame`
    /// takes backend-node identity, not a re-resolved `Element` handle)
    /// without a second DOM query.
    challenge_backend_node_id: BackendNodeId,
    /// The matched challenge candidate's own `aria-label` value — a real
    /// accessible name the page's author wrote, not a caller-supplied
    /// prompt. Carried through so a later provider-routing frontier can use
    /// it as the canonical challenge instruction without re-querying.
    instruction: String,
    /// Stable `id` paired with the real CDP backend-node identity of every
    /// nested target candidate — the latter unused by the top-level path
    /// (which re-resolves a live `Element` by selector instead) but needed
    /// verbatim by frame-scoped materialization.
    target_ids: Vec<(String, BackendNodeId)>,
}

/// Passively inspect a real Chrome page for a supported, evidence-based
/// challenge. Never mutates the page. `Ok(None)` means the page was
/// genuinely inspected and no supported challenge evidence was found — it
/// does not mean the detector failed.
///
/// `browser` gates same-session-child frame materialization
/// (`SCORPION_CANONICAL_CAPTCHA_FRAME_ACTION_BINDING_001`): `None`
/// preserves this function's exact prior contract (every existing
/// caller/test) — a detected `FramedEvidence`'s `materialization` is always
/// `FramedMaterialization::Unavailable`. `Some(browser)` additionally
/// attempts full frame-aware materialization — see the module-level "Frame
/// scope" section for exactly what that can and cannot prove.
pub async fn detect_browser_challenge(
    page: &Page,
    browser: Option<&Browser>,
) -> Result<Option<DetectedBrowserChallenge>, ChallengeDetectionFailure> {
    let top_frame_id = page
        .mainframe()
        .await
        .map_err(|_| ChallengeDetectionFailure::ObservationFailed)?
        .ok_or(ChallengeDetectionFailure::ObservationFailed)?
        .inner()
        .clone();

    let root = page
        .get_document()
        .await
        .map_err(|_| ChallengeDetectionFailure::ObservationFailed)?;

    let mut evidence = Vec::new();
    scan_node(&root, &top_frame_id, None, &mut evidence);

    let Some(matched) = evidence.into_iter().next() else {
        // No evidence anywhere the top-level pierced walk can see. When a
        // live browser handle is available, *and* the already-fetched tree
        // shows at least one `<iframe>` element whose content is genuinely
        // absent here (`content_document: None` — the structural OOPIF
        // signature this same tree already carries, no extra CDP call
        // needed to check it), also probe genuine OOPIF children. An
        // ordinary page with no such candidate never pays any of this
        // cost — see this module's own "Frame scope" docs and
        // `probe_oopif_challenges`'s doc comment for exactly what probing
        // does and does not claim.
        if let Some(browser) = browser {
            if has_potential_oopif_candidate(&root) {
                if let Some(detected) = probe_oopif_challenges(page, browser, &top_frame_id).await {
                    return Ok(Some(detected));
                }
            }
        }
        return Ok(None);
    };

    if matched.frame_id != top_frame_id {
        let materialization = match browser {
            Some(browser) => {
                materialize_framed_challenge(page, browser, &top_frame_id, &matched).await
            }
            None => FramedMaterialization::Unavailable,
        };
        return Ok(Some(DetectedBrowserChallenge::FramedEvidence {
            frame_id: matched.frame_id,
            parent_frame_id: matched.parent_frame_id,
            challenge_element_id: matched.challenge_id,
            instruction: matched.instruction,
            materialization,
        }));
    }

    let challenge_element_id = matched.challenge_id;
    let instruction = matched.instruction;
    let challenge_element = page
        .find_element_pierced(id_selector(&challenge_element_id))
        .await
        .map_err(|_| ChallengeDetectionFailure::ObservationFailed)?;

    let mut targets = Vec::with_capacity(matched.target_ids.len());
    for (target_id, _backend_node_id) in matched.target_ids {
        let element = page
            .find_element_pierced(id_selector(&target_id))
            .await
            .map_err(|_| ChallengeDetectionFailure::ObservationFailed)?;
        targets.push((target_id, element));
    }

    let snapshot = BrowserChallengeSnapshot::capture(page, challenge_element, targets)
        .await
        .map_err(ChallengeDetectionFailure::MaterializationFailed)?;

    Ok(Some(DetectedBrowserChallenge::TopLevel {
        snapshot,
        challenge_element_id,
        instruction,
    }))
}

/// Attempt to turn one already-detected `FramedEvidence` match into a
/// [`FramedMaterialization::Ready`] snapshot, using a live `browser` handle.
/// Never mutates the page — every CDP call here (`Target.getTargetInfo`,
/// frame-context resolution, `Page.captureScreenshot`) is a read, exactly
/// like the top-level materialization path above. Failure is always typed
/// and returned as `FramedMaterialization::Failed`, never silently
/// downgraded to `Unavailable` (that variant means "no browser handle was
/// even offered", a categorically different fact).
async fn materialize_framed_challenge(
    page: &Page,
    browser: &Browser,
    top_frame_id: &str,
    matched: &Evidence,
) -> FramedMaterialization {
    let top_level_target_info = match page
        .execute(
            GetTargetInfoParams::builder()
                .target_id(page.target_id().clone())
                .build(),
        )
        .await
    {
        Ok(response) => response.result.target_info,
        Err(_) => {
            return FramedMaterialization::Failed(
                FrameMaterializationFailure::TopLevelTargetInfoUnavailable,
            )
        }
    };
    let _ = top_frame_id; // Already proven equal to top_level_target_info.target_id by construction.
    let top_level = match FrameContext::resolve_top_level(browser, &top_level_target_info).await {
        Ok(context) => context,
        Err(failure) => {
            return FramedMaterialization::Failed(FrameMaterializationFailure::FrameContext(
                failure,
            ))
        }
    };
    let frame = match FrameContext::resolve_same_session_child(
        browser,
        &top_level,
        CdpFrameId(matched.frame_id.clone()),
    )
    .await
    {
        Ok(context) => context,
        Err(failure) => {
            return FramedMaterialization::Failed(FrameMaterializationFailure::FrameContext(
                failure,
            ))
        }
    };
    let targets = matched
        .target_ids
        .iter()
        .map(|(id, backend_node_id)| (id.clone(), *backend_node_id))
        .collect();
    match BrowserChallengeSnapshot::capture_in_frame(
        page,
        &top_level,
        &frame,
        matched.challenge_backend_node_id,
        targets,
    )
    .await
    {
        Ok(snapshot) => FramedMaterialization::Ready {
            top_level,
            frame,
            snapshot,
        },
        Err(failure) => {
            FramedMaterialization::Failed(FrameMaterializationFailure::Snapshot(failure))
        }
    }
}

/// Probe genuine out-of-process (OOPIF) children for supported challenge
/// evidence — only ever reached when the top-level pierced DOM walk found
/// no evidence anywhere, since a genuine OOPIF child's content is
/// structurally invisible to that walk (see this module's own "Frame
/// scope" docs).
///
/// A fresh `Target.getTargets` call — not a persistent event subscription
/// — is enough: `SCORPION_CANONICAL_CAPTCHA_FRAME_ACTION_BINDING_001`
/// already proved a page's own `TargetInfo` can be queried fresh, on
/// demand, through nothing more than the page's own attached session;
/// `Target.getTargets` is the same kind of on-demand, `&Browser`-scoped
/// read, listing every target Chromium's own (already-enabled, needed for
/// its internal auto-attach bookkeeping) target discovery already knows
/// about — no new subscription to own, bound, or clean up. For each
/// `"iframe"`-typed candidate, [`FrameContext::resolve_child`] — the exact
/// primitive an earlier frontier's real Turnstile acceptance already
/// proved correct for genuine OOPIF — attaches to its own session (or
/// fails typed, e.g. for an ordinary same-session iframe that also shows
/// up in this listing but can never resolve as a child target) and this
/// module's own `scan_node` evidence walk runs again, this time through
/// that child's own session. Only a candidate whose own document genuinely
/// contains matching evidence is ever materialized — never "first child",
/// never a URL/text guess: an iframe target with no matching evidence, or
/// that fails to resolve at all, is simply skipped. Never mutates the
/// page: every CDP call here (`Target.getTargetInfo`, `Target.getTargets`,
/// frame-context resolution, `DOM.getDocument`, `Page.captureScreenshot`)
/// is a read.
async fn probe_oopif_challenges(
    page: &Page,
    browser: &Browser,
    top_frame_id: &str,
) -> Option<DetectedBrowserChallenge> {
    let top_level_target_info = page
        .execute(
            GetTargetInfoParams::builder()
                .target_id(page.target_id().clone())
                .build(),
        )
        .await
        .ok()?
        .result
        .target_info;
    let top_level = FrameContext::resolve_top_level(browser, &top_level_target_info)
        .await
        .ok()?;

    // Bounded poll: a genuine OOPIF child needs an entire separate renderer
    // process to spin up, navigate and auto-attach, which can genuinely
    // lag behind the top-level page's own "load" — observed live, a fresh
    // `Target.getTargets` immediately after top-level navigation completes
    // can still report zero matching iframe targets for a child that
    // attaches only a few hundred milliseconds later. Re-list and retry
    // the full candidate scan for up to `OOPIF_ATTACH_TIMEOUT`, exactly the
    // same bounded-wait shape `FrameContext`'s own
    // `wait_for_frame_and_owner` already uses for a materially identical
    // "target/frame just attached, Chromium hasn't fully settled yet"
    // race — never unbounded, never retried past this deadline. A page
    // with no genuine OOPIF candidate at all pays this cost too, but only
    // ever reaches this function when the caller's own
    // `has_potential_oopif_candidate` check already found a real
    // `<iframe>` element with no visible content — never an ordinary,
    // iframe-free page.
    // Defensive, idempotent re-assertion that target discovery is enabled
    // — chromiumoxide's own Handler already enables this at browser launch
    // for its internal auto-attach bookkeeping, but re-asserting it here is
    // a cheap, safe no-op when already enabled and costs nothing extra.
    let _ = browser
        .execute(chromiumoxide::cdp::browser_protocol::target::SetDiscoverTargetsParams::new(true))
        .await;

    let deadline = tokio::time::Instant::now() + OOPIF_ATTACH_TIMEOUT;
    loop {
        let candidates = browser
            .execute(GetTargetsParams::builder().build())
            .await
            .ok()?
            .result
            .target_infos;

        for candidate in candidates.iter().filter(|target| target.r#type == "iframe") {
            let Ok(child) = FrameContext::resolve_child(browser, &top_level, candidate).await
            else {
                continue;
            };
            let Ok(response) = child
                .execute(GetDocumentParams {
                    depth: Some(-1),
                    pierce: Some(true),
                })
                .await
            else {
                continue;
            };
            let mut evidence = Vec::new();
            scan_node(
                &response.result.root,
                child.frame_id.0.as_str(),
                Some(top_frame_id),
                &mut evidence,
            );
            let Some(matched) = evidence.into_iter().next() else {
                continue;
            };
            let targets = matched
                .target_ids
                .iter()
                .map(|(id, backend_node_id)| (id.clone(), *backend_node_id))
                .collect();
            let materialization = match BrowserChallengeSnapshot::capture_in_frame(
                page,
                &top_level,
                &child,
                matched.challenge_backend_node_id,
                targets,
            )
            .await
            {
                Ok(snapshot) => FramedMaterialization::Ready {
                    top_level,
                    frame: child,
                    snapshot,
                },
                Err(failure) => {
                    FramedMaterialization::Failed(FrameMaterializationFailure::Snapshot(failure))
                }
            };
            return Some(DetectedBrowserChallenge::FramedEvidence {
                frame_id: matched.frame_id,
                parent_frame_id: matched.parent_frame_id,
                challenge_element_id: matched.challenge_id,
                instruction: matched.instruction,
                materialization,
            });
        }

        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(OOPIF_ATTACH_POLL_INTERVAL).await;
    }
}

impl DetectedBrowserChallenge {
    /// Route this challenge through the canonical provider router if, and
    /// only if, it was fully materialized — top-level via
    /// `crate::features::solvers::route_detected_browser_challenge`, or a
    /// same-session framed challenge whose `materialization` is
    /// [`FramedMaterialization::Ready`] via
    /// `crate::features::solvers::route_detected_framed_browser_challenge`
    /// (`SCORPION_CANONICAL_CAPTCHA_FRAME_ACTION_BINDING_001`). Evidence
    /// that never reached `Ready` (`Unavailable`/`Failed`) is never routed
    /// — no top-level fallback, no guessed action.
    ///
    /// When a solution is produced, also binds it to the real browser
    /// through the router's own action-binding branch
    /// (`SCORPION_CANONICAL_CAPTCHA_SOLUTION_BROWSER_ACTION_BINDING_001`) —
    /// the router itself only ever reaches browser input through the
    /// pre-proven `execute_browser_captcha_attempt`/`_in_frame` seams,
    /// never an ad-hoc dispatcher, and never more than the one explicit
    /// provider attempt those seams already make. After a real action is
    /// applied, this method — and only this method, never the router —
    /// performs one more passive detection pass through the exact same
    /// evidence-based convention used to find the challenge in the first
    /// place, and records whether the same challenge element is still
    /// observed. This is deliberately not a "solved" claim, only the
    /// minimal genuine real-DOM evidence that the dispatched action did
    /// something — [`crate::features::captcha::CaptchaBrowserActionOutcome`]'s
    /// own doc comment explains why no stronger claim is made. `browser` is
    /// threaded through only for this post-action re-detection pass on a
    /// framed challenge (frame-context resolution needs a live handle the
    /// same way materialization did); the top-level path never needs it.
    pub(crate) async fn route(
        &self,
        page: &Page,
        browser: Option<&Browser>,
        selected_provider: Option<crate::features::captcha::CaptchaProviderId>,
        deadline: std::time::Duration,
    ) -> Option<crate::features::captcha::CaptchaRouteOutcomeSummary> {
        use crate::features::captcha::{CaptchaBrowserActionOutcome, CaptchaRouteOutcomeSummary};

        let outcome = match self {
            Self::TopLevel {
                snapshot,
                instruction,
                ..
            } => {
                let challenge = crate::features::captcha::CaptchaChallenge {
                    kind: crate::features::captcha::CaptchaChallengeKind::PointSelection,
                    instruction: instruction.clone(),
                    visuals: vec![crate::features::captcha::CaptchaVisualInput::materialized(
                        None,
                        "image/png",
                        snapshot.visual_bytes.clone(),
                    )],
                };
                crate::features::solvers::route_detected_browser_challenge(
                    page,
                    Some(snapshot),
                    challenge,
                    selected_provider,
                    deadline,
                )
                .await
            }
            Self::FramedEvidence {
                instruction,
                materialization:
                    FramedMaterialization::Ready {
                        top_level,
                        frame,
                        snapshot,
                    },
                ..
            } => {
                let challenge = crate::features::captcha::CaptchaChallenge {
                    kind: crate::features::captcha::CaptchaChallengeKind::PointSelection,
                    instruction: instruction.clone(),
                    visuals: vec![crate::features::captcha::CaptchaVisualInput::materialized(
                        None,
                        "image/png",
                        snapshot.visual_bytes.clone(),
                    )],
                };
                crate::features::solvers::route_detected_framed_browser_challenge(
                    page,
                    top_level,
                    frame,
                    snapshot,
                    challenge,
                    selected_provider,
                    deadline,
                )
                .await
            }
            Self::FramedEvidence { .. } => return None,
        };

        let CaptchaRouteOutcomeSummary::SolutionProduced {
            action:
                CaptchaBrowserActionOutcome::Applied {
                    actions_applied, ..
                },
        } = &outcome
        else {
            return Some(outcome);
        };
        let actions_applied = *actions_applied;

        // Minimal, generic, real-DOM post-action observation: re-run the
        // exact same passive detector once more and check whether it still
        // matches this same challenge element. Never a fixture-specific
        // "solved" marker, never a Rust-side flag flipped without a real
        // browser round trip. For a framed challenge this re-detection
        // needs the same live `browser` handle materialization itself
        // needed — always available here, since only a `Ready` framed
        // match (which required exactly that handle) or a `TopLevel` match
        // can ever reach this point.
        let challenge_observed_after_action = match self {
            Self::TopLevel {
                challenge_element_id,
                ..
            } => matches!(
                detect_browser_challenge(page, browser).await,
                Ok(Some(DetectedBrowserChallenge::TopLevel {
                    challenge_element_id: ref observed_id,
                    ..
                })) if observed_id == challenge_element_id
            ),
            Self::FramedEvidence {
                challenge_element_id,
                frame_id,
                ..
            } => matches!(
                detect_browser_challenge(page, browser).await,
                Ok(Some(DetectedBrowserChallenge::FramedEvidence {
                    challenge_element_id: ref observed_id,
                    frame_id: ref observed_frame_id,
                    ..
                })) if observed_id == challenge_element_id && observed_frame_id == frame_id
            ),
        };

        Some(CaptchaRouteOutcomeSummary::SolutionProduced {
            action: CaptchaBrowserActionOutcome::Applied {
                actions_applied,
                challenge_observed_after_action,
            },
        })
    }

    /// Reduce this live-handle-bearing result to the plain, `Clone + Debug`
    /// evidence [`crate::page::Page`] retains once `Page::new_base` (the
    /// caller) returns. See
    /// [`crate::features::captcha::BrowserChallengeObservation`]'s doc
    /// comment for why the live snapshot itself is not what gets retained.
    /// `route_outcome` is `None` only for the framed-evidence case (never
    /// routed); the top-level case must always have attempted routing —
    /// callers pass `CaptchaRouteOutcomeSummary::NotConfigured` themselves
    /// when no provider was configured, so this never silently drops a
    /// route attempt.
    pub(crate) fn into_observation(
        self,
        route_outcome: Option<crate::features::captcha::CaptchaRouteOutcomeSummary>,
    ) -> crate::features::captcha::BrowserChallengeObservation {
        use crate::features::captcha::{
            BrowserChallengeObservation, CaptchaChallengeKind, CaptchaRouteOutcomeSummary,
        };
        match self {
            Self::TopLevel {
                snapshot,
                challenge_element_id,
                ..
            } => BrowserChallengeObservation::TopLevel {
                kind: CaptchaChallengeKind::PointSelection,
                frame_id: snapshot.frame_id.clone(),
                challenge_element_id,
                target_element_ids: snapshot.targets.keys().cloned().collect(),
                visual_bytes: std::sync::Arc::from(snapshot.visual_bytes.as_slice()),
                captured_pixel_width: snapshot.captured_pixel_width,
                captured_pixel_height: snapshot.captured_pixel_height,
                route_outcome: route_outcome.unwrap_or(CaptchaRouteOutcomeSummary::NotConfigured),
            },
            Self::FramedEvidence {
                frame_id,
                parent_frame_id,
                challenge_element_id,
                materialization,
                ..
            } => BrowserChallengeObservation::Framed {
                frame_id,
                parent_frame_id,
                challenge_element_id,
                materialized: matches!(materialization, FramedMaterialization::Ready { .. }),
                route_outcome: route_outcome.unwrap_or(CaptchaRouteOutcomeSummary::NotConfigured),
            },
        }
    }
}

/// Build a CSS attribute-selector for an exact `id`, escaping `"` and `\`
/// rather than assuming the id is selector-safe as-is.
fn id_selector(id: &str) -> String {
    let escaped = id.replace('\\', "\\\\").replace('"', "\\\"");
    format!("[id=\"{escaped}\"]")
}

/// Look up one flat CDP `attributes: [name, value, name, value, ...]` list.
fn attr<'a>(node: &'a Node, name: &str) -> Option<&'a str> {
    let attributes = node.attributes.as_ref()?;
    let mut it = attributes.iter();
    while let (Some(key), Some(value)) = (it.next(), it.next()) {
        if key == name {
            return Some(value.as_str());
        }
    }
    None
}

/// Whether the already-fetched pierced tree contains at least one
/// `<iframe>` element at all. Zero extra CDP calls: `root` is the same tree
/// the top-level evidence walk already fetched. An ordinary page with no
/// `<iframe>` element anywhere never pays [`probe_oopif_challenges`]'s
/// cost.
///
/// Deliberately not narrowed to "`content_document: None`" — observed
/// live: right after the top-level page's own navigation completes, an
/// `<iframe>` that is *about* to become a genuine out-of-process child can
/// still carry its transient pre-navigation placeholder `content_document`
/// (the same "briefly still the pre-navigation placeholder" race
/// `FrameContext::resolve`'s own doc comment already documents for target
/// attachment) — checking `content_document.is_none()` here would miss
/// exactly the common case this seam exists to catch. Any `<iframe>`
/// element is enough to justify [`probe_oopif_challenges`]'s bounded poll;
/// a same-session child is cheaply ruled out there (`resolve_child` fails
/// closed for it) without ever reaching a second, duplicate evidence walk.
fn has_potential_oopif_candidate(node: &Node) -> bool {
    if node.node_name.eq_ignore_ascii_case("iframe") {
        return true;
    }
    if let Some(shadow_roots) = node.shadow_roots.as_ref() {
        if shadow_roots.iter().any(has_potential_oopif_candidate) {
            return true;
        }
    }
    if let Some(children) = node.children.as_ref() {
        if children.iter().any(has_potential_oopif_candidate) {
            return true;
        }
    }
    false
}

/// Recursively walk one pierced CDP document tree, tracking which `FrameId`
/// (and its parent) owns the current subtree as we descend into
/// `content_document` boundaries, and collecting every challenge-candidate
/// node that has at least one nested target-candidate. Returns the
/// target-candidate `id`s discovered in this subtree that have not yet been
/// consumed by an ancestor challenge candidate, so the caller can attach
/// them once it finishes visiting this node's own attributes.
fn scan_node(
    node: &Node,
    frame_id: &str,
    parent_frame_id: Option<&str>,
    out: &mut Vec<Evidence>,
) -> Vec<(String, BackendNodeId)> {
    let mut collected = Vec::new();

    if let Some(content_document) = node.content_document.as_deref() {
        // CDP's `DOM.Node.frameId` is documented on the *frame owner*
        // element (this `<iframe>` node itself) as the id of the frame it
        // owns — not on its `content_document`, which normally leaves
        // `frame_id` unset. Prefer the owner's own field; fall back to the
        // content document's (defensive, in case a future Chromium version
        // populates it there instead) before giving up and treating the
        // child as if it shared the parent's frame — a conservative
        // under-claim, never a false different-frame claim.
        let child_frame_id = node
            .frame_id
            .as_ref()
            .or(content_document.frame_id.as_ref())
            .map(|id| id.inner().clone())
            .unwrap_or_else(|| frame_id.to_string());
        collected.extend(scan_node(
            content_document,
            &child_frame_id,
            Some(frame_id),
            out,
        ));
    }

    if let Some(shadow_roots) = node.shadow_roots.as_ref() {
        for shadow_root in shadow_roots {
            collected.extend(scan_node(shadow_root, frame_id, parent_frame_id, out));
        }
    }

    if let Some(children) = node.children.as_ref() {
        for child in children {
            collected.extend(scan_node(child, frame_id, parent_frame_id, out));
        }
    }

    let is_target = attr(node, "role") == Some("button")
        && attr(node, "tabindex").is_some()
        && attr(node, "id").is_some_and(|value| !value.is_empty());
    if is_target {
        // Safe: guarded by `attr(node, "id").is_some_and(...)` above.
        collected.push((
            attr(node, "id").expect("checked above").to_string(),
            node.backend_node_id,
        ));
    }

    let is_challenge_candidate = attr(node, "role") == Some("application")
        && attr(node, "aria-label").is_some_and(|value| !value.trim().is_empty())
        && attr(node, "id").is_some_and(|value| !value.is_empty());
    if is_challenge_candidate && !collected.is_empty() {
        out.push(Evidence {
            frame_id: frame_id.to_string(),
            parent_frame_id: parent_frame_id.map(str::to_string),
            challenge_id: attr(node, "id").expect("checked above").to_string(),
            challenge_backend_node_id: node.backend_node_id,
            instruction: attr(node, "aria-label")
                .expect("checked above")
                .trim()
                .to_string(),
            target_ids: std::mem::take(&mut collected),
        });
    }

    collected
}

#[cfg(test)]
mod tests {
    use super::*;
    use chromiumoxide::cdp::browser_protocol::page::FrameId;

    fn attrs(pairs: &[(&str, &str)]) -> Option<Vec<String>> {
        Some(
            pairs
                .iter()
                .flat_map(|(k, v)| [k.to_string(), v.to_string()])
                .collect(),
        )
    }

    fn leaf(node_name: &str, attributes: Option<Vec<String>>) -> Node {
        Node {
            node_name: node_name.to_string(),
            attributes,
            ..Default::default()
        }
    }

    fn container(node_name: &str, attributes: Option<Vec<String>>, children: Vec<Node>) -> Node {
        Node {
            node_name: node_name.to_string(),
            attributes,
            children: Some(children),
            ..Default::default()
        }
    }

    /// Ordinary page: no evidence at all.
    #[test]
    fn normal_page_has_no_evidence() {
        let root = container("div", attrs(&[("class", "content")]), vec![leaf("p", None)]);
        let mut out = Vec::new();
        scan_node(&root, "top", None, &mut out);
        assert!(out.is_empty());
    }

    /// Text mentioning "captcha"/"verify"/"human" without the ARIA
    /// structural convention must never be evidence — proves this is not
    /// page-text matching.
    #[test]
    fn text_only_mentions_are_not_evidence() {
        let root = container(
            "div",
            attrs(&[
                ("class", "captcha-verify-human"),
                ("aria-label", "please verify you are human"),
            ]),
            vec![leaf(
                "span",
                attrs(&[("role", "button"), ("id", "b1")]), // no tabindex
            )],
        );
        let mut out = Vec::new();
        scan_node(&root, "top", None, &mut out);
        assert!(
            out.is_empty(),
            "plain text/class mentions must never be treated as evidence"
        );
    }

    /// The real, disclosed structural convention: `role=application` +
    /// non-empty `aria-label` + `id`, containing >=1 `role=button` +
    /// `tabindex` + `id` descendant.
    #[test]
    fn supported_challenge_structure_is_detected() {
        let target = leaf(
            "div",
            attrs(&[("role", "button"), ("tabindex", "0"), ("id", "pick-1")]),
        );
        let root = container(
            "div",
            attrs(&[
                ("role", "application"),
                ("aria-label", "select the matching point"),
                ("id", "challenge-1"),
            ]),
            vec![target],
        );
        let mut out = Vec::new();
        scan_node(&root, "top", None, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].challenge_id, "challenge-1");
        assert_eq!(
            out[0].target_ids,
            vec![("pick-1".to_string(), BackendNodeId(0))]
        );
        assert_eq!(out[0].frame_id, "top");
        assert_eq!(out[0].parent_frame_id, None);
    }

    /// Real CDP `backendNodeId` identity — never defaulted or reconstructed
    /// — is carried through for both the challenge candidate and every
    /// target candidate, needed verbatim by frame-scoped materialization
    /// (`SCORPION_CANONICAL_CAPTCHA_FRAME_ACTION_BINDING_001`).
    #[test]
    fn backend_node_ids_are_captured_verbatim_not_defaulted() {
        let target = Node {
            backend_node_id: BackendNodeId(77),
            ..leaf(
                "div",
                attrs(&[("role", "button"), ("tabindex", "0"), ("id", "pick-1")]),
            )
        };
        let root = Node {
            backend_node_id: BackendNodeId(99),
            ..container(
                "div",
                attrs(&[
                    ("role", "application"),
                    ("aria-label", "select the matching point"),
                    ("id", "challenge-1"),
                ]),
                vec![target],
            )
        };
        let mut out = Vec::new();
        scan_node(&root, "top", None, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].challenge_backend_node_id, BackendNodeId(99));
        assert_eq!(
            out[0].target_ids,
            vec![("pick-1".to_string(), BackendNodeId(77))]
        );
    }

    /// A challenge candidate with zero qualifying targets is not evidence —
    /// a point-selection challenge needs something to select.
    #[test]
    fn challenge_candidate_without_any_target_is_not_evidence() {
        let root = container(
            "div",
            attrs(&[
                ("role", "application"),
                ("aria-label", "select the matching point"),
                ("id", "challenge-1"),
            ]),
            vec![leaf("div", attrs(&[("role", "button")]))], // no tabindex, no id
        );
        let mut out = Vec::new();
        scan_node(&root, "top", None, &mut out);
        assert!(out.is_empty());
    }

    /// A challenge candidate missing `id` cannot be re-selected through the
    /// public pierced-selector API, so it must not count as evidence.
    #[test]
    fn challenge_candidate_without_id_is_not_evidence() {
        let target = leaf(
            "div",
            attrs(&[("role", "button"), ("tabindex", "0"), ("id", "pick-1")]),
        );
        let root = container(
            "div",
            attrs(&[
                ("role", "application"),
                ("aria-label", "select the matching point"),
                // no id
            ]),
            vec![target],
        );
        let mut out = Vec::new();
        scan_node(&root, "top", None, &mut out);
        assert!(out.is_empty());
    }

    /// A same-session child frame's evidence carries the exact child
    /// `FrameId` and its parent `FrameId` — not collapsed to the top-level
    /// frame.
    #[test]
    fn child_frame_evidence_preserves_frame_identity() {
        let target = leaf(
            "div",
            attrs(&[("role", "button"), ("tabindex", "0"), ("id", "pick-1")]),
        );
        let challenge = container(
            "div",
            attrs(&[
                ("role", "application"),
                ("aria-label", "select the matching point"),
                ("id", "challenge-1"),
            ]),
            vec![target],
        );
        let child_document = Node {
            node_name: "#document".to_string(),
            frame_id: Some(FrameId::new("child-frame")),
            children: Some(vec![challenge]),
            ..Default::default()
        };
        let root = Node {
            node_name: "iframe".to_string(),
            content_document: Some(Box::new(child_document)),
            ..Default::default()
        };
        let mut out = Vec::new();
        scan_node(&root, "top-frame", None, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].frame_id, "child-frame");
        assert_eq!(out[0].parent_frame_id, Some("top-frame".to_string()));
        assert_eq!(out[0].challenge_id, "challenge-1");
    }

    #[test]
    fn id_selector_escapes_quotes_and_backslashes() {
        assert_eq!(id_selector("plain"), "[id=\"plain\"]");
        assert_eq!(id_selector(r#"a"b\c"#), "[id=\"a\\\"b\\\\c\"]".to_string());
    }

    #[test]
    fn attr_reads_flat_pairs_and_missing_key_is_none() {
        let node = leaf("div", attrs(&[("role", "application"), ("id", "x")]));
        assert_eq!(attr(&node, "role"), Some("application"));
        assert_eq!(attr(&node, "id"), Some("x"));
        assert_eq!(attr(&node, "missing"), None);
    }
}
