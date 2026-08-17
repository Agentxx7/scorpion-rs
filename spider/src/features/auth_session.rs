//! Canonical authenticated-session lifecycle.
//!
//! Track 5 of the roadmap. Realizes the lifecycle half of `SCORPION.md`
//! §5 ("Authenticated Research") — identity ([`AuthSessionId`]) lives in
//! [`crate::features::identity`]; the transition contract this module
//! uses (current state / historical record / transition) is
//! [`crate::features::domain_state`]'s, unmodified; persistence is
//! [`crate::features::domain_persistence::DomainPersistence`], unmodified.
//! This module is the first to actually *use* Track 2's full contract —
//! Track 4's evidence ledger only ever needed append-only history because
//! evidence has no current state; an authenticated session's whole point
//! is that it *does* have one, replaced over time by explicit transitions.
//!
//! # Collision audit (this frontier's required reconciliation)
//!
//! "Session" already names three unrelated things in this codebase.
//! [`AuthSessionId`]/this module redefine none of them:
//!
//! - `chromiumoxide::cdp::browser_protocol::target::SessionId` — a CDP
//!   protocol value identifying one attached browser-automation
//!   connection. Transport-layer plumbing; carries no notion of "is the
//!   user logged in."
//! - `features/frame_context.rs`'s `FrameContext::session_id`/
//!   `owner_session_id` — the same CDP `SessionId` above, used to
//!   identify which attached session owns which frame in the canonical
//!   OOPIF frame-identity chain. Also transport-layer plumbing.
//! - `spider_mcp`'s `CrawlSession`/`CrawlSessionStatus` — in-memory,
//!   `DashMap<String, CrawlSession>`-keyed tracking of one async MCP
//!   tool-call's crawl progress (`Running`/`Complete`/`Failed`), evicted
//!   by a TTL/LRU policy. Server bookkeeping for a single tool
//!   invocation; unrelated to authentication and not durable.
//!
//! None of the three represent "this identity is currently authenticated
//! against some origin, and that fact should survive across requests." An
//! `AuthSessionId` may outlive any single crawl, any single browser
//! connection, and any single MCP tool call — it names none of those
//! things.
//!
//! # Secret/identity separation proof
//!
//! [`AuthSessionId`] is 16 opaque random bytes (see
//! `features/identity.rs`) — there is no field, variant, or constructor
//! through which a cookie, `Authorization` value, token, or credential
//! could ever enter it. [`AuthSessionState`] (the durable lifecycle
//! record this module persists) carries only `origin` (a plain string
//! naming *which* origin the session authenticates against — required by
//! §5's "authentication is origin/policy scoped" rule),
//! [`AuthenticationProfile`] (§5's locked method-vocabulary enum — a
//! classification, not a secret), and, while paused,
//! [`BrowserContinuityToken`] (an opaque, caller-supplied *reference*
//! proving which existing browser/cookie-jar context was reused — never
//! the cookie jar's contents, never a token value). None of these types
//! can hold secret material; this module never imports a cookie jar,
//! `reqwest::header::HeaderValue`, or any credential type, and never
//! will (`SCORPION.md` §5's `CredentialRef` — referenced, never embedded
//! — remains locked/undefined, exactly as before this frontier; this
//! module does not implement it).
//!
//! # Lifecycle vocabulary (source/domain justified, not invented)
//!
//! Three states — `Active`, `Paused`, `Invalidated` — directly realize
//! §5's own words: sessions that need "pausing and resuming," and,
//! separately, revocation (a credential that must "fail closed," never
//! silently continue). "Resumed" is deliberately **not** a fourth state:
//! after a resume succeeds, the session's current state is `Active`,
//! indistinguishable from a session that was never paused except by its
//! historical record — treating "resumed" as a state rather than the
//! *transition* that produces `Active` again would be inventing a
//! distinction for symmetry, which this frontier's own scope forbids.
//! [`ResumeSession`] is a transition, not a state.
//!
//! `Invalidated` is terminal — no transition in this module leads out of
//! it. Once a session is invalidated there is no "un-invalidate"; a new
//! authentication produces a new [`AuthSessionId`].
//!
//! # Browser-session continuity (§5's pause/resume rule, made truthful)
//!
//! §5: *"Future MFA/interactive authentication must support pausing and
//! resuming the same authenticated browser session — not
//! re-authenticating from scratch, and not silently continuing
//! unauthenticated."* [`PauseSession`] records a
//! [`BrowserContinuityToken`] the caller derived from whatever existing
//! canonical browser/frame/session primitive it actually kept alive
//! across the pause (e.g. a stable hash of a still-live cookie jar's
//! identity, or of the still-attached CDP session/target the browser
//! context never tore down — this module does not construct or
//! prescribe the derivation; doing so would mean building new browser
//! architecture, out of this frontier's scope). [`ResumeSession`] must
//! present that *exact same* token to succeed. A resume presenting a
//! different (or no) token is rejected —
//! [`AuthSessionTransitionRejected::ContinuityMismatch`] — rather than
//! silently accepted as continuous. This is what makes "preserves
//! continuity" a checked, falsifiable property instead of a comment: it
//! is structurally impossible to resume a paused session without
//! presenting proof of the same underlying context, and there is no
//! other way to reach `Active` from `Paused` in this module.

