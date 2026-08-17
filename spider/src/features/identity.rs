//! Canonical identity for persisted Scorpion domain objects.
//!
//! This module is the **one** definition site for every canonical
//! persisted-domain identity type. It owns identity only:
//!
//! - explicit, distinct Rust type per identity kind (never interchangeable)
//! - one deterministic string serialization format per kind
//! - validating parse (rejects anything that isn't that exact format)
//! - value equality/hash/ordering semantics
//!
//! It deliberately owns **none** of the following — those belong to later,
//! separate frontiers building on top of this identity:
//!
//! - persistence (no database, no file, no cache read/write)
//! - state or lifecycle (no status, no transitions, no "current" anything)
//! - the domain object itself (no `Evidence`/`Watch` record type)
//!
//! # Locked provenance
//!
//! - [`EvidenceId`] realizes the `EvidenceId` concept locked in
//!   `SCORPION.md` §3 ("a unit of captured evidence derived from a fetch").
//!   §3 explicitly left representation undefined ("None of the identifier
//!   types below are defined, typed, or implemented at this baseline") —
//!   this module is that representation.
//! - [`WatchId`] realizes the first link of the state-driven capability
//!   chain locked in `SCORPION_SDD.md` §5.2:
//!   `WatchId → WatchDefinition → WatchState → Snapshot → Transition →
//!   Event/Result → persisted updated state`. Per §5.2, WATCH/MONITOR
//!   itself remains **BLOCKED** — `WatchDefinition`/`WatchState`/the state
//!   machine do not exist and must not be added here. This module defines
//!   only the identity that will eventually name a watch, nothing more.
//! - [`AuthSessionId`] realizes the identity half of the authenticated
//!   session lifecycle locked in `SCORPION.md` §5 ("Authenticated
//!   Research") — specifically its rule that "future MFA/interactive
//!   authentication must support pausing and resuming the *same*
//!   authenticated browser session." This type is identity only; it is
//!   **not**, and structurally cannot become, a credential, cookie, or
//!   token — see `features/auth_session.rs` for the lifecycle state that
//!   uses it and the explicit proof that secrets never enter identity.
//!   Distinct from — and never to be confused with — three other,
//!   unrelated existing "session" concepts: `chromiumoxide`'s CDP
//!   `SessionId` (a browser-automation transport concept, re-exported
//!   into `features/frame_context.rs`'s frame-identity chain) and
//!   `spider_mcp`'s `CrawlSession` (in-memory async tool-call progress
//!   tracking, keyed by a plain `String`, with no relation to
//!   authentication). None of those three are redefined, renamed, or
//!   touched by this type.
//!
//! Only these three identity types exist. Do not add `ResearchId`,
//! `CrawlId`, `FetchId`, a bare `SessionId`, `JobId`, `OperationId`, or any
//! other identity "for symmetry" — each new identity type is its own
//! frontier, scoped to a concept that is actually locked and actually
//! needed next.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

/// Number of raw identity bytes backing every canonical identity type in
/// this module (128 bits — the same width as a UUID, though these are not
/// claimed to be RFC 4122 UUIDs).
const ID_BYTES: usize = 16;

/// A canonical identity string failed to parse.
///
/// Carries only what was wrong with the *shape* of the input — never
/// persistence, lookup, or "does this identity exist" concerns, which are
/// out of scope for this module entirely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityParseError {
    /// The input did not start with the expected type prefix (e.g. an
    /// [`EvidenceId`] string passed where a [`WatchId`] was expected).
    WrongPrefix {
        /// The prefix this identity kind requires.
        expected: &'static str,
    },
    /// The input started with the right prefix but the remainder was not
    /// exactly [`ID_BYTES`] `* 2` lowercase hex characters.
    InvalidBody,
}

impl fmt::Display for IdentityParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityParseError::WrongPrefix { expected } => {
                write!(f, "identity string must start with {expected:?}")
            }
            IdentityParseError::InvalidBody => write!(
                f,
                "identity body must be exactly {} lowercase hex characters",
                ID_BYTES * 2
            ),
        }
    }
}

impl std::error::Error for IdentityParseError {}

