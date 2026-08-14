//! Canonical provider-neutral Chromium frame-context identity seam.
//!
//! Establishes the authoritative chain
//! `FrameId -> TargetId -> SessionId -> ExecutionContextId -> frame-scoped DOM
//! identity -> frame-owner identity` for both the top-level Chromium frame
//! and genuine out-of-process (OOPIF) child frames, using only Chromium/CDP
//! facts reachable through `chromiumoxide::browser::Browser` and the
//! attached-session API it exposes. Every fact here is either read directly
//! from a CDP response or reused from an existing chromey-owned check;
//! nothing is inferred from a selector, DOM order, URL, origin or geometry.
//!
//! Coordinate transforms, snapshots, browser input actions and CAPTCHA
//! binding remain out of scope for this module; see `browser_challenge` for
//! the (currently top-level-only) action seam this module's successor will
//! extend to cover frames.

#![cfg(feature = "chrome")]

use chromiumoxide::browser::{AttachedSessionError, AttachedTargetSession, Browser};
use chromiumoxide::cdp::browser_protocol::dom::{
    BackendNodeId, DescribeNodeParams, GetFrameOwnerParams,
};
use chromiumoxide::cdp::browser_protocol::network::LoaderId;
use chromiumoxide::cdp::browser_protocol::page::{FrameId, GetFrameTreeParams};
use chromiumoxide::cdp::browser_protocol::target::{SessionId, TargetId, TargetInfo};
use chromiumoxide::cdp::js_protocol::runtime::{
    DisableParams as RuntimeDisableParams, EnableParams as RuntimeEnableParams, EvaluateParams,
    EventExecutionContextCreated, ExecutionContextId,
};
use chromiumoxide::types::CommandResponse;
use chromiumoxide::Command;
use serde_json::Value;
use std::time::Duration;
use tokio_stream::StreamExt;

/// How long to wait for Chromium to replay `Runtime.executionContextCreated`
/// after a forced disable/enable cycle before declaring the default context
/// unavailable.
const EXECUTION_CONTEXT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound on any single CDP round trip through an attached session. A session
/// racing a target/frame detach can have Chromium silently drop a command
/// without ever answering it; chromey's own `AttachedTargetSession` does not
/// bound that wait, so this seam does — a wedged session must fail closed,
/// never hang a caller forever.
const SESSION_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound on [`FrameContext::wait_for_frame_owner`]'s poll for Chromium to
/// recognize a just-attached child's frame ownership.
const FRAME_OWNER_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const FRAME_OWNER_DISCOVERY_INTERVAL: Duration = Duration::from_millis(50);

/// Run one attached-session operation under [`SESSION_COMMAND_TIMEOUT`],
/// mapping both its own failure and an expired deadline onto this seam's
/// typed vocabulary through the single translation point,
/// [`map_attached_session_error`].
async fn with_session_timeout<F, T>(future: F) -> Result<T, FrameContextFailure>
where
    F: std::future::Future<Output = std::result::Result<T, AttachedSessionError>>,
{
    match tokio::time::timeout(SESSION_COMMAND_TIMEOUT, future).await {
        Ok(result) => result.map_err(map_attached_session_error),
        Err(_) => Err(FrameContextFailure::TargetSessionUnavailable(
            AttachedSessionError::CommandRoutingFailed(chromiumoxide::error::CdpError::msg(
                "attached-session command exceeded the bounded wait",
            )),
        )),
    }
}