#[cfg(all(feature = "disk", feature = "serde"))]
use crate::features::domain_state::Applied;
#[cfg(any(test, all(feature = "disk", feature = "serde")))]
use crate::features::domain_state::CurrentState;
use crate::features::domain_state::Transition;
#[cfg(any(test, all(feature = "disk", feature = "serde")))]
use crate::features::identity::AuthSessionId;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Locked authentication-method vocabulary — `SCORPION.md` §5. Names
/// only; this frontier implements no concrete authentication flow for
/// any variant (form submission, OAuth redirect handling, etc. remain
/// unbuilt) — this enum exists so [`AuthSessionState`] can truthfully
/// record *which* locked method classification a session belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum AuthenticationProfile {
    /// No authentication.
    None,
    /// Credentials submitted through an HTML form.
    FormLogin,
    /// HTTP Basic authentication.
    BasicAuth,
    /// A bearer token presented on each request.
    BearerToken,
    /// An established cookie-based session.
    CookieSession,
    /// OAuth-family delegated authentication.
    OAuth,
    /// A live, human/automated browser session — the profile §5's
    /// pause/resume rule specifically names.
    InteractiveBrowser,
}

/// An opaque proof that a pause/resume boundary reused the *same*
/// underlying browser/cookie-jar context, rather than a fresh one
/// silently substituted for it. Carries no secret material — a
/// caller-supplied reference (e.g. a hash of a live primitive's stable
/// identity), never a cookie or token value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct BrowserContinuityToken(String);

impl BrowserContinuityToken {
    /// Wrap an opaque continuity reference. Callers derive `token` from
    /// whatever existing canonical browser/frame/session primitive they
    /// actually kept alive (see this module's doc comment) — this
    /// constructor does not validate or interpret it.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// Borrow the opaque reference value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The durable, current lifecycle state of one authenticated session.
/// Every variant carries `origin` and `profile` — §5's "origin/policy
/// scoped" rule means a session is never meaningfully stateless of
/// either.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum AuthSessionState {
    /// Authenticated and currently usable.
    Active {
        /// The origin this session is scoped to.
        origin: String,
        /// Which locked authentication method this session used.
        profile: AuthenticationProfile,
    },
    /// Temporarily suspended — not usable for requests, not revoked.
    /// Carries the continuity proof a resume must match exactly.
    Paused {
        /// The origin this session is scoped to.
        origin: String,
        /// Which locked authentication method this session used.
        profile: AuthenticationProfile,
        /// Proof of which browser/cookie-jar context must be reused to
        /// resume this session truthfully.
        continuity: BrowserContinuityToken,
    },
    /// Permanently revoked. Terminal: no transition in this module leads
    /// out of this state.
    Invalidated {
        /// The origin this session was scoped to.
        origin: String,
        /// Which locked authentication method this session used.
        profile: AuthenticationProfile,
    },
}