/// Fill 16 bytes of process-local, non-cryptographic entropy.
///
/// Deliberately dependency-free (no `rand`/`fastrand`, which are only
/// available under optional feature flags elsewhere in this crate) so
/// identity minting works identically regardless of which cargo features
/// are enabled — mirrors the fallback technique already used for WARC
/// record IDs in `utils/warc.rs`. Not suitable for security purposes; it
/// exists only to make two IDs minted in the same process, at the same
/// nanosecond, on the same thread, still distinct (via the monotonic
/// counter).
fn random_bytes() -> [u8; ID_BYTES] {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = ahash::AHasher::default();
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    nanos.hash(&mut hasher);
    let first = hasher.finish();
    first.hash(&mut hasher);
    let second = hasher.finish();

    let mut bytes = [0u8; ID_BYTES];
    bytes[..8].copy_from_slice(&first.to_le_bytes());
    bytes[8..].copy_from_slice(&second.to_le_bytes());
    bytes
}

/// Encode `bytes` as `prefix` followed by lowercase hex — the one
/// canonical serialization routine every identity type in this module
/// delegates to.
fn format_id(prefix: &str, bytes: &[u8; ID_BYTES]) -> String {
    let mut out = String::with_capacity(prefix.len() + ID_BYTES * 2);
    out.push_str(prefix);
    for byte in bytes {
        use fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Validate and decode `input` against `prefix` — the one canonical
/// parse routine every identity type in this module delegates to.
fn parse_id(prefix: &'static str, input: &str) -> Result<[u8; ID_BYTES], IdentityParseError> {
    let body = input
        .strip_prefix(prefix)
        .ok_or(IdentityParseError::WrongPrefix { expected: prefix })?;
    let body = body.as_bytes();
    if body.len() != ID_BYTES * 2 {
        return Err(IdentityParseError::InvalidBody);
    }
    let mut bytes = [0u8; ID_BYTES];
    for (index, pair) in body.chunks_exact(2).enumerate() {
        // Reject anything but lowercase hex — no uppercase, no whitespace,
        // no alternate encodings. Exactly one valid textual form per value.
        let hi = hex_nibble(pair[0]).ok_or(IdentityParseError::InvalidBody)?;
        let lo = hex_nibble(pair[1]).ok_or(IdentityParseError::InvalidBody)?;
        bytes[index] = (hi << 4) | lo;
    }
    Ok(bytes)
}

#[inline]
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Canonical identity for one unit of captured evidence derived from a
/// fetch.
///
/// Realizes the `EvidenceId` concept locked in `SCORPION.md` §3. This type
/// is identity only: it names a (future) evidence record, it does not hold
/// one, store one, or know whether one currently exists.
///
/// Serialized form: `evid_` followed by 32 lowercase hex characters, e.g.
/// `evid_0123456789abcdef0123456789abcdef`. The format is fixed and
/// deterministic — [`EvidenceId::to_string`] always produces it and
/// [`EvidenceId::from_str`] accepts only it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EvidenceId([u8; ID_BYTES]);

impl EvidenceId {
    /// Wire-format prefix. Part of the type's serialization contract.
    pub const PREFIX: &'static str = "evid_";

    /// Mint a fresh, unused-so-far identity. Pure value construction —
    /// this performs no I/O and records nothing anywhere.
    pub fn new() -> Self {
        Self(random_bytes())
    }
}

impl Default for EvidenceId {
    /// Same as [`EvidenceId::new`] — a fresh identity, not a sentinel
    /// "empty" value (this identity kind has none).
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EvidenceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_id(Self::PREFIX, &self.0))
    }
}

impl fmt::Debug for EvidenceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EvidenceId({self})")
    }
}

impl FromStr for EvidenceId {
    type Err = IdentityParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_id(Self::PREFIX, s).map(Self)
    }
}