/// Typed frame-context resolution/revalidation failure.
///
/// Variants that chromey's `AttachedSessionError` already proves truthfully
/// (target/session liveness) are derived from it rather than reimplemented;
/// see [`map_attached_session_error`].
#[derive(Debug)]
pub enum FrameContextFailure {
    /// Chromium does not report the frame this context/operation needs.
    FrameUnavailable,
    /// No attached target could be proven to own the frame.
    FrameTargetAssociationUnavailable,
    /// More than one attached target could plausibly own the frame.
    FrameTargetAssociationAmbiguous,
    /// The owning target has no usable attached session right now.
    TargetSessionUnavailable(AttachedSessionError),
    /// No authoritative default execution context was found for the frame.
    ExecutionContextUnavailable,
    /// More than one candidate default execution context was found.
    ExecutionContextAmbiguous,
    /// The captured execution context no longer resolves.
    ExecutionContextChanged,
    /// The frame detached.
    FrameDetached,
    /// The frame navigated (new document, new or unchanged frame id).
    FrameNavigated,
    /// Chromium destroyed the owning target.
    TargetDetached,
    /// The owning target was replaced (unknown to this browser generation).
    TargetReplaced,
    /// The owning session was replaced by a new attachment generation.
    SessionChanged,
    /// The parent could not prove ownership of the child frame.
    FrameOwnerUnavailable,
    /// The parent's frame-owner element changed identity.
    FrameOwnerChanged,
    /// The captured DOM/backend-node identity no longer resolves.
    DomIdentityUnavailable,
    /// The active browser/frame context cannot be represented truthfully.
    UnsupportedContext,
}

/// Map chromey's own attached-session liveness vocabulary onto this seam's
/// typed failures. This is the single translation point; nothing else in
/// this module re-derives target/session liveness from scratch.
fn map_attached_session_error(error: AttachedSessionError) -> FrameContextFailure {
    match error {
        AttachedSessionError::TargetDestroyed => FrameContextFailure::TargetDetached,
        AttachedSessionError::SessionDetached => FrameContextFailure::FrameDetached,
        AttachedSessionError::SessionReplaced => FrameContextFailure::SessionChanged,
        AttachedSessionError::UnknownTarget => FrameContextFailure::TargetReplaced,
        other @ (AttachedSessionError::TargetNotAttached
        | AttachedSessionError::CommandRoutingFailed(_)) => {
            FrameContextFailure::TargetSessionUnavailable(other)
        }
    }
}

/// Whether a resolved [`FrameContext`] denotes the top-level Chromium frame
/// or a genuine out-of-process (OOPIF) child frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameClassification {
    /// The main, top-level frame of a page target.
    TopLevel,
    /// A genuine out-of-process child frame with its own target and session.
    Oopif,
}

/// Authoritative identity of the parent-context element that owns a child
/// frame, captured through the exact parent session that proved it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameOwnerIdentity {
    /// Backend-node identity of the owning `<iframe>` element, captured
    /// through the parent's own session via `DOM.getFrameOwner`.
    pub backend_node_id: BackendNodeId,
    /// Parent target identity at the moment ownership was proven.
    pub owner_target_id: TargetId,
    /// Parent session identity at the moment ownership was proven.
    pub owner_session_id: SessionId,
}

/// Frame-scoped DOM/backend-node identity, proven to resolve through the
/// exact owning session. This is never derived from a selector: callers
/// resolve a `BackendNodeId` themselves (e.g. via `DOM.getDocument` +
/// `DOM.querySelector` through this context's [`FrameContext::execute`]) and
/// hand it to [`FrameContext::resolve_dom_identity`] to bind it canonically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameDomIdentity {
    /// Exact backend-node identity, proven live through the owning session.
    pub backend_node_id: BackendNodeId,
}