impl AuthSessionState {
    /// The origin every variant carries.
    pub fn origin(&self) -> &str {
        match self {
            AuthSessionState::Active { origin, .. }
            | AuthSessionState::Paused { origin, .. }
            | AuthSessionState::Invalidated { origin, .. } => origin,
        }
    }

    /// The authentication profile every variant carries.
    pub fn profile(&self) -> AuthenticationProfile {
        match self {
            AuthSessionState::Active { profile, .. }
            | AuthSessionState::Paused { profile, .. }
            | AuthSessionState::Invalidated { profile, .. } => *profile,
        }
    }

    /// A short, stable label for this variant — used only for error
    /// reporting (never persisted, never a lifecycle decision input).
    fn kind(&self) -> &'static str {
        match self {
            AuthSessionState::Active { .. } => "active",
            AuthSessionState::Paused { .. } => "paused",
            AuthSessionState::Invalidated { .. } => "invalidated",
        }
    }
}

/// Why an [`AuthSessionState`] transition did not apply. Every variant
/// names a structural reason — never a fabricated one — and the
/// transition it came from left the session's current state completely
/// unchanged (see [`crate::features::domain_state::CurrentState::apply`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSessionTransitionRejected {
    /// The transition does not apply from the session's current state
    /// (e.g. resuming a session that is not paused).
    InvalidFromState {
        /// The transition that was attempted.
        attempted: &'static str,
        /// The state it was attempted from.
        from: &'static str,
    },
    /// A resume was attempted with a [`BrowserContinuityToken`] that does
    /// not match the one recorded when the session was paused — resuming
    /// would mean silently substituting a different (or absent) browser
    /// context and claiming continuity that was never proven.
    ContinuityMismatch,
    /// The session is [`AuthSessionState::Invalidated`] — terminal; no
    /// transition exists out of it.
    Terminal,
}

impl std::fmt::Display for AuthSessionTransitionRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthSessionTransitionRejected::InvalidFromState { attempted, from } => {
                write!(f, "{attempted} does not apply from state {from}")
            }
            AuthSessionTransitionRejected::ContinuityMismatch => write!(
                f,
                "resume rejected: presented continuity token does not match the \
                 token recorded at pause time"
            ),
            AuthSessionTransitionRejected::Terminal => {
                write!(f, "session is invalidated; no transition applies")
            }
        }
    }
}

impl std::error::Error for AuthSessionTransitionRejected {}

/// Suspend an active session, recording proof of which browser context
/// must be reused to resume it truthfully. Only applies from
/// [`AuthSessionState::Active`].
#[derive(Debug, Clone)]
pub struct PauseSession {
    /// Proof of the browser/cookie-jar context being kept alive across
    /// the pause.
    pub continuity: BrowserContinuityToken,
}

impl Transition<AuthSessionState> for PauseSession {
    type Rejection = AuthSessionTransitionRejected;

    fn apply(&self, current: &AuthSessionState) -> Result<AuthSessionState, Self::Rejection> {
        match current {
            AuthSessionState::Active { origin, profile } => Ok(AuthSessionState::Paused {
                origin: origin.clone(),
                profile: *profile,
                continuity: self.continuity.clone(),
            }),
            AuthSessionState::Invalidated { .. } => Err(AuthSessionTransitionRejected::Terminal),
            other => Err(AuthSessionTransitionRejected::InvalidFromState {
                attempted: "pause",
                from: other.kind(),
            }),
        }
    }
}

/// Resume a paused session — only if `continuity` matches exactly what
/// was recorded when it was paused. Only applies from
/// [`AuthSessionState::Paused`]; a mismatched (or, trivially, any other)
/// token fails closed rather than resuming with an unproven context.
#[derive(Debug, Clone)]
pub struct ResumeSession {
    /// Proof of the browser/cookie-jar context being resumed with. Must
    /// equal the token [`PauseSession`] recorded, or this transition is
    /// rejected.
    pub continuity: BrowserContinuityToken,
}

