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
//! frames, but *not* a genuine out-of-process (OOPIF) child, which lives on
//! a separate CDP target this seam never attaches to. For a same-session
//! child, this module reports truthful, correct frame identity (the exact
//! `FrameId`, and its parent `FrameId`) but does not attempt
//! [`BrowserChallengeSnapshot::capture_in_frame`] — that requires a
//! [`crate::features::frame_context::FrameContext`], which in turn requires
//! `Target.attachedToTarget`/`TargetInfo` identity that this seam's caller
//! (`Page::new_base`, the shared chrome-backed `Page` constructor) does not
//! currently retain. That remains a disclosed follow-up, not part of this
//! seam's claim.

#![cfg(feature = "chrome")]

use chromiumoxide::cdp::browser_protocol::dom::Node;
use chromiumoxide::Page;

use crate::features::browser_challenge::{BrowserChallengeFailure, BrowserChallengeSnapshot};

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
    /// real and correct; materialization is intentionally not attempted —
    /// see the module-level "Frame scope" section.
    FramedEvidence {
        /// Exact `FrameId` (as reported by Chromium) owning the matched
        /// challenge evidence.
        frame_id: String,
        /// Exact `FrameId` of the parent document, when known.
        parent_frame_id: Option<String>,
        /// `id` attribute of the matched challenge-candidate element,
        /// preserved for a later frontier's frame-scoped materialization.
        challenge_element_id: String,
    },
}

/// One matched challenge candidate, gathered from an in-memory walk of a
/// single `DOM.getDocument(pierce: true)` snapshot.
struct Evidence {
    frame_id: String,
    parent_frame_id: Option<String>,
    challenge_id: String,
    /// The matched challenge candidate's own `aria-label` value — a real
    /// accessible name the page's author wrote, not a caller-supplied
    /// prompt. Carried through so a later provider-routing frontier can use
    /// it as the canonical challenge instruction without re-querying.
    instruction: String,
    target_ids: Vec<String>,
}

/// Passively inspect a real Chrome page for a supported, evidence-based
/// challenge. Never mutates the page. `Ok(None)` means the page was
/// genuinely inspected and no supported challenge evidence was found — it
/// does not mean the detector failed.
pub async fn detect_browser_challenge(
    page: &Page,
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
        return Ok(None);
    };

    if matched.frame_id != top_frame_id {
        return Ok(Some(DetectedBrowserChallenge::FramedEvidence {
            frame_id: matched.frame_id,
            parent_frame_id: matched.parent_frame_id,
            challenge_element_id: matched.challenge_id,
        }));
    }

    let challenge_element_id = matched.challenge_id;
    let instruction = matched.instruction;
    let challenge_element = page
        .find_element_pierced(id_selector(&challenge_element_id))
        .await
        .map_err(|_| ChallengeDetectionFailure::ObservationFailed)?;

    let mut targets = Vec::with_capacity(matched.target_ids.len());
    for target_id in matched.target_ids {
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

impl DetectedBrowserChallenge {
    /// Route this challenge through the canonical provider router
    /// (`crate::features::solvers::route_detected_browser_challenge`) if,
    /// and only if, it was fully materialized in the top-level document.
    /// Framed evidence is never routed in this frontier — see the
    /// module-level "Frame scope" section. Never mutates the browser: the
    /// router itself performs no click/type/submit, only an explicit
    /// provider `solve()` attempt (or none, when no provider is
    /// configured).
    pub(crate) async fn route(
        &self,
        page: &Page,
        selected_provider: Option<crate::features::captcha::CaptchaProviderId>,
        deadline: std::time::Duration,
    ) -> Option<crate::features::captcha::CaptchaRouteOutcomeSummary> {
        let Self::TopLevel {
            snapshot,
            instruction,
            ..
        } = self
        else {
            return None;
        };
        let challenge = crate::features::captcha::CaptchaChallenge {
            kind: crate::features::captcha::CaptchaChallengeKind::PointSelection,
            instruction: instruction.clone(),
            visuals: vec![crate::features::captcha::CaptchaVisualInput::materialized(
                None,
                "image/png",
                snapshot.visual_bytes.clone(),
            )],
        };
        Some(
            crate::features::solvers::route_detected_browser_challenge(
                page,
                challenge,
                selected_provider,
                deadline,
            )
            .await,
        )
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
            } => BrowserChallengeObservation::Framed {
                frame_id,
                parent_frame_id,
                challenge_element_id,
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
) -> Vec<String> {
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
        collected.push(attr(node, "id").expect("checked above").to_string());
    }

    let is_challenge_candidate = attr(node, "role") == Some("application")
        && attr(node, "aria-label").is_some_and(|value| !value.trim().is_empty())
        && attr(node, "id").is_some_and(|value| !value.is_empty());
    if is_challenge_candidate && !collected.is_empty() {
        out.push(Evidence {
            frame_id: frame_id.to_string(),
            parent_frame_id: parent_frame_id.map(str::to_string),
            challenge_id: attr(node, "id").expect("checked above").to_string(),
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
        assert_eq!(out[0].target_ids, vec!["pick-1".to_string()]);
        assert_eq!(out[0].frame_id, "top");
        assert_eq!(out[0].parent_frame_id, None);
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