/// Immutable canonical Chromium frame-context identity.
///
/// Holds exactly enough authoritative Chromium/CDP identity to prove
/// `FrameId -> TargetId -> SessionId -> ExecutionContextId`, plus the
/// frame-owner relationship for child frames. It never rebinds silently: a
/// [`FrameContext`] that fails [`FrameContext::revalidate`] must be
/// discarded and re-resolved from scratch by the caller, not repaired.
#[derive(Debug)]
pub struct FrameContext {
    /// Exact frame identity.
    pub frame_id: FrameId,
    /// Exact owning Chromium target identity.
    pub target_id: TargetId,
    /// Exact attached CDP session identity bound to `target_id`.
    pub session_id: SessionId,
    /// Exact authoritative default execution context for this frame.
    pub execution_context_id: ExecutionContextId,
    /// Exact loader identity of the frame's current document. A change here
    /// (with `frame_id` unchanged) is still a navigation.
    pub loader_id: LoaderId,
    /// Parent frame identity, if this is a child/OOPIF frame.
    pub parent_frame_id: Option<FrameId>,
    /// Frame-owner identity in the parent context, if this is a child/OOPIF
    /// frame.
    pub frame_owner: Option<FrameOwnerIdentity>,
    /// Top-level vs. OOPIF classification.
    pub classification: FrameClassification,
    /// System-unique execution-context identifier. `Runtime.executionContextDestroyed`
    /// reports only this string (never the numeric `ExecutionContextId`), so
    /// it is the sole correlation key for callers that listen for that event
    /// directly rather than using [`FrameContext::revalidate`]'s pull-based
    /// check.
    pub execution_context_unique_id: String,
    session: AttachedTargetSession,
}

impl FrameContext {
    /// Resolve the canonical context for a page's top-level frame.
    ///
    /// `target_info` must be an exact `TargetInfo` the caller observed
    /// directly from Chromium (e.g. via `Target.targetCreated` /
    /// `Target.attachedToTarget`) for a target of type `"page"`. Using the
    /// caller's own observed fact rather than re-querying it here avoids a
    /// race against Chromium still populating target metadata immediately
    /// after attachment.
    pub async fn resolve_top_level(
        browser: &Browser,
        target_info: &TargetInfo,
    ) -> Result<Self, FrameContextFailure> {
        Self::resolve(browser, target_info, None).await
    }

    /// Resolve the canonical context for a genuine OOPIF child frame,
    /// proving ownership through `parent`'s exact live session.
    ///
    /// `target_info` must be an exact `TargetInfo` the caller observed via
    /// `Target.attachedToTarget` (or `Target.targetCreated`) for a target of
    /// type `"iframe"`. If multiple candidate targets exist, disambiguate
    /// with [`select_unique_child_target`] before calling this.
    pub async fn resolve_child(
        browser: &Browser,
        parent: &FrameContext,
        target_info: &TargetInfo,
    ) -> Result<Self, FrameContextFailure> {
        Self::resolve(browser, target_info, Some(parent)).await
    }

    async fn resolve(
        browser: &Browser,
        target_info: &TargetInfo,
        parent: Option<&FrameContext>,
    ) -> Result<Self, FrameContextFailure> {
        // Coarse pre-filter only: never treat a page-type target as a child,
        // or an iframe-type target as top-level. The authoritative proof of
        // *which* frame owns this target is `DOM.getFrameOwner`, below —
        // `TargetInfo.parentFrameId` was observed (live, against a genuine
        // `--site-per-process` fixture) to sometimes report a transient
        // intermediate frame id instead of the ultimate parent, for a
        // dynamically inserted iframe that starts as a same-process frame
        // before Chromium swaps it out to a genuine OOPIF target. Relying on
        // it for the actual association proof is therefore not safe.
        let type_matches = match parent {
            None => target_info.r#type == "page",
            Some(_) => target_info.r#type == "iframe",
        };
        if !type_matches {
            return Err(FrameContextFailure::FrameTargetAssociationUnavailable);
        }

        let session = browser
            .attached_session(target_info.target_id.clone())
            .await
            .map_err(map_attached_session_error)?;

        // Immediately after a target attaches, its own default document can
        // briefly still be its pre-navigation placeholder — observed live:
        // `GetFrameTreeParams` right at attach can report a frame id for an
        // empty document that the parent's `DOM.getFrameOwner` does not yet
        // (and, for that placeholder, never will) recognize. For a child,
        // re-fetch both facts together, fresh each attempt, until Chromium
        // reports a frame id the parent actually recognizes as an owned
        // iframe, or fail closed after a bounded wait. A top-level target has
        // no ownership fact to corroborate, so one fetch is authoritative.
        let (frame_id, loader_id, frame_owner) = if let Some(parent) = parent {
            let (frame_id, loader_id, backend_node_id) =
                Self::wait_for_frame_and_owner(&session, parent).await?;
            (
                frame_id,
                loader_id,
                Some(FrameOwnerIdentity {
                    backend_node_id,
                    owner_target_id: parent.target_id.clone(),
                    owner_session_id: parent.session_id.clone(),
                }),
            )
        } else {
            let tree = with_session_timeout(session.execute(GetFrameTreeParams::default()))
                .await?
                .result
                .frame_tree;
            (tree.frame.id, tree.frame.loader_id, None)
        };

        let (execution_context_id, execution_context_unique_id) =
            resolve_default_execution_context(&session, &frame_id).await?;

        Ok(Self {
            frame_id,
            target_id: session.target_id().clone(),
            session_id: session.session_id().clone(),
            execution_context_id,
            loader_id,
            parent_frame_id: parent.map(|parent| parent.frame_id.clone()),
            frame_owner,
            classification: if parent.is_some() {
                FrameClassification::Oopif
            } else {
                FrameClassification::TopLevel
            },
            execution_context_unique_id,
            session,
        })
    }