impl Transition<AuthSessionState> for ResumeSession {
    type Rejection = AuthSessionTransitionRejected;

    fn apply(&self, current: &AuthSessionState) -> Result<AuthSessionState, Self::Rejection> {
        match current {
            AuthSessionState::Paused {
                origin,
                profile,
                continuity,
            } => {
                if *continuity == self.continuity {
                    Ok(AuthSessionState::Active {
                        origin: origin.clone(),
                        profile: *profile,
                    })
                } else {
                    Err(AuthSessionTransitionRejected::ContinuityMismatch)
                }
            }
            AuthSessionState::Invalidated { .. } => Err(AuthSessionTransitionRejected::Terminal),
            other => Err(AuthSessionTransitionRejected::InvalidFromState {
                attempted: "resume",
                from: other.kind(),
            }),
        }
    }
}

/// Permanently revoke a session. Applies from [`AuthSessionState::Active`]
/// or [`AuthSessionState::Paused`]; rejected (as
/// [`AuthSessionTransitionRejected::Terminal`]) if the session is already
/// invalidated — invalidation is not idempotent-by-silent-success, it is
/// simply refused a second time.
#[derive(Debug, Clone, Copy, Default)]
pub struct InvalidateSession;

impl Transition<AuthSessionState> for InvalidateSession {
    type Rejection = AuthSessionTransitionRejected;

    fn apply(&self, current: &AuthSessionState) -> Result<AuthSessionState, Self::Rejection> {
        match current {
            AuthSessionState::Active { origin, profile }
            | AuthSessionState::Paused {
                origin, profile, ..
            } => Ok(AuthSessionState::Invalidated {
                origin: origin.clone(),
                profile: *profile,
            }),
            AuthSessionState::Invalidated { .. } => Err(AuthSessionTransitionRejected::Terminal),
        }
    }
}

// ---------------------------------------------------------------------------
// Persistence (Track 3: DomainPersistence, unmodified). Current state is
// compare-and-swap (write_current); each superseded state becomes an
// immutable, append-only historical record (append_history) — this is
// the first capability that needs and uses *both* of Track 3's
// primitives together, wired through Track 2's CurrentState::apply.
// ---------------------------------------------------------------------------

/// Failure creating, reading, or transitioning a durable authenticated
/// session. Storage/domain-shaped only.
#[cfg(all(feature = "disk", feature = "serde"))]
#[derive(Debug)]
pub enum AuthSessionError {
    /// No session is recorded for the given `AuthSessionId`.
    NotFound,
    /// The attempted transition does not apply to the session's current
    /// state. The session's persisted state is unchanged.
    TransitionRejected(AuthSessionTransitionRejected),
    /// Another writer changed this session's persisted state between
    /// this call's read and write — a genuine concurrent-modification
    /// race (Track 3's compare-and-swap fail-closed behavior), not an
    /// invalid domain transition. The session's persisted state is
    /// unchanged; retry with a fresh read.
    ConcurrentModification,
    /// A backend/persistence failure unrelated to the above.
    Persistence(crate::features::domain_persistence::PersistenceError),
    /// The state could not be encoded/decoded.
    Serialization(serde_json::Error),
}

#[cfg(all(feature = "disk", feature = "serde"))]
impl std::fmt::Display for AuthSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthSessionError::NotFound => write!(f, "no authenticated session recorded"),
            AuthSessionError::TransitionRejected(rejection) => write!(f, "{rejection}"),
            AuthSessionError::ConcurrentModification => {
                write!(
                    f,
                    "session state changed concurrently; retry with a fresh read"
                )
            }
            AuthSessionError::Persistence(error) => write!(f, "auth session ledger: {error}"),
            AuthSessionError::Serialization(error) => {
                write!(
                    f,
                    "auth session ledger: state serialization failed: {error}"
                )
            }
        }
    }
}