impl TryFrom<&str> for EvidenceId {
    type Error = IdentityParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl TryFrom<String> for EvidenceId {
    type Error = IdentityParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for EvidenceId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for EvidenceId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// Canonical identity for one state-driven WATCH/MONITOR capability
/// instance.
///
/// Realizes the first link of the capability chain locked in
/// `SCORPION_SDD.md` §5.2 — `WatchId → WatchDefinition → WatchState → ...`.
/// This type is identity only. Everything else in that chain
/// (`WatchDefinition`, `WatchState`, snapshots, transitions, persisted
/// state) is **BLOCKED** per §5.2 and does not exist anywhere in this
/// crate; do not add it here or infer it from this type's presence. A
/// `WatchId` may exist with no watch, no state, and no persistence behind
/// it — minting one performs no I/O and records nothing anywhere.
///
/// Serialized form: `watch_` followed by 32 lowercase hex characters, e.g.
/// `watch_0123456789abcdef0123456789abcdef`. The format is fixed and
/// deterministic — [`WatchId::to_string`] always produces it and
/// [`WatchId::from_str`] accepts only it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WatchId([u8; ID_BYTES]);

impl WatchId {
    /// Wire-format prefix. Part of the type's serialization contract.
    pub const PREFIX: &'static str = "watch_";

    /// Mint a fresh, unused-so-far identity. Pure value construction —
    /// this performs no I/O and records nothing anywhere.
    pub fn new() -> Self {
        Self(random_bytes())
    }
}

impl Default for WatchId {
    /// Same as [`WatchId::new`] — a fresh identity, not a sentinel
    /// "empty" value (this identity kind has none).
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_id(Self::PREFIX, &self.0))
    }
}

impl fmt::Debug for WatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WatchId({self})")
    }
}

impl FromStr for WatchId {
    type Err = IdentityParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_id(Self::PREFIX, s).map(Self)
    }
}

impl TryFrom<&str> for WatchId {
    type Error = IdentityParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl TryFrom<String> for WatchId {
    type Error = IdentityParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for WatchId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for WatchId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// Canonical identity for one authenticated-session lifecycle instance.
///
/// Realizes the identity half of `SCORPION.md` §5's "Authenticated
/// Research" — pause/resume/invalidate lifecycle state lives in
/// `features/auth_session.rs`, built on this identity and on
/// [`crate::features::domain_state`]'s transition contract. This type is
/// identity only, and — like every identity in this module — 16 opaque
/// random bytes: it cannot hold a cookie, an `Authorization` value, a
/// token, or any other credential, because there is no field, variant, or
/// constructor through which one could ever be supplied. See
/// `features/auth_session.rs`'s module doc for the full secret/identity
/// separation proof.
///
/// Not to be confused with `chromiumoxide`'s CDP `SessionId` (browser
/// transport identity) or `spider_mcp`'s `CrawlSession` (async tool-call
/// progress tracking) — see this module's own doc comment.
///
/// Serialized form: `auth_` followed by 32 lowercase hex characters, e.g.
/// `auth_0123456789abcdef0123456789abcdef`. The format is fixed and
/// deterministic — [`AuthSessionId::to_string`] always produces it and
/// [`AuthSessionId::from_str`] accepts only it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AuthSessionId([u8; ID_BYTES]);

impl AuthSessionId {
    /// Wire-format prefix. Part of the type's serialization contract.
    pub const PREFIX: &'static str = "auth_";

    /// Mint a fresh, unused-so-far identity. Pure value construction —
    /// this performs no I/O and records nothing anywhere.
    pub fn new() -> Self {
        Self(random_bytes())
    }
}

impl Default for AuthSessionId {
    /// Same as [`AuthSessionId::new`] — a fresh identity, not a sentinel
    /// "empty" value (this identity kind has none).
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AuthSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_id(Self::PREFIX, &self.0))
    }
}

impl fmt::Debug for AuthSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AuthSessionId({self})")
    }
}

impl FromStr for AuthSessionId {
    type Err = IdentityParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_id(Self::PREFIX, s).map(Self)
    }
}