    /// Poll the child's own `Page.getFrameTree` together with `parent`'s
    /// `DOM.getFrameOwner(frame_id)`, re-fetching the frame id fresh each
    /// attempt, for a bounded window — until Chromium reports a frame id the
    /// parent recognizes as an owned iframe. Fails closed if it never does.
    async fn wait_for_frame_and_owner(
        session: &AttachedTargetSession,
        parent: &FrameContext,
    ) -> Result<(FrameId, LoaderId, BackendNodeId), FrameContextFailure> {
        let deadline = tokio::time::Instant::now() + FRAME_OWNER_DISCOVERY_TIMEOUT;
        loop {
            let tree = with_session_timeout(session.execute(GetFrameTreeParams::default()))
                .await?
                .result
                .frame_tree;
            let frame_id = tree.frame.id;
            let loader_id = tree.frame.loader_id;
            let owner = with_session_timeout(
                parent
                    .session
                    .execute(GetFrameOwnerParams::new(frame_id.clone())),
            )
            .await;
            match owner {
                Ok(response) => return Ok((frame_id, loader_id, response.result.backend_node_id)),
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(FRAME_OWNER_DISCOVERY_INTERVAL).await;
                }
                Err(_) => return Err(FrameContextFailure::FrameTargetAssociationUnavailable),
            }
        }
    }

    /// Revalidate every authoritative fact this context asserts, without
    /// repairing or re-resolving anything. `parent` must be the caller's own
    /// current, live [`FrameContext`] for this context's parent frame when
    /// `classification` is [`FrameClassification::Oopif`]; it is ignored for
    /// [`FrameClassification::TopLevel`].
    ///
    /// `Ok(())` means every fact this context asserted at resolution time is
    /// still true right now. Any other outcome means this context must be
    /// discarded and re-resolved, never patched.
    pub async fn revalidate(
        &self,
        parent: Option<&FrameContext>,
    ) -> Result<(), FrameContextFailure> {
        with_session_timeout(self.session.validate()).await?;

        let tree = with_session_timeout(self.session.execute(GetFrameTreeParams::default()))
            .await?
            .result
            .frame_tree;
        if tree.frame.id != self.frame_id {
            return Err(FrameContextFailure::FrameNavigated);
        }
        if tree.frame.loader_id != self.loader_id {
            return Err(FrameContextFailure::FrameNavigated);
        }

        if self.classification == FrameClassification::Oopif {
            let owner = self
                .frame_owner
                .as_ref()
                .ok_or(FrameContextFailure::FrameOwnerUnavailable)?;
            let parent = parent.ok_or(FrameContextFailure::FrameOwnerUnavailable)?;
            let current_backend_node_id = with_session_timeout(
                parent
                    .session
                    .execute(GetFrameOwnerParams::new(self.frame_id.clone())),
            )
            .await
            .map_err(|_| FrameContextFailure::FrameOwnerUnavailable)?
            .result
            .backend_node_id;
            if current_backend_node_id != owner.backend_node_id
                || parent.target_id != owner.owner_target_id
                || parent.session_id != owner.owner_session_id
            {
                return Err(FrameContextFailure::FrameOwnerChanged);
            }
        }

        with_session_timeout(
            self.session.execute(
                EvaluateParams::builder()
                    .expression("true")
                    .context_id(self.execution_context_id)
                    .build()
                    .expect("expression is mandatory and was supplied"),
            ),
        )
        .await
        .map_err(|_| FrameContextFailure::ExecutionContextChanged)?;

        Ok(())
    }

    /// Bind an already-resolved backend-node identity (obtained by the
    /// caller through this context's exact session) as canonical frame-scoped
    /// DOM identity, proving it currently resolves through this session.
    pub async fn resolve_dom_identity(
        &self,
        backend_node_id: BackendNodeId,
    ) -> Result<FrameDomIdentity, FrameContextFailure> {
        with_session_timeout(self.session.execute(DescribeNodeParams {
            backend_node_id: Some(backend_node_id),
            ..Default::default()
        }))
        .await
        .map_err(|_| FrameContextFailure::DomIdentityUnavailable)?;
        Ok(FrameDomIdentity { backend_node_id })
    }

    /// Revalidate a previously bound [`FrameDomIdentity`] against this exact
    /// session. Does not re-query by selector or accept a replacement node.
    pub async fn revalidate_dom_identity(
        &self,
        identity: FrameDomIdentity,
    ) -> Result<(), FrameContextFailure> {
        with_session_timeout(self.session.execute(DescribeNodeParams {
            backend_node_id: Some(identity.backend_node_id),
            ..Default::default()
        }))
        .await
        .map_err(|_| FrameContextFailure::DomIdentityUnavailable)?;
        Ok(())
    }

    /// Execute one CDP command through this exact frame's owning session.
    /// This is the only sanctioned way for downstream callers (browser
    /// action/CAPTCHA seams) to run a frame-scoped command: it never exposes
    /// the underlying chromey `AttachedTargetSession`.
    pub async fn execute<T: Command>(
        &self,
        command: T,
    ) -> Result<CommandResponse<T::Response>, FrameContextFailure> {
        with_session_timeout(self.session.execute(command)).await
    }
}