#[cfg(all(feature = "disk", feature = "serde"))]
impl std::error::Error for AuthSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AuthSessionError::TransitionRejected(rejection) => Some(rejection),
            AuthSessionError::Persistence(error) => Some(error),
            AuthSessionError::Serialization(error) => Some(error),
            AuthSessionError::NotFound | AuthSessionError::ConcurrentModification => None,
        }
    }
}

/// Mint a fresh [`AuthSessionId`] and durably record it as
/// [`AuthSessionState::Active`] for `origin`/`profile`. The first write
/// for a freshly minted id — [`DomainPersistence::write_current`] with
/// `expected_revision: None` — so this can only fail if the astronomically
/// unlikely happens and the id already has a row.
#[cfg(all(feature = "disk", feature = "serde"))]
pub async fn create_session(
    store: &crate::features::domain_persistence::DomainPersistence,
    origin: String,
    profile: AuthenticationProfile,
) -> Result<(AuthSessionId, AuthSessionState), AuthSessionError> {
    let id = AuthSessionId::new();
    let state = AuthSessionState::Active { origin, profile };
    let payload = serde_json::to_vec(&state).map_err(AuthSessionError::Serialization)?;

    store
        .write_current(&id.to_string(), None, &payload)
        .await
        .map_err(AuthSessionError::Persistence)?;

    Ok((id, state))
}

/// Read the current durable state of `id`, together with its storage
/// revision (needed by nothing outside this module today, but returned
/// for callers that want to detect their own races without going through
/// [`apply_session_transition`]). `Ok(None)` if no session is recorded.
#[cfg(all(feature = "disk", feature = "serde"))]
pub async fn read_current_session(
    store: &crate::features::domain_persistence::DomainPersistence,
    id: AuthSessionId,
) -> Result<Option<(u64, AuthSessionState)>, AuthSessionError> {
    match store
        .read_current(&id.to_string())
        .await
        .map_err(AuthSessionError::Persistence)?
    {
        Some((revision, payload)) => {
            let state =
                serde_json::from_slice(&payload).map_err(AuthSessionError::Serialization)?;
            Ok(Some((revision, state)))
        }
        None => Ok(None),
    }
}