impl TryFrom<&str> for AuthSessionId {
    type Error = IdentityParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl TryFrom<String> for AuthSessionId {
    type Error = IdentityParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for AuthSessionId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for AuthSessionId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn evidence_id_round_trips_through_display_and_parse() {
        let id = EvidenceId::new();
        let text = id.to_string();
        assert!(text.starts_with(EvidenceId::PREFIX));
        assert_eq!(text.len(), EvidenceId::PREFIX.len() + ID_BYTES * 2);
        let parsed: EvidenceId = text.parse().expect("round trip must parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn watch_id_round_trips_through_display_and_parse() {
        let id = WatchId::new();
        let text = id.to_string();
        assert!(text.starts_with(WatchId::PREFIX));
        assert_eq!(text.len(), WatchId::PREFIX.len() + ID_BYTES * 2);
        let parsed: WatchId = text.parse().expect("round trip must parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn evidence_id_and_watch_id_are_distinct_types_with_distinct_prefixes() {
        assert_ne!(EvidenceId::PREFIX, WatchId::PREFIX);
        // An EvidenceId's serialized form must never parse as a WatchId,
        // and vice versa — the prefix is a hard type boundary on the wire,
        // not just in Rust's type system.
        let evidence = EvidenceId::new().to_string();
        assert!(matches!(
            WatchId::from_str(&evidence),
            Err(IdentityParseError::WrongPrefix { .. })
        ));
        let watch = WatchId::new().to_string();
        assert!(matches!(
            EvidenceId::from_str(&watch),
            Err(IdentityParseError::WrongPrefix { .. })
        ));
    }

    #[test]
    fn auth_session_id_round_trips_through_display_and_parse() {
        let id = AuthSessionId::new();
        let text = id.to_string();
        assert!(text.starts_with(AuthSessionId::PREFIX));
        assert_eq!(text.len(), AuthSessionId::PREFIX.len() + ID_BYTES * 2);
        let parsed: AuthSessionId = text.parse().expect("round trip must parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn auth_session_id_is_distinct_from_evidence_id_and_watch_id() {
        assert_ne!(AuthSessionId::PREFIX, EvidenceId::PREFIX);
        assert_ne!(AuthSessionId::PREFIX, WatchId::PREFIX);
        let auth = AuthSessionId::new().to_string();
        assert!(matches!(
            EvidenceId::from_str(&auth),
            Err(IdentityParseError::WrongPrefix { .. })
        ));
        assert!(matches!(
            WatchId::from_str(&auth),
            Err(IdentityParseError::WrongPrefix { .. })
        ));
    }

    #[test]
    fn parse_rejects_malformed_bodies() {
        for bad in [
            "evid_",                                   // empty body
            "evid_short",                              // too short, non-hex
            "evid_0123456789abcdef0123456789abcdef00", // too long
            "evid_0123456789ABCDEF0123456789abcdef",   // uppercase hex
            "evid_0123456789abcdef0123456789abcde!",   // non-hex char
            " evid_0123456789abcdef0123456789abcdef",  // leading whitespace
            "watch_0123456789abcdef0123456789abcdef",  // wrong prefix
            "0123456789abcdef0123456789abcdef",        // missing prefix
        ] {
            assert!(
                EvidenceId::from_str(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn equality_and_hash_are_value_based() {
        let a = EvidenceId::new();
        let b: EvidenceId = a.to_string().parse().unwrap();
        assert_eq!(a, b, "equal serialized forms must compare equal");

        let mut set = HashSet::new();
        set.insert(a);
        assert!(
            set.contains(&b),
            "a value-equal EvidenceId must hash to the same bucket"
        );
    }

    #[test]
    fn generated_ids_are_pairwise_distinct() {
        let mut seen = HashSet::new();
        for _ in 0..256 {
            assert!(
                seen.insert(EvidenceId::new()),
                "collision in freshly minted IDs"
            );
        }
        let mut seen = HashSet::new();
        for _ in 0..256 {
            assert!(
                seen.insert(WatchId::new()),
                "collision in freshly minted IDs"
            );
        }
        let mut seen = HashSet::new();
        for _ in 0..256 {
            assert!(
                seen.insert(AuthSessionId::new()),
                "collision in freshly minted IDs"
            );
        }
    }

    #[test]
    fn debug_format_is_type_qualified() {
        let id = EvidenceId::new();
        let debug = format!("{id:?}");
        assert!(debug.starts_with("EvidenceId(evid_"));
    }

    #[test]
    fn try_from_str_and_string_agree_with_from_str() {
        let text = EvidenceId::new().to_string();
        let via_from_str: EvidenceId = text.parse().unwrap();
        let via_try_from_str: EvidenceId = EvidenceId::try_from(text.as_str()).unwrap();
        let via_try_from_string: EvidenceId = EvidenceId::try_from(text.clone()).unwrap();
        assert_eq!(via_from_str, via_try_from_str);
        assert_eq!(via_from_str, via_try_from_string);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_round_trips_through_the_same_canonical_string() {
        let id = EvidenceId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("{:?}", id.to_string()));
        let back: EvidenceId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