/// Select the unique attached iframe target that is a direct child of
/// `parent_frame_id`, using only Chromium-reported `TargetInfo` facts. Fails
/// closed (never guesses) when zero or more than one candidate qualifies.
///
/// This is a convenience pre-filter over already-observed attach candidates,
/// not the authoritative association proof — [`FrameContext::resolve_child`]
/// independently proves ownership via `DOM.getFrameOwner` regardless of what
/// this returns. Prefer it for disambiguating between genuine sibling
/// candidates; a dynamically inserted iframe's `TargetInfo.parentFrameId` can
/// transiently name an intermediate frame rather than the ultimate parent
/// (see `FrameContext::resolve`), so a caller that already knows the exact
/// candidate `TargetId` should skip this and call `resolve_child` directly.
pub fn select_unique_child_target(
    parent_frame_id: &FrameId,
    candidates: &[TargetInfo],
) -> Result<TargetId, FrameContextFailure> {
    let mut matches = candidates.iter().filter(|candidate| {
        candidate.r#type == "iframe" && candidate.parent_frame_id.as_ref() == Some(parent_frame_id)
    });
    let first = matches
        .next()
        .ok_or(FrameContextFailure::FrameTargetAssociationUnavailable)?;
    if matches.next().is_some() {
        return Err(FrameContextFailure::FrameTargetAssociationAmbiguous);
    }
    Ok(first.target_id.clone())
}