/// Apply `transition` to `id`'s current durable state: read the current
/// state and revision, run it through
/// [`CurrentState::apply`](crate::features::domain_state::CurrentState::apply)
/// (the canonical `current state + explicit transition → new current
/// state` contract, unmodified), then durably record both halves of the
/// result — the new current state via
/// [`DomainPersistence::write_current`] (compare-and-swap against the
/// revision just read, so a concurrent writer racing this call is
/// rejected, never silently lost) and the just-superseded state via
/// [`DomainPersistence::append_history`] (immutable, append-only).
///
/// On an invalid transition, nothing is written — the session's
/// persisted state is exactly what it was before this call — and
/// `Err(AuthSessionError::TransitionRejected(_))` names why.
///
/// [`DomainPersistence::write_current`]: crate::features::domain_persistence::DomainPersistence::write_current
/// [`DomainPersistence::append_history`]: crate::features::domain_persistence::DomainPersistence::append_history
#[cfg(all(feature = "disk", feature = "serde"))]
pub async fn apply_session_transition<T>(
    store: &crate::features::domain_persistence::DomainPersistence,
    id: AuthSessionId,
    transition: &T,
) -> Result<AuthSessionState, AuthSessionError>
where
    T: Transition<AuthSessionState, Rejection = AuthSessionTransitionRejected>,
{
    let (revision, current_state) = read_current_session(store, id)
        .await?
        .ok_or(AuthSessionError::NotFound)?;

    let current = CurrentState::new(id, current_state);
    let Applied {
        current: new_current,
        superseded,
    } = match current.apply(transition) {
        Ok(applied) => applied,
        Err((_unchanged, rejection)) => {
            return Err(AuthSessionError::TransitionRejected(rejection));
        }
    };

    let new_payload =
        serde_json::to_vec(new_current.state()).map_err(AuthSessionError::Serialization)?;
    store
        .write_current(&id.to_string(), Some(revision), &new_payload)
        .await
        .map_err(|error| match error {
            crate::features::domain_persistence::PersistenceError::CurrentStateConflict {
                ..
            } => AuthSessionError::ConcurrentModification,
            other => AuthSessionError::Persistence(other),
        })?;

    let superseded_payload =
        serde_json::to_vec(superseded.state()).map_err(AuthSessionError::Serialization)?;
    store
        .append_history(
            &id.to_string(),
            revision,
            &superseded_payload,
            superseded.recorded_at(),
        )
        .await
        .map_err(AuthSessionError::Persistence)?;

    Ok(new_current.into_parts().1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active(origin: &str) -> AuthSessionState {
        AuthSessionState::Active {
            origin: origin.to_string(),
            profile: AuthenticationProfile::InteractiveBrowser,
        }
    }

    #[test]
    fn pause_then_resume_with_matching_token_returns_to_active() {
        let token = BrowserContinuityToken::new("cookie-jar-hash-abc");
        let current = CurrentState::new(AuthSessionId::new(), active("https://example.test"));

        let paused = current
            .apply(&PauseSession {
                continuity: token.clone(),
            })
            .expect("pause from Active must succeed");
        assert!(matches!(
            paused.current.state(),
            AuthSessionState::Paused { .. }
        ));
        assert_eq!(paused.superseded.state().kind(), "active");

        let resumed = paused
            .current
            .apply(&ResumeSession { continuity: token })
            .expect("resume with matching continuity must succeed");
        assert!(matches!(
            resumed.current.state(),
            AuthSessionState::Active { .. }
        ));
        assert_eq!(resumed.superseded.state().kind(), "paused");
    }

    #[test]
    fn resume_with_mismatched_continuity_fails_closed_and_leaves_state_unchanged() {
        let current = CurrentState::new(AuthSessionId::new(), active("https://example.test"));
        let paused = current
            .apply(&PauseSession {
                continuity: BrowserContinuityToken::new("real-context"),
            })
            .unwrap();

        let (unchanged, rejection) = paused
            .current
            .apply(&ResumeSession {
                continuity: BrowserContinuityToken::new("a-different-fresh-context"),
            })
            .unwrap_err();

        assert_eq!(rejection, AuthSessionTransitionRejected::ContinuityMismatch);
        // Fail closed: still Paused, with the ORIGINAL continuity token —
        // never silently swapped to the mismatched one, never advanced to
        // Active.
        assert!(matches!(unchanged.state(), AuthSessionState::Paused { .. }));
        if let AuthSessionState::Paused { continuity, .. } = unchanged.state() {
            assert_eq!(continuity.as_str(), "real-context");
        }
    }

    #[test]
    fn resume_of_a_never_paused_session_fails_closed() {
        let current = CurrentState::new(AuthSessionId::new(), active("https://example.test"));
        let (unchanged, rejection) = current
            .apply(&ResumeSession {
                continuity: BrowserContinuityToken::new("anything"),
            })
            .unwrap_err();
        assert!(matches!(
            rejection,
            AuthSessionTransitionRejected::InvalidFromState {
                attempted: "resume",
                from: "active"
            }
        ));
        assert!(matches!(unchanged.state(), AuthSessionState::Active { .. }));
    }

    #[test]
    fn invalidate_is_terminal_no_transition_leaves_it() {
        let current = CurrentState::new(AuthSessionId::new(), active("https://example.test"));
        let invalidated = current.apply(&InvalidateSession).unwrap();
        assert!(matches!(
            invalidated.current.state(),
            AuthSessionState::Invalidated { .. }
        ));

        // Pause, resume, and a second invalidate all fail from Invalidated.
        let (still_invalid, rejection) = invalidated
            .current
            .clone()
            .apply(&PauseSession {
                continuity: BrowserContinuityToken::new("x"),
            })
            .unwrap_err();
        assert_eq!(rejection, AuthSessionTransitionRejected::Terminal);
        assert!(matches!(
            still_invalid.state(),
            AuthSessionState::Invalidated { .. }
        ));

        let (_, rejection) = invalidated
            .current
            .clone()
            .apply(&ResumeSession {
                continuity: BrowserContinuityToken::new("x"),
            })
            .unwrap_err();
        assert_eq!(rejection, AuthSessionTransitionRejected::Terminal);

        let (_, rejection) = invalidated.current.apply(&InvalidateSession).unwrap_err();
        assert_eq!(rejection, AuthSessionTransitionRejected::Terminal);
    }

    #[test]
    fn invalidate_from_paused_is_allowed() {
        let current = CurrentState::new(AuthSessionId::new(), active("https://example.test"));
        let paused = current
            .apply(&PauseSession {
                continuity: BrowserContinuityToken::new("x"),
            })
            .unwrap();
        let invalidated = paused.current.apply(&InvalidateSession).unwrap();
        assert!(matches!(
            invalidated.current.state(),
            AuthSessionState::Invalidated { .. }
        ));
    }

    #[test]
    fn origin_and_profile_survive_every_transition() {
        let current = CurrentState::new(
            AuthSessionId::new(),
            AuthSessionState::Active {
                origin: "https://example.test".to_string(),
                profile: AuthenticationProfile::OAuth,
            },
        );
        let paused = current
            .apply(&PauseSession {
                continuity: BrowserContinuityToken::new("x"),
            })
            .unwrap();
        assert_eq!(paused.current.state().origin(), "https://example.test");
        assert_eq!(
            paused.current.state().profile(),
            AuthenticationProfile::OAuth
        );

        let resumed = paused
            .current
            .apply(&ResumeSession {
                continuity: BrowserContinuityToken::new("x"),
            })
            .unwrap();
        assert_eq!(resumed.current.state().origin(), "https://example.test");
        assert_eq!(
            resumed.current.state().profile(),
            AuthenticationProfile::OAuth
        );
    }

    #[cfg(all(feature = "disk", feature = "serde"))]
    mod ledger {
        use super::*;
        use crate::features::domain_persistence::DomainPersistence;

        #[tokio::test]
        async fn create_session_persists_active_state() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let (id, state) = create_session(
                &store,
                "https://example.test".to_string(),
                AuthenticationProfile::InteractiveBrowser,
            )
            .await
            .unwrap();
            assert!(matches!(state, AuthSessionState::Active { .. }));

            let (revision, read_back) = read_current_session(&store, id).await.unwrap().unwrap();
            assert_eq!(revision, 1);
            assert_eq!(read_back.origin(), "https://example.test");
            assert_eq!(
                read_back.profile(),
                AuthenticationProfile::InteractiveBrowser
            );
        }

        #[tokio::test]
        async fn full_pause_resume_invalidate_lifecycle_persists_and_reads_back_truthfully() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let (id, _) = create_session(
                &store,
                "https://example.test".to_string(),
                AuthenticationProfile::InteractiveBrowser,
            )
            .await
            .unwrap();

            let token = BrowserContinuityToken::new("same-browser-context-hash");

            let paused = apply_session_transition(
                &store,
                id,
                &PauseSession {
                    continuity: token.clone(),
                },
            )
            .await
            .unwrap();
            assert!(matches!(paused, AuthSessionState::Paused { .. }));

            let resumed =
                apply_session_transition(&store, id, &ResumeSession { continuity: token })
                    .await
                    .unwrap();
            assert!(matches!(resumed, AuthSessionState::Active { .. }));

            let invalidated = apply_session_transition(&store, id, &InvalidateSession)
                .await
                .unwrap();
            assert!(matches!(invalidated, AuthSessionState::Invalidated { .. }));

            // Current state reads back exactly the last transition's result.
            let (_, current) = read_current_session(&store, id).await.unwrap().unwrap();
            assert!(matches!(current, AuthSessionState::Invalidated { .. }));

            // Every superseded state was appended to history, in order:
            // Active (superseded by pause), Paused (superseded by resume),
            // Active (superseded by invalidate).
            let history = store.read_history(&id.to_string()).await.unwrap();
            assert_eq!(history.len(), 3);
            let kinds: Vec<&str> = history
                .iter()
                .map(|(_, bytes, _)| {
                    let state: AuthSessionState = serde_json::from_slice(bytes).unwrap();
                    match state {
                        AuthSessionState::Active { .. } => "active",
                        AuthSessionState::Paused { .. } => "paused",
                        AuthSessionState::Invalidated { .. } => "invalidated",
                    }
                })
                .collect();
            assert_eq!(kinds, vec!["active", "paused", "active"]);
        }

        #[tokio::test]
        async fn invalid_transition_persists_nothing_and_leaves_current_state_unchanged() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let (id, _) = create_session(
                &store,
                "https://example.test".to_string(),
                AuthenticationProfile::CookieSession,
            )
            .await
            .unwrap();

            // Resuming an Active (never-paused) session must fail closed.
            let error = apply_session_transition(
                &store,
                id,
                &ResumeSession {
                    continuity: BrowserContinuityToken::new("anything"),
                },
            )
            .await
            .unwrap_err();
            assert!(matches!(
                error,
                AuthSessionError::TransitionRejected(
                    AuthSessionTransitionRejected::InvalidFromState { .. }
                )
            ));

            // Nothing was written: still revision 1, still Active, and no
            // history was appended.
            let (revision, state) = read_current_session(&store, id).await.unwrap().unwrap();
            assert_eq!(revision, 1);
            assert!(matches!(state, AuthSessionState::Active { .. }));
            assert!(store
                .read_history(&id.to_string())
                .await
                .unwrap()
                .is_empty());
        }

        #[tokio::test]
        async fn resume_continuity_mismatch_persists_nothing() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let (id, _) = create_session(
                &store,
                "https://example.test".to_string(),
                AuthenticationProfile::InteractiveBrowser,
            )
            .await
            .unwrap();
            apply_session_transition(
                &store,
                id,
                &PauseSession {
                    continuity: BrowserContinuityToken::new("real-context"),
                },
            )
            .await
            .unwrap();

            let error = apply_session_transition(
                &store,
                id,
                &ResumeSession {
                    continuity: BrowserContinuityToken::new("fresh-unauthenticated-context"),
                },
            )
            .await
            .unwrap_err();
            assert!(matches!(
                error,
                AuthSessionError::TransitionRejected(
                    AuthSessionTransitionRejected::ContinuityMismatch
                )
            ));

            // Still paused, still revision 2 (from the successful pause
            // only) — the rejected resume wrote nothing.
            let (revision, state) = read_current_session(&store, id).await.unwrap().unwrap();
            assert_eq!(revision, 2);
            assert!(matches!(state, AuthSessionState::Paused { .. }));
        }

        #[tokio::test]
        async fn read_current_session_of_unknown_id_is_none() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            assert!(read_current_session(&store, AuthSessionId::new())
                .await
                .unwrap()
                .is_none());
        }

        #[tokio::test]
        async fn apply_session_transition_on_unknown_id_is_not_found() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let error = apply_session_transition(&store, AuthSessionId::new(), &InvalidateSession)
                .await
                .unwrap_err();
            assert!(matches!(error, AuthSessionError::NotFound));
        }

        #[tokio::test]
        async fn distinct_sessions_do_not_interfere() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let (a, _) = create_session(
                &store,
                "https://a.test".to_string(),
                AuthenticationProfile::FormLogin,
            )
            .await
            .unwrap();
            let (b, _) = create_session(
                &store,
                "https://b.test".to_string(),
                AuthenticationProfile::BasicAuth,
            )
            .await
            .unwrap();
            assert_ne!(a, b);

            apply_session_transition(&store, a, &InvalidateSession)
                .await
                .unwrap();

            let (_, state_a) = read_current_session(&store, a).await.unwrap().unwrap();
            let (_, state_b) = read_current_session(&store, b).await.unwrap().unwrap();
            assert!(matches!(state_a, AuthSessionState::Invalidated { .. }));
            assert!(matches!(state_b, AuthSessionState::Active { .. }));
        }
    }
}