/// Resolve the exact authoritative default execution context for `frame_id`
/// within `session`, by forcing Chromium to replay
/// `Runtime.executionContextCreated` (disable then enable) and selecting the
/// unique context whose `auxData` reports `isDefault: true` for this frame.
/// Isolated worlds, extension worlds and contexts for other frames are
/// rejected; the first event is never selected blindly.
async fn resolve_default_execution_context(
    session: &AttachedTargetSession,
    frame_id: &FrameId,
) -> Result<(ExecutionContextId, String), FrameContextFailure> {
    with_session_timeout(session.execute(RuntimeDisableParams::default())).await?;
    let mut contexts =
        with_session_timeout(session.event_listener::<EventExecutionContextCreated>()).await?;
    with_session_timeout(session.execute(RuntimeEnableParams::default())).await?;

    let mut found: Option<(ExecutionContextId, String)> = None;
    let deadline = tokio::time::Instant::now() + EXECUTION_CONTEXT_DISCOVERY_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let event = match tokio::time::timeout(remaining, contexts.next()).await {
            Ok(Some(event)) => event,
            Ok(None) | Err(_) => break,
        };
        let aux_data = event.context.aux_data.as_ref();
        let is_default = aux_data
            .and_then(|data| data.get("isDefault"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let event_frame_id = aux_data
            .and_then(|data| data.get("frameId"))
            .and_then(Value::as_str);
        if is_default && event_frame_id == Some(frame_id.inner().as_str()) {
            if found.is_some() {
                return Err(FrameContextFailure::ExecutionContextAmbiguous);
            }
            found = Some((event.context.id, event.context.unique_id.clone()));
        }
    }
    found.ok_or(FrameContextFailure::ExecutionContextUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_id(value: &str) -> FrameId {
        FrameId::new(value)
    }

    fn target_info(kind: &str, target_id: &str, parent_frame_id: Option<&str>) -> TargetInfo {
        TargetInfo {
            target_id: TargetId::new(target_id.to_string()),
            r#type: kind.to_string(),
            title: String::new(),
            url: String::new(),
            attached: true,
            opener_id: None,
            can_access_opener: false,
            opener_frame_id: None,
            parent_frame_id: parent_frame_id.map(frame_id),
            browser_context_id: None,
            subtype: None,
        }
    }

    #[test]
    fn unique_child_selection_requires_exactly_one_match() {
        let parent = frame_id("parent-frame");
        let candidates = vec![target_info("iframe", "child-a", Some("parent-frame"))];
        let selected = select_unique_child_target(&parent, &candidates).unwrap();
        assert_eq!(selected, TargetId::new("child-a".to_string()));
    }

    #[test]
    fn ambiguous_siblings_fail_closed() {
        let parent = frame_id("parent-frame");
        let candidates = vec![
            target_info("iframe", "child-a", Some("parent-frame")),
            target_info("iframe", "child-b", Some("parent-frame")),
        ];
        assert!(matches!(
            select_unique_child_target(&parent, &candidates),
            Err(FrameContextFailure::FrameTargetAssociationAmbiguous)
        ));
    }

    #[test]
    fn unrelated_target_type_is_excluded() {
        let parent = frame_id("parent-frame");
        let candidates = vec![target_info("page", "other-page", Some("parent-frame"))];
        assert!(matches!(
            select_unique_child_target(&parent, &candidates),
            Err(FrameContextFailure::FrameTargetAssociationUnavailable)
        ));
    }

    #[test]
    fn different_parent_is_excluded() {
        let parent = frame_id("parent-frame");
        let candidates = vec![target_info("iframe", "cousin", Some("other-parent"))];
        assert!(matches!(
            select_unique_child_target(&parent, &candidates),
            Err(FrameContextFailure::FrameTargetAssociationUnavailable)
        ));
    }

    #[test]
    fn no_candidates_fail_closed_as_unavailable_not_ambiguous() {
        let parent = frame_id("parent-frame");
        assert!(matches!(
            select_unique_child_target(&parent, &[]),
            Err(FrameContextFailure::FrameTargetAssociationUnavailable)
        ));
    }
}
