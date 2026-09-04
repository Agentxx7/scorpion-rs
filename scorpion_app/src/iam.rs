//! IAM Callback Inspector — canonical trace creation, generic callback
//! reception, and durable readback.
//!
//! `SCORPION_IAM_CALLBACK_RECEIVER_AND_PERSISTENCE_001`, the first real
//! runtime slice of the owner-decided IAM Callback Inspector
//! (`SCORPION_ARCHITECTURE.md` §3.8). This module owns HTTP-adjacent
//! request/response translation and generic callback parsing only — it
//! performs no OIDC/JWT decoding, no SAML XML parsing, and no
//! cryptographic signature verification. The trace/observation/redaction
//! *model* it drives (`IamTraceId`, `IamTraceState`, `ReceiveCallback`,
//! `IamCallbackObservation`, `IamFact`, `IamFactStatus`, `redact`) already
//! exists, unmodified, in `spider::features::iam_trace`/`identity`
//! (`SCORPION_CANONICAL_IAM_TRACE_AND_OBSERVATION_MODEL_001`).
//!
//! # Persistence: reused, not duplicated
//!
//! This module resolves the canonical shared store fresh on every call,
//! through [`open_shared_domain_store`] with no explicit path and real
//! process environment — exactly `scorpion_app::audit`/`evidence`/
//! `fetch`'s own decision, for the same reason: this is one more
//! independent tool sharing one canonical store, not a server-owned
//! eagerly-opened handle. No second database, no second environment
//! variable — `SCORPION_DOMAIN_DB`/`RESEARCH_EVIDENCE_DB` resolution is
//! entirely `domain_runtime`'s, never re-typed here (see
//! `architecture_guardrails.rs`'s
//! `domain_runtime_seam_owns_database_resolution_not_a_local_literal`).
//!
//! # Persistence ordering and atomicity (the honest accounting)
//!
//! [`DomainPersistence::write_current`] and
//! [`DomainPersistence::append_history`] are two independent SQL
//! transactions — nothing in `DomainPersistence` spans both in one atomic
//! unit, and this module does not invent a transaction abstraction to
//! pretend otherwise. On each callback:
//!
//! 1. The trace's existence is confirmed via `read_current` first — a
//!    callback to a nonexistent trace never reaches persistence at all.
//! 2. The observation is **appended to history first**, at a revision
//!    computed by actually counting this trace's existing durable history
//!    (never a process-local counter — see [`append_observation`]) and
//!    retried on a `HistoryAlreadyExists` race. This ordering is
//!    deliberate: the observation *is* the evidence a real IAM
//!    troubleshooting session cares about, so it is made durable before
//!    anything else is attempted.
//! 3. The `AwaitingCallback -> Received` transition is then attempted —
//!    **on every callback, not only the first** — via a fresh
//!    `read_current`/`CurrentState::apply`/`write_current` compare-and-swap
//!    (see [`try_mark_received`]). This step is best-effort and, once a
//!    trace is genuinely `Received`, a true no-op (`ReceiveCallback`
//!    rejects re-application; the rejection is discarded). The
//!    observation from step 2 is always safely durable regardless of
//!    whether this step succeeds — but the earlier version of this
//!    module gated this attempt on "only when this observation's own
//!    revision is `1`", which was a real correctness gap, not a harmless
//!    simplification: if that one attempt failed (process crash, a
//!    transient persistence error) between steps 2 and 3, no later
//!    callback would ever retry it, because no later observation is ever
//!    revision `1` again — the trace could stay permanently
//!    `AwaitingCallback` forever despite real, correctly-recorded
//!    observations accumulating underneath it. Attempting this step
//!    unconditionally on every callback closes that gap: any subsequent
//!    callback repairs a trace still stuck `AwaitingCallback`, so the
//!    "harmless to lose" claim about this step is true *because* of that
//!    retry, not despite skipping it.
//! 4. A second (or later) observation under an already-`Received` trace
//!    never *needs* the transition to do anything (see step 3) — it is
//!    simply appended at the next revision, per `features/iam_trace.rs`'s
//!    own documented lifecycle.

use serde::Serialize;
use spider::features::domain_persistence::{DomainPersistence, PersistenceError};
use spider::features::domain_runtime::{open_shared_domain_store, DomainRuntimeError};
use spider::features::domain_state::CurrentState;
use spider::features::iam_trace::{
    IamCallbackObservation, IamFact, IamFactStatus, IamProtocolClassification, IamTraceState,
    ReceiveCallback,
};
use spider::features::identity::IamTraceId;
use spider::url;
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum number of facts (query/form parameters, or flattened JSON
/// fields) extracted from one callback. Conservative but generous enough
/// for real OAuth2/OIDC/SAML callback shapes, which carry a handful of
/// top-level fields, not dozens.
const IAM_MAX_FACTS: usize = 64;
/// Maximum bytes for one fact's name.
const IAM_MAX_KEY_BYTES: usize = 256;
/// Maximum bytes for one fact's value before redaction/observation —
/// generous enough for a base64-encoded SAMLResponse or a large JWT while
/// still bounded; the overall request body is already bounded far below
/// this by `main.rs`'s own `MAX_BODY_BYTES` (64 KiB).
const IAM_MAX_VALUE_BYTES: usize = 16 * 1024;
/// Maximum JSON object nesting depth this module will flatten. Bounds
/// recursion against a maliciously deep JSON body.
const IAM_MAX_JSON_DEPTH: usize = 8;
/// How many times [`append_observation`] retries after losing a
/// concurrent-append race before failing closed. Concurrent callbacks to
/// the same trace are not an expected access pattern for a single-operator
/// diagnostic tool; this bound exists only so a pathological race cannot
/// loop forever.
const IAM_MAX_REVISION_RETRIES: usize = 5;

/// Parameter/header names redacted regardless of where they appear (query,
/// form, or JSON — including nested JSON object fields), matched
/// case-insensitively. This is the one place this module's redaction
/// *policy* (which names are sensitive) lives — `spider::features::
/// iam_trace::redact` itself has no name-detection opinion at all.
const SENSITIVE_NAMES: &[&str] = &[
    "code",
    "access_token",
    "refresh_token",
    "id_token",
    "client_secret",
    "password",
    "authorization",
    "cookie",
    "set-cookie",
    "samlresponse",
    "samlrequest",
];

fn is_sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SENSITIVE_NAMES.contains(&lower.as_str())
}

/// Public IAM Callback Inspector error vocabulary. Every variant maps to a
/// truthful HTTP status ([`iam_error_status`]) and a sanitized JSON body
/// ([`iam_error_json`]) that never echoes request content.
#[derive(Debug)]
pub enum IamError {
    /// The supplied string is not a well-formed [`IamTraceId`].
    InvalidTraceId,
    /// The trace id parsed, but no trace has ever been created for it.
    NotFound,
    /// No canonical domain database is configured for this process.
    NotConfigured,
    /// The canonical store is configured but could not be opened.
    Unavailable,
    /// A POST callback's `Content-Type` was missing or is not one of the
    /// two this frontier supports
    /// (`application/x-www-form-urlencoded`, `application/json`).
    UnsupportedContentType,
    /// A query string or form body contained a `%` not followed by
    /// exactly two hex digits.
    MalformedPercentEncoding,
    /// A form body was not valid UTF-8 once percent-decoded.
    MalformedFormEncoding,
    /// A JSON body did not parse as JSON at all.
    MalformedJson,
    /// Too many facts, an over-long name, or an over-long value — see
    /// this module's `IAM_MAX_*` constants.
    ParameterLimitExceeded,
    /// The store opened, but a read or write failed.
    PersistenceFailed,
    /// An internal invariant failed (state/observation serialization).
    Internal,
}

impl std::fmt::Display for IamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            IamError::InvalidTraceId => "the supplied trace id is not well-formed",
            IamError::NotFound => "no trace has been created for this id",
            IamError::NotConfigured => "no canonical domain database is configured",
            IamError::Unavailable => "the canonical domain database is unavailable",
            IamError::UnsupportedContentType => {
                "unsupported content type: only application/x-www-form-urlencoded and \
                 application/json are accepted"
            }
            IamError::MalformedPercentEncoding => "malformed percent-encoding",
            IamError::MalformedFormEncoding => "form body is not valid UTF-8",
            IamError::MalformedJson => "request body is not valid JSON",
            IamError::ParameterLimitExceeded => "too many parameters, or one is too large",
            IamError::PersistenceFailed => "a persistence operation failed",
            IamError::Internal => "an internal error occurred",
        };
        f.write_str(message)
    }
}

/// Truthful HTTP status for one [`IamError`].
pub fn iam_error_status(error: &IamError) -> u16 {
    match error {
        IamError::InvalidTraceId
        | IamError::UnsupportedContentType
        | IamError::MalformedPercentEncoding
        | IamError::MalformedFormEncoding
        | IamError::MalformedJson
        | IamError::ParameterLimitExceeded => 400,
        IamError::NotFound => 404,
        IamError::NotConfigured | IamError::Unavailable => 503,
        IamError::PersistenceFailed | IamError::Internal => 500,
    }
}

/// Serialize a public IAM error without leaking persistence diagnostics
/// or any request content (query/form/JSON values are never included).
pub fn iam_error_json(error: &IamError) -> String {
    let code = match error {
        IamError::InvalidTraceId => "invalid_trace_id",
        IamError::NotFound => "trace_not_found",
        IamError::NotConfigured => "iam_store_not_configured",
        IamError::Unavailable => "iam_store_unavailable",
        IamError::UnsupportedContentType => "unsupported_content_type",
        IamError::MalformedPercentEncoding => "malformed_percent_encoding",
        IamError::MalformedFormEncoding => "malformed_form_encoding",
        IamError::MalformedJson => "malformed_json",
        IamError::ParameterLimitExceeded => "parameter_limit_exceeded",
        IamError::PersistenceFailed => "iam_persistence_failed",
        IamError::Internal => "internal_error",
    };
    serde_json::json!({"error": {"code": code, "message": error.to_string()}}).to_string()
}

fn map_persistence_error(error: PersistenceError) -> IamError {
    eprintln!("scorpion-api iam: persistence error: {error}");
    IamError::PersistenceFailed
}

async fn open_store(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<DomainPersistence, IamError> {
    open_shared_domain_store(None, lookup)
        .await
        .map_err(|error| match error {
            DomainRuntimeError::NotConfigured(_) => IamError::NotConfigured,
            DomainRuntimeError::Persistence(internal) => {
                eprintln!("scorpion-api iam: domain store open failed: {internal}");
                IamError::Unavailable
            }
        })
}

fn serialize_state(state: IamTraceState) -> Result<Vec<u8>, IamError> {
    serde_json::to_vec(&state).map_err(|_| IamError::Internal)
}

fn deserialize_state(bytes: &[u8]) -> Result<IamTraceState, IamError> {
    serde_json::from_slice(bytes).map_err(|_| IamError::Internal)
}

fn state_label(state: IamTraceState) -> &'static str {
    match state {
        IamTraceState::AwaitingCallback => "awaiting_callback",
        IamTraceState::Received => "received",
    }
}

// ---------------------------------------------------------------------
// Create trace
// ---------------------------------------------------------------------

/// Public wire shape of a newly created trace.
#[derive(Debug, Serialize)]
pub struct CreateTraceResponse {
    pub trace_id: String,
    pub callback_uri: String,
    pub state: &'static str,
}

pub async fn create_trace() -> Result<CreateTraceResponse, IamError> {
    create_trace_with_environment(&|name| std::env::var(name).ok()).await
}

/// Real implementation, parameterized over environment lookup — mirrors
/// `scorpion_app::audit`/`evidence`/`fetch`'s own `run`/`run_with_
/// environment` split, so tests can deterministically exercise every
/// configured/unconfigured/misconfigured store shape without mutating
/// real process environment.
async fn create_trace_with_environment(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<CreateTraceResponse, IamError> {
    let store = open_store(lookup).await?;
    let id = IamTraceId::new();
    let state_bytes = serialize_state(IamTraceState::AwaitingCallback)?;
    // `expected_revision: None` means "no row must exist yet" — true for
    // a freshly minted identity; a conflict here would mean a genuine
    // identity collision, reported truthfully rather than silently
    // overwritten.
    store
        .write_current(&id.to_string(), None, &state_bytes)
        .await
        .map_err(map_persistence_error)?;

    // The callback URI must point back to this same scorpion-api
    // instance — reusing the identical `SCORPION_API_BIND` variable
    // `main.rs` binds its own listener to (same source of truth, no
    // drift risk) rather than a second, independently configured value.
    let bind = lookup("SCORPION_API_BIND").unwrap_or_else(|| "127.0.0.1:8787".to_string());
    Ok(CreateTraceResponse {
        trace_id: id.to_string(),
        callback_uri: format!("http://{bind}/iam/callback/{id}"),
        state: state_label(IamTraceState::AwaitingCallback),
    })
}

// ---------------------------------------------------------------------
// Receive callback
// ---------------------------------------------------------------------

/// Outcome of successfully receiving one callback.
#[derive(Debug)]
pub struct ReceiveCallbackOutcome {
    pub trace_id: String,
}

pub async fn receive_callback(
    trace_id_raw: &str,
    method: &str,
    query: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<ReceiveCallbackOutcome, IamError> {
    receive_callback_with_environment(trace_id_raw, method, query, content_type, body, &|name| {
        std::env::var(name).ok()
    })
    .await
}

async fn receive_callback_with_environment(
    trace_id_raw: &str,
    method: &str,
    query: &str,
    content_type: Option<&str>,
    body: &[u8],
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<ReceiveCallbackOutcome, IamError> {
    let trace_id: IamTraceId = trace_id_raw
        .trim()
        .parse()
        .map_err(|_| IamError::InvalidTraceId)?;
    let store = open_store(lookup).await?;
    let key = trace_id.to_string();

    // 1. Verify the trace exists before doing any parsing work at all.
    if store
        .read_current(&key)
        .await
        .map_err(map_persistence_error)?
        .is_none()
    {
        return Err(IamError::NotFound);
    }

    // 2. Parse, classify (OAuth2/OIDC/Generic — see this module's own
    // classification doc for the exact precedence), and redact.
    let (protocol, facts) = match method {
        "GET" => parse_query_facts(query)?,
        "POST" => parse_post_facts(content_type, body)?,
        _ => return Err(IamError::UnsupportedContentType),
    };

    let target = format!("/iam/callback/{trace_id}");
    let observed_at_unix_ms = now_unix_ms();
    let observation = IamCallbackObservation::new(
        trace_id,
        method,
        target,
        observed_at_unix_ms,
        protocol,
        facts,
    );

    // 3. Durable proof first — see this module's doc comment for the
    // full ordering/atomicity accounting.
    let _revision = append_observation(&store, &trace_id, &observation).await?;

    // 4. Attempt the lifecycle transition on *every* callback, not only
    // when `revision == 1`. This is deliberate self-healing, added after
    // a real consistency gap was pointed out: gating this on `revision ==
    // 1` alone meant that if the transition attempt for a trace's very
    // first observation failed (process crash, transient persistence
    // error) between steps 3 and 4, no later callback would ever retry
    // it — `revision` would never be `1` again for that trace, so the
    // trace could stay permanently `AwaitingCallback` forever despite
    // real, correctly-recorded observations existing. Calling
    // `try_mark_received` unconditionally instead costs one extra
    // read/write on every callback after the first (cheap, and harmless
    // since it is a no-op once already `Received`), and means any
    // callback — not just the first — repairs a trace stuck in that
    // window. See `try_mark_received`'s own doc for what "harmless" now
    // actually means: it is true because of this unconditional retry,
    // not despite it.
    try_mark_received(&store, &trace_id).await;

    Ok(ReceiveCallbackOutcome {
        trace_id: trace_id.to_string(),
    })
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Append `observation` under `trace_id` at the next revision, determined
/// by actually reading this trace's durable history — never a
/// process-local counter, so this is correct even across restarts and
/// concurrent processes. Retries on a `HistoryAlreadyExists` race (another
/// concurrent callback to the same trace claimed that revision first).
async fn append_observation(
    store: &DomainPersistence,
    trace_id: &IamTraceId,
    observation: &IamCallbackObservation,
) -> Result<u64, IamError> {
    let key = trace_id.to_string();
    let payload = serde_json::to_vec(observation).map_err(|_| IamError::Internal)?;
    for _ in 0..IAM_MAX_REVISION_RETRIES {
        let history = store
            .read_history(&key)
            .await
            .map_err(map_persistence_error)?;
        let next_revision = history.len() as u64 + 1;
        match store
            .append_history(&key, next_revision, &payload, SystemTime::now())
            .await
        {
            Ok(()) => return Ok(next_revision),
            Err(PersistenceError::HistoryAlreadyExists) => continue,
            Err(other) => return Err(map_persistence_error(other)),
        }
    }
    eprintln!(
        "scorpion-api iam: exhausted {IAM_MAX_REVISION_RETRIES} retries appending an \
         observation (concurrent-append race never resolved)"
    );
    Err(IamError::PersistenceFailed)
}

/// Best-effort `AwaitingCallback -> Received` transition, called
/// unconditionally on *every* callback (see the caller's comment for why
/// gating this on "only the first observation" was itself the bug). Never
/// returns an error: losing a compare-and-swap race, or the transition
/// being rejected because the trace is already `Received`, is a true no-op
/// — the calling observation is already durably recorded regardless, and
/// the very next callback (if any) will retry this exact step again, so a
/// trace can never get permanently stuck `AwaitingCallback` while real
/// observations accumulate underneath it.
async fn try_mark_received(store: &DomainPersistence, trace_id: &IamTraceId) {
    let key = trace_id.to_string();
    let Ok(Some((current_revision, state_bytes))) = store.read_current(&key).await else {
        return;
    };
    let Ok(current_state) = deserialize_state(&state_bytes) else {
        return;
    };
    let Ok(applied) = CurrentState::new(*trace_id, current_state).apply(&ReceiveCallback) else {
        return;
    };
    let Ok(new_bytes) = serialize_state(*applied.current.state()) else {
        return;
    };
    // A `CurrentStateConflict` here means another concurrent request
    // already recorded this exact transition — harmless, nothing to do.
    let _ = store
        .write_current(&key, Some(current_revision), &new_bytes)
        .await;
}

// ---------------------------------------------------------------------
// Generic callback parsing (GET query / POST form / POST JSON), OAuth2/
// OIDC classification, and bounded, decode-only id_token (JWT) inspection.
//
// `SCORPION_IAM_OIDC_AND_JWT_INSPECTION_001`. Classification derives only
// from parameter/field *names* actually observed in this callback — never
// from the route being an IAM route, and never from prior calls. Per the
// documented precedence, `id_token` presence classifies `Oidc` even when
// `code`/`state` also appear; otherwise `code`/`error`/`state` classify
// `OAuth2`; otherwise `Generic`. `error`/`error_description` need no
// special-casing beyond this: they are ordinary, bounded, never-rendered-
// as-HTML Observed facts like any other unrecognized name, and an
// upstream OAuth error never becomes a Scorpion-side failure — Scorpion's
// job is to observe the callback, not judge the flow it came from.
// ---------------------------------------------------------------------

fn parse_query_facts(query: &str) -> Result<(IamProtocolClassification, Vec<IamFact>), IamError> {
    validate_percent_encoding(query)?;
    let mut facts = facts_from_pairs(url::form_urlencoded::parse(query.as_bytes()))?;
    let raw_id_token = url::form_urlencoded::parse(query.as_bytes())
        .find(|(name, _)| name == "id_token")
        .map(|(_, value)| value.into_owned());
    let protocol = finalize_oauth_oidc_facts(&mut facts, raw_id_token)?;
    Ok((protocol, facts))
}

fn parse_post_facts(
    content_type: Option<&str>,
    body: &[u8],
) -> Result<(IamProtocolClassification, Vec<IamFact>), IamError> {
    let media_type = content_type
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let (mut facts, raw_id_token) = match media_type.as_str() {
        "application/x-www-form-urlencoded" => {
            let raw = std::str::from_utf8(body).map_err(|_| IamError::MalformedFormEncoding)?;
            validate_percent_encoding(raw)?;
            let facts = facts_from_pairs(url::form_urlencoded::parse(body))?;
            let raw_id_token = url::form_urlencoded::parse(body)
                .find(|(name, _)| name == "id_token")
                .map(|(_, value)| value.into_owned());
            (facts, raw_id_token)
        }
        "application/json" => {
            let value: serde_json::Value =
                serde_json::from_slice(body).map_err(|_| IamError::MalformedJson)?;
            let mut facts = Vec::new();
            flatten_json_facts(&value, "", 0, &mut facts)?;
            // Only a top-level string `id_token` field is treated as a
            // JWT to decode — this matches every real OIDC callback
            // shape and avoids guessing at a nested field's meaning.
            let raw_id_token = match &value {
                serde_json::Value::Object(map) => map
                    .get("id_token")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                _ => None,
            };
            (facts, raw_id_token)
        }
        _ => return Err(IamError::UnsupportedContentType),
    };
    let protocol = finalize_oauth_oidc_facts(&mut facts, raw_id_token)?;
    Ok((protocol, facts))
}

/// Classify this callback's protocol from the fact *names* already
/// present (survive redaction — see [`push_fact`]'s own doc), mark the
/// generic `state` fact `NotValidated` when it is playing its OAuth2/OIDC
/// correlation role, and append bounded, decode-only `id_token` (JWT)
/// facts when one was observed. Mutates `facts` in place; returns the
/// classification.
fn finalize_oauth_oidc_facts(
    facts: &mut Vec<IamFact>,
    raw_id_token: Option<String>,
) -> Result<IamProtocolClassification, IamError> {
    let has_code = facts.iter().any(|fact| fact.name == "code");
    let has_error = facts.iter().any(|fact| fact.name == "error");
    let has_state = facts.iter().any(|fact| fact.name == "state");
    let protocol = classify_protocol(raw_id_token.is_some(), has_code, has_error, has_state);
    if matches!(
        protocol,
        IamProtocolClassification::OAuth2 | IamProtocolClassification::Oidc
    ) {
        mark_state_not_validated(facts);
    }
    if let Some(raw_jwt) = raw_id_token {
        append_id_token_facts(facts, &raw_jwt)?;
    }
    Ok(protocol)
}

/// Documented classification precedence: `id_token` presence always wins
/// (an OIDC callback carrying `code` too is still `Oidc`, never
/// downgraded to `OAuth2`); otherwise any of `code`/`error`/`state`
/// classifies `OAuth2`; otherwise `Generic`. Never derives from the route
/// itself — only from these observed names.
fn classify_protocol(
    has_id_token: bool,
    has_code: bool,
    has_error: bool,
    has_state: bool,
) -> IamProtocolClassification {
    if has_id_token {
        IamProtocolClassification::Oidc
    } else if has_code || has_error || has_state {
        IamProtocolClassification::OAuth2
    } else {
        IamProtocolClassification::Generic
    }
}

/// `state` has no expectation-storage model to compare against yet —
/// trace creation accepts no input and `IamTraceState` carries no payload
/// (audited, not invented, by this frontier; see its own closure report's
/// "state/nonce finding"). Relabeling it `NotValidated` uses only the
/// existing truth vocabulary's own "no expectation: NOT_VALIDATED" case —
/// no new field, no new storage — so a real OAuth2/OIDC `state` parameter
/// is never presented as if Scorpion had verified it just because the
/// parameter exists.
fn mark_state_not_validated(facts: &mut [IamFact]) {
    for fact in facts.iter_mut() {
        if fact.name == "state" {
            if let IamFactStatus::Observed { value } = &fact.status {
                fact.status = IamFactStatus::NotValidated {
                    value: value.clone(),
                };
            }
        }
    }
}

/// Maximum raw compact-JWT bytes this module will attempt to decode.
/// Reuses the existing per-value callback bound: in practice a
/// `raw_jwt` longer than this already failed the generic per-parameter
/// bound (`IAM_MAX_VALUE_BYTES`) before reaching here, since `id_token`'s
/// own value is checked there too — this constant exists as an explicit,
/// independently-documented, directly-testable bound rather than relying
/// on that as an implicit side effect.
const JWT_MAX_RAW_BYTES: usize = IAM_MAX_VALUE_BYTES;
/// Maximum decoded (post-base64url) header bytes. Real JWT headers are a
/// handful of short fields (`alg`/`typ`/`kid`); this is generous while
/// still bounding memory from an attacker-controlled decode.
const JWT_MAX_DECODED_HEADER_BYTES: usize = 4 * 1024;
/// Maximum decoded (post-base64url) payload bytes. Deliberately smaller
/// than [`JWT_MAX_RAW_BYTES`] by more than base64's ~4/3 expansion factor
/// — otherwise this bound could never actually be reached (any decoded
/// payload large enough to exceed an *equal* raw-byte bound would already
/// have failed the raw-length check first, on its larger base64 form).
const JWT_MAX_DECODED_PAYLOAD_BYTES: usize = 8 * 1024;

/// Allowed JWT header claim names — see this frontier's own scope: no
/// other header field is ever surfaced.
const JWT_HEADER_FACTS: &[&str] = &["alg", "typ", "kid"];
/// Allowed JWT payload claim names, minus `nonce` (handled separately —
/// see [`finalize_oauth_oidc_facts`]'s sibling reasoning for `state`). No
/// other claim (`sub`, `email`, `name`, `groups`, `roles`, or any other
/// custom claim) is ever surfaced — that requires a future, separate
/// privacy/scope decision.
const JWT_PAYLOAD_FACTS: &[&str] = &["iss", "aud", "exp", "iat", "nbf"];

/// Append this `id_token`'s derived facts (or, on any decode failure, one
/// truthful diagnostic fact) to `facts`. Never returns an error for a
/// malformed/oversized JWT itself — per this frontier's explicit
/// requirement, a callback carrying an unparseable `id_token` must still
/// be durably recorded, `id_token` remains `Redacted` (handled entirely
/// by the pre-existing generic redaction path, unrelated to this
/// function), and `protocol` stays `Oidc` regardless. The only error this
/// can return is the pre-existing global per-callback fact-count bound.
fn append_id_token_facts(facts: &mut Vec<IamFact>, raw_jwt: &str) -> Result<(), IamError> {
    let new_facts = match decode_id_token(raw_jwt) {
        Ok(decoded) => decoded,
        Err(reason) => vec![IamFact::not_validated("jwt.decode_error", reason)],
    };
    if facts.len() + new_facts.len() > IAM_MAX_FACTS {
        return Err(IamError::ParameterLimitExceeded);
    }
    facts.extend(new_facts);
    Ok(())
}

/// Decode-only compact-JWT inspection. Never verifies a signature, never
/// fetches a key, never negotiates on `alg` — `alg` is attacker-controlled
/// observed data (surfaced as `jwt.header.alg`, nothing more) and is never
/// used to select any cryptographic behavior, because none exists here at
/// all. Fails closed and truthfully on any malformed/oversized input;
/// never panics, never partially fabricates a claim it could not actually
/// decode.
fn decode_id_token(raw_jwt: &str) -> Result<Vec<IamFact>, &'static str> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    if raw_jwt.len() > JWT_MAX_RAW_BYTES {
        return Err("oversized_token");
    }
    let segments: Vec<&str> = raw_jwt.split('.').collect();
    if segments.len() != 3 {
        return Err("malformed_segment_count");
    }

    let header_bytes = URL_SAFE_NO_PAD
        .decode(segments[0])
        .map_err(|_| "malformed_base64url_header")?;
    if header_bytes.len() > JWT_MAX_DECODED_HEADER_BYTES {
        return Err("oversized_decoded_header");
    }
    let header: serde_json::Value =
        serde_json::from_slice(&header_bytes).map_err(|_| "malformed_header_json")?;

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(segments[1])
        .map_err(|_| "malformed_base64url_payload")?;
    if payload_bytes.len() > JWT_MAX_DECODED_PAYLOAD_BYTES {
        return Err("oversized_decoded_payload");
    }
    let payload: serde_json::Value =
        serde_json::from_slice(&payload_bytes).map_err(|_| "malformed_payload_json")?;

    let mut facts = Vec::new();
    for key in JWT_HEADER_FACTS {
        if let Some(value) = header.get(*key) {
            facts.push(IamFact::observed(
                format!("jwt.header.{key}"),
                json_scalar_to_string(value),
            ));
        }
    }
    for key in JWT_PAYLOAD_FACTS {
        if let Some(value) = payload.get(*key) {
            facts.push(IamFact::observed(
                format!("jwt.{key}"),
                json_scalar_to_string(value),
            ));
        }
    }
    if let Some(value) = payload.get("nonce") {
        // Same reasoning as `state` above: no expectation-storage model
        // exists yet, so this is NotValidated, never bare Observed —
        // using only the existing truth vocabulary, no new field.
        facts.push(IamFact::not_validated(
            "jwt.nonce",
            json_scalar_to_string(value),
        ));
    }

    // Signature: presence is a plain, truthful, structural observation
    // (the third segment may legitimately be empty for an unsigned
    // `alg: "none"` token). Verification is unconditionally
    // `NotValidated` — no key lookup, no JWKS fetch, no X.509 validation,
    // no cryptographic verification of any kind exists in this frontier,
    // so nothing could ever justify `Validated` here.
    let signature_present = !segments[2].is_empty();
    facts.push(IamFact::observed(
        "jwt.signature.present",
        signature_present.to_string(),
    ));
    facts.push(IamFact::not_validated(
        "jwt.signature.verification",
        "not_attempted",
    ));

    Ok(facts)
}

/// Reject a query/form string containing a `%` not followed by exactly
/// two hex digits — `url::form_urlencoded::parse` decodes lossily and
/// would otherwise silently tolerate this rather than failing truthfully.
fn validate_percent_encoding(raw: &str) -> Result<(), IamError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let valid = bytes
                .get(index + 1..index + 3)
                .is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit));
            if !valid {
                return Err(IamError::MalformedPercentEncoding);
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn facts_from_pairs<'a>(
    pairs: impl Iterator<Item = (std::borrow::Cow<'a, str>, std::borrow::Cow<'a, str>)>,
) -> Result<Vec<IamFact>, IamError> {
    let mut facts = Vec::new();
    for (name, value) in pairs {
        let name = name.into_owned();
        let sensitive = is_sensitive_name(&name);
        push_fact(&mut facts, name, value.into_owned(), sensitive)?;
    }
    Ok(facts)
}

/// Append one fact, applying bounds first. `sensitive` is decided by the
/// caller from the *leaf* field/parameter name — never re-derived from
/// `name` here, since a nested JSON fact's `name` is a dotted path (e.g.
/// `"auth.client_secret"`) that would never match the flat
/// [`SENSITIVE_NAMES`] list even though its leaf key does.
fn push_fact(
    facts: &mut Vec<IamFact>,
    name: String,
    value: String,
    sensitive: bool,
) -> Result<(), IamError> {
    if facts.len() >= IAM_MAX_FACTS {
        return Err(IamError::ParameterLimitExceeded);
    }
    if name.len() > IAM_MAX_KEY_BYTES || value.len() > IAM_MAX_VALUE_BYTES {
        return Err(IamError::ParameterLimitExceeded);
    }
    if sensitive {
        facts.push(IamFact::redacted(name, value));
    } else {
        facts.push(IamFact::observed(name, value));
    }
    Ok(())
}

/// Flatten a JSON body into neutral facts, applying the same name-based
/// redaction recursively to nested object fields. Only objects (and array
/// elements within them) are descended into; a top-level non-object JSON
/// body produces no facts (nothing to name a fact after) rather than an
/// error.
fn flatten_json_facts(
    value: &serde_json::Value,
    prefix: &str,
    depth: usize,
    facts: &mut Vec<IamFact>,
) -> Result<(), IamError> {
    if depth > IAM_MAX_JSON_DEPTH {
        return Err(IamError::ParameterLimitExceeded);
    }
    let serde_json::Value::Object(map) = value else {
        return Ok(());
    };
    for (key, field_value) in map {
        let name = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if is_sensitive_name(key) {
            let plaintext = json_scalar_to_string(field_value);
            push_fact(facts, name, plaintext, true)?;
            continue;
        }
        match field_value {
            serde_json::Value::Object(_) => {
                flatten_json_facts(field_value, &name, depth + 1, facts)?
            }
            serde_json::Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    let item_name = format!("{name}[{index}]");
                    if let serde_json::Value::Object(_) = item {
                        flatten_json_facts(item, &item_name, depth + 1, facts)?;
                    } else {
                        push_fact(facts, item_name, json_scalar_to_string(item), false)?;
                    }
                }
            }
            other => push_fact(facts, name, json_scalar_to_string(other), false)?,
        }
    }
    Ok(())
}

fn json_scalar_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------
// Read trace
// ---------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ObservationView {
    pub revision: u64,
    pub recorded_at_unix_ms: u64,
    pub observation: IamCallbackObservation,
}

#[derive(Debug, Serialize)]
pub struct TraceReadback {
    pub trace_id: String,
    pub state: &'static str,
    pub observations: Vec<ObservationView>,
}

pub async fn read_trace(trace_id_raw: &str) -> Result<TraceReadback, IamError> {
    read_trace_with_environment(trace_id_raw, &|name| std::env::var(name).ok()).await
}

async fn read_trace_with_environment(
    trace_id_raw: &str,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<TraceReadback, IamError> {
    let trace_id: IamTraceId = trace_id_raw
        .trim()
        .parse()
        .map_err(|_| IamError::InvalidTraceId)?;
    let store = open_store(lookup).await?;
    let key = trace_id.to_string();

    let Some((_, state_bytes)) = store
        .read_current(&key)
        .await
        .map_err(map_persistence_error)?
    else {
        return Err(IamError::NotFound);
    };
    let mut state = deserialize_state(&state_bytes)?;

    let history = store
        .read_history(&key)
        .await
        .map_err(map_persistence_error)?;

    // Explicit, durable recovery for the one crash window `receive_
    // callback`'s own self-healing (SCORPION_IAM_CALLBACK_RECEIVER_AND_
    // PERSISTENCE_001's follow-up fix) cannot close: an observation
    // durably appended, the process dying before AwaitingCallback ->
    // Received was persisted, and no later callback ever arriving to
    // trigger that self-healing retry. Readback is the one access path
    // guaranteed to be exercised for any trace an operator actually
    // cares about, so it repairs the condition here -- via the exact
    // same `try_mark_received`/`write_current` path `receive_callback`
    // uses, never a value fabricated only in this response -- before
    // returning. History is the authority for *whether* a repair is
    // due: this only fires when real, already-durable observation
    // history exists alongside a still-`AwaitingCallback` state; a trace
    // genuinely still awaiting its first callback (empty history) is
    // untouched. No process-local flag records that a repair happened —
    // this check is re-derived from durable state on every call, so it
    // is safe to run again even if a previous repair attempt itself
    // failed.
    if matches!(state, IamTraceState::AwaitingCallback) && !history.is_empty() {
        try_mark_received(&store, &trace_id).await;
        if let Some((_, refreshed_bytes)) = store
            .read_current(&key)
            .await
            .map_err(map_persistence_error)?
        {
            state = deserialize_state(&refreshed_bytes)?;
        }
    }

    let mut observations = Vec::with_capacity(history.len());
    for (revision, bytes, recorded_at) in history {
        let observation: IamCallbackObservation =
            serde_json::from_slice(&bytes).map_err(|_| IamError::Internal)?;
        let recorded_at_unix_ms = recorded_at
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        observations.push(ObservationView {
            revision,
            recorded_at_unix_ms,
            observation,
        });
    }

    Ok(TraceReadback {
        trace_id: trace_id.to_string(),
        state: state_label(state),
        observations,
    })
}

// ---------------------------------------------------------------------
// Operator-facing callback response page
// ---------------------------------------------------------------------

/// The page a browser sees after delivering a callback. Only the trace id
/// is ever embedded — a fixed-format, non-secret identifier
/// (`IamTraceId`'s `Display` never contains attacker-controlled content) —
/// never any captured fact, redacted or otherwise.
pub fn callback_received_page(trace_id: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>Scorpion</title></head><body>\
         <p>Callback received by Scorpion.</p>\
         <p>Trace: {trace_id}</p>\
         <p>You may return to Scorpion.</p>\
         </body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use spider::features::iam_trace::IamFactStatus;

    fn store_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "scorpion-app-iam-test-{}-{}.sqlite3",
            std::process::id(),
            IamTraceId::new()
        ))
    }

    fn configured_lookup(path: &std::path::Path) -> impl Fn(&str) -> Option<String> {
        let db = path.to_string_lossy().to_string();
        move |name| match name {
            "SCORPION_DOMAIN_DB" => Some(db.clone()),
            "SCORPION_API_BIND" => Some("127.0.0.1:8787".to_string()),
            _ => None,
        }
    }

    fn unconfigured_lookup() -> impl Fn(&str) -> Option<String> {
        |_name: &str| None
    }

    fn assert_no_validated(facts: &[IamFact]) {
        assert!(
            !facts
                .iter()
                .any(|fact| matches!(fact.status, IamFactStatus::Validated { .. })),
            "no fact in this production path may ever be Validated: {facts:?}"
        );
    }

    fn fact<'a>(facts: &'a [IamFact], name: &str) -> &'a IamFact {
        facts
            .iter()
            .find(|fact| fact.name == name)
            .unwrap_or_else(|| panic!("no fact named {name:?} in {facts:?}"))
    }

    fn find_fact<'a>(facts: &'a [IamFact], name: &str) -> Option<&'a IamFact> {
        facts.iter().find(|fact| fact.name == name)
    }

    /// Build a real compact three-segment JWT string from raw header/
    /// payload JSON text and a raw (never re-encoded) signature segment —
    /// exactly the shape a real OIDC provider would deliver.
    fn make_jwt(header_json: &str, payload_json: &str, signature_segment: &str) -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        format!("{header_b64}.{payload_b64}.{signature_segment}")
    }

    // ---------------------------------------------------------------
    // 1/2: create trace
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn create_trace_starts_awaiting_callback_and_uri_contains_the_trace_id() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let response = create_trace_with_environment(&configured_lookup(&path))
            .await
            .unwrap();
        assert_eq!(response.state, "awaiting_callback");
        assert!(response.trace_id.starts_with(IamTraceId::PREFIX));
        assert!(response.callback_uri.contains(&response.trace_id));
        assert!(response
            .callback_uri
            .starts_with("http://127.0.0.1:8787/iam/callback/"));
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 3/6/10: GET callback, non-sensitive + sensitive query facts, first
    // callback transitions AwaitingCallback -> Received
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn get_callback_persists_facts_redacts_sensitive_value_and_transitions_to_received() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();

        const SECRET: &str = "super-secret-test-value";
        let query = format!("foo=bar&code={SECRET}");
        receive_callback_with_environment(&created.trace_id, "GET", &query, None, b"", &lookup)
            .await
            .unwrap();

        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        assert_eq!(readback.state, "received");
        assert_eq!(readback.observations.len(), 1);
        let facts = &readback.observations[0].observation.facts;
        assert_no_validated(facts);

        let foo = facts.iter().find(|f| f.name == "foo").unwrap();
        assert_eq!(
            foo.status,
            IamFactStatus::Observed {
                value: "bar".into()
            }
        );

        let code = facts.iter().find(|f| f.name == "code").unwrap();
        match &code.status {
            IamFactStatus::Redacted { sha256_digest } => assert_ne!(sha256_digest, SECRET),
            other => panic!("expected code to be Redacted, got {other:?}"),
        }

        // The plaintext secret must never appear anywhere in the durable
        // serialized observation.
        let raw_history = DomainPersistence::open(&path)
            .await
            .unwrap()
            .read_history(&created.trace_id)
            .await
            .unwrap();
        for (_, bytes, _) in &raw_history {
            assert!(!String::from_utf8_lossy(bytes).contains(SECRET));
        }
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 4/7: form POST, sensitive form value never persisted plaintext
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn form_post_persists_facts_and_redacts_sensitive_value() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();

        const SECRET: &str = "form-secret-sentinel";
        let body = format!("state=xyz&access_token={SECRET}");
        receive_callback_with_environment(
            &created.trace_id,
            "POST",
            "",
            Some("application/x-www-form-urlencoded"),
            body.as_bytes(),
            &lookup,
        )
        .await
        .unwrap();

        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        assert_eq!(
            readback.observations[0].observation.protocol,
            IamProtocolClassification::OAuth2
        );
        let facts = &readback.observations[0].observation.facts;
        assert_no_validated(facts);
        // This callback carries `state`, one of the documented OAuth2
        // signals, so it classifies OAuth2 — and `state` itself becomes
        // NotValidated (SCORPION_IAM_OIDC_AND_JWT_INSPECTION_001: no
        // expectation-storage model exists to compare it against).
        let state = facts.iter().find(|f| f.name == "state").unwrap();
        assert_eq!(
            state.status,
            IamFactStatus::NotValidated {
                value: "xyz".into()
            }
        );
        let token = facts.iter().find(|f| f.name == "access_token").unwrap();
        match &token.status {
            IamFactStatus::Redacted { sha256_digest } => assert_ne!(sha256_digest, SECRET),
            other => panic!("expected access_token to be Redacted, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 5/8/9: JSON POST, nested sensitive field, SAMLResponse
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn json_post_persists_facts_redacts_nested_and_saml_fields() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();

        const NESTED_SECRET: &str = "nested-client-secret-sentinel";
        const SAML_SECRET: &str = "opaque-base64-saml-response-sentinel";
        let body = serde_json::json!({
            "iss": "https://idp.example.test",
            "SAMLResponse": SAML_SECRET,
            "auth": {
                "client_secret": NESTED_SECRET,
                "scope": "openid"
            }
        })
        .to_string();

        receive_callback_with_environment(
            &created.trace_id,
            "POST",
            "",
            Some("application/json"),
            body.as_bytes(),
            &lookup,
        )
        .await
        .unwrap();

        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        let facts = &readback.observations[0].observation.facts;
        assert_no_validated(facts);

        let iss = facts.iter().find(|f| f.name == "iss").unwrap();
        assert!(matches!(iss.status, IamFactStatus::Observed { .. }));

        let saml = facts.iter().find(|f| f.name == "SAMLResponse").unwrap();
        match &saml.status {
            IamFactStatus::Redacted { sha256_digest } => assert_ne!(sha256_digest, SAML_SECRET),
            other => panic!("expected SAMLResponse to be Redacted, got {other:?}"),
        }

        let nested = facts
            .iter()
            .find(|f| f.name == "auth.client_secret")
            .unwrap();
        match &nested.status {
            IamFactStatus::Redacted { sha256_digest } => assert_ne!(sha256_digest, NESTED_SECRET),
            other => panic!("expected auth.client_secret to be Redacted, got {other:?}"),
        }
        let scope = facts.iter().find(|f| f.name == "auth.scope").unwrap();
        assert!(matches!(scope.status, IamFactStatus::Observed { .. }));

        let raw_history = DomainPersistence::open(&path)
            .await
            .unwrap()
            .read_history(&created.trace_id)
            .await
            .unwrap();
        for (_, bytes, _) in &raw_history {
            let text = String::from_utf8_lossy(bytes);
            assert!(!text.contains(NESTED_SECRET));
            assert!(!text.contains(SAML_SECRET));
        }
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 11: second callback appends without reapplying the transition
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn second_callback_is_appended_and_state_remains_received() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();

        receive_callback_with_environment(&created.trace_id, "GET", "first=1", None, b"", &lookup)
            .await
            .unwrap();
        receive_callback_with_environment(&created.trace_id, "GET", "second=2", None, b"", &lookup)
            .await
            .unwrap();

        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        assert_eq!(readback.state, "received");
        assert_eq!(readback.observations.len(), 2);
        assert_eq!(readback.observations[0].revision, 1);
        assert_eq!(readback.observations[1].revision, 2);
        assert_eq!(readback.observations[0].observation.facts[0].name, "first");
        assert_eq!(readback.observations[1].observation.facts[0].name, "second");
        let _ = std::fs::remove_file(&path);
    }

    /// Reproduces, directly, the exact consistency gap an owner review
    /// caught in this frontier's first version: an observation durably
    /// appended (history has 1 record) while the transition to `Received`
    /// never completed (current state is still `AwaitingCallback` — as if
    /// the process had died between the two writes). Proves a *second*
    /// callback repairs it, rather than staying stuck forever because
    /// `revision` is never `1` again.
    #[tokio::test]
    async fn a_second_callback_repairs_a_trace_stuck_awaiting_callback_despite_prior_history() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        let trace_id: IamTraceId = created.trace_id.parse().unwrap();
        let key = trace_id.to_string();

        // Manufacture the exact inconsistent state directly, bypassing
        // receive_callback entirely: history already has one observation,
        // but current state is still AwaitingCallback (as if the
        // transition attempt for it had failed).
        let store = open_store(&lookup).await.unwrap();
        let stuck_observation = IamCallbackObservation::new(
            trace_id,
            "GET",
            format!("/iam/callback/{trace_id}"),
            now_unix_ms(),
            IamProtocolClassification::Generic,
            vec![IamFact::observed("stuck", "true")],
        );
        store
            .append_history(
                &key,
                1,
                &serde_json::to_vec(&stuck_observation).unwrap(),
                SystemTime::now(),
            )
            .await
            .unwrap();
        // Sanity check the manufactured inconsistency actually exists —
        // reading the *raw* persisted state directly, not through
        // `read_trace`, since (after
        // SCORPION_IAM_CALLBACK_TRACE_STATE_RECOVERY_001) `read_trace`
        // itself now repairs exactly this condition; going through it
        // here would repair the state before this test ever exercises
        // the second-callback repair path it means to prove.
        let (_, raw_state_bytes) = store.read_current(&key).await.unwrap().unwrap();
        let raw_state: IamTraceState = serde_json::from_slice(&raw_state_bytes).unwrap();
        assert_eq!(raw_state, IamTraceState::AwaitingCallback);
        let history = store.read_history(&key).await.unwrap();
        assert_eq!(history.len(), 1);

        // A second, ordinary callback should both append its own
        // observation *and* repair the stuck state — even though this
        // observation's own revision is 2, not 1.
        receive_callback_with_environment(&created.trace_id, "GET", "second=2", None, b"", &lookup)
            .await
            .unwrap();

        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        assert_eq!(
            readback.state, "received",
            "a later callback must repair a trace stuck AwaitingCallback despite prior history"
        );
        assert_eq!(readback.observations.len(), 2);
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 12/13: nonexistent / malformed trace id
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn nonexistent_trace_is_rejected() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        // A well-formed but never-created id.
        let fake = IamTraceId::new().to_string();
        let error = receive_callback_with_environment(&fake, "GET", "", None, b"", &lookup)
            .await
            .unwrap_err();
        assert!(matches!(error, IamError::NotFound));
        let error = read_trace_with_environment(&fake, &lookup)
            .await
            .unwrap_err();
        assert!(matches!(error, IamError::NotFound));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn malformed_trace_id_is_rejected() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let error =
            receive_callback_with_environment("not-a-trace-id", "GET", "", None, b"", &lookup)
                .await
                .unwrap_err();
        assert!(matches!(error, IamError::InvalidTraceId));
        let error = read_trace_with_environment("not-a-trace-id", &lookup)
            .await
            .unwrap_err();
        assert!(matches!(error, IamError::InvalidTraceId));
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 14/15: malformed JSON / unsupported content type
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn malformed_json_is_rejected() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        let error = receive_callback_with_environment(
            &created.trace_id,
            "POST",
            "",
            Some("application/json"),
            b"{ not json",
            &lookup,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, IamError::MalformedJson));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn unsupported_content_type_is_rejected() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        let error = receive_callback_with_environment(
            &created.trace_id,
            "POST",
            "",
            Some("multipart/form-data; boundary=x"),
            b"irrelevant",
            &lookup,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, IamError::UnsupportedContentType));
        // Missing Content-Type entirely is equally unsupported.
        let error =
            receive_callback_with_environment(&created.trace_id, "POST", "", None, b"x=1", &lookup)
                .await
                .unwrap_err();
        assert!(matches!(error, IamError::UnsupportedContentType));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn malformed_percent_encoding_is_rejected() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        let error = receive_callback_with_environment(
            &created.trace_id,
            "GET",
            "foo=%zz",
            None,
            b"",
            &lookup,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, IamError::MalformedPercentEncoding));
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 16/17: oversized body (too many parameters) / oversized parameter
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn oversized_body_too_many_parameters_is_rejected() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        let query = (0..IAM_MAX_FACTS + 1)
            .map(|i| format!("p{i}=v"))
            .collect::<Vec<_>>()
            .join("&");
        let error =
            receive_callback_with_environment(&created.trace_id, "GET", &query, None, b"", &lookup)
                .await
                .unwrap_err();
        assert!(matches!(error, IamError::ParameterLimitExceeded));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn oversized_single_parameter_is_rejected() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        let huge_value = "a".repeat(IAM_MAX_VALUE_BYTES + 1);
        let query = format!("foo={huge_value}");
        let error =
            receive_callback_with_environment(&created.trace_id, "GET", &query, None, b"", &lookup)
                .await
                .unwrap_err();
        assert!(matches!(error, IamError::ParameterLimitExceeded));
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 18: callback response never echoes a sensitive value
    // ---------------------------------------------------------------

    #[test]
    fn callback_response_never_echoes_a_sensitive_value() {
        const SECRET: &str = "authorization-code-should-never-appear";
        let page = callback_received_page("iam_trace_0123456789abcdef0123456789abcdef");
        assert!(!page.contains(SECRET));
        assert!(page.contains("iam_trace_0123456789abcdef0123456789abcdef"));
    }

    // ---------------------------------------------------------------
    // 19: readback returns redacted observations
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn readback_returns_redacted_observations() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        receive_callback_with_environment(
            &created.trace_id,
            "GET",
            "code=some-secret-value",
            None,
            b"",
            &lookup,
        )
        .await
        .unwrap();

        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        let json = serde_json::to_string(&readback.observations[0].observation.facts).unwrap();
        assert!(json.contains("redacted"));
        assert!(!json.contains("some-secret-value"));
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 20: persistence survives reopening the same DB
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn persistence_survives_reopening_the_database() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        receive_callback_with_environment(&created.trace_id, "GET", "foo=bar", None, b"", &lookup)
            .await
            .unwrap();

        // A brand-new lookup/store resolution against the exact same file
        // — nothing in this test reuses an in-process handle.
        let reopened_lookup = configured_lookup(&path);
        let readback = read_trace_with_environment(&created.trace_id, &reopened_lookup)
            .await
            .unwrap();
        assert_eq!(readback.state, "received");
        assert_eq!(readback.observations.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    /// SCORPION_IAM_CALLBACK_TRACE_STATE_RECOVERY_001: proves the crash
    /// window the prior self-healing-on-callback fix could not close —
    /// an observation durably appended, the state transition never
    /// persisted, and *no second callback ever arriving* — is repaired
    /// by the normal readback path instead, and that the repair is a
    /// real durable write, not a value fabricated only in one response.
    #[tokio::test]
    async fn readback_repairs_a_stuck_trace_and_the_repair_survives_reopening_the_database() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        let trace_id: IamTraceId = created.trace_id.parse().unwrap();
        let key = trace_id.to_string();

        // Manufacture the exact scenario directly, with no second
        // callback anywhere in this test: one real, durable observation
        // in history; current state left at AwaitingCallback, exactly as
        // a process crash between the two writes would leave it.
        {
            let store = open_store(&lookup).await.unwrap();
            let stuck_observation = IamCallbackObservation::new(
                trace_id,
                "GET",
                format!("/iam/callback/{trace_id}"),
                now_unix_ms(),
                IamProtocolClassification::Generic,
                vec![IamFact::observed("stuck", "true")],
            );
            store
                .append_history(
                    &key,
                    1,
                    &serde_json::to_vec(&stuck_observation).unwrap(),
                    SystemTime::now(),
                )
                .await
                .unwrap();
            // `store` dropped here — close persistence explicitly before
            // the next step reopens it fresh.
        }

        // Reopen persistence independently and perform the *normal*
        // readback path — no test-only recovery function, the same
        // `read_trace` a real API call would use.
        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        assert_eq!(
            readback.state, "received",
            "readback must repair a trace stuck AwaitingCallback despite durable history, \
             even with no second callback ever arriving"
        );
        assert_eq!(
            readback.observations.len(),
            1,
            "no observation may be lost by the repair"
        );

        // Reopen again, independently, and read the raw persisted state
        // directly — proving the repair was actually durably written,
        // not merely returned once by the call above.
        let reopened_lookup = configured_lookup(&path);
        let store = open_store(&reopened_lookup).await.unwrap();
        let (_, state_bytes) = store.read_current(&key).await.unwrap().unwrap();
        let state: IamTraceState = serde_json::from_slice(&state_bytes).unwrap();
        assert_eq!(
            state,
            IamTraceState::Received,
            "the repair must survive a process restart, not just live in one response"
        );

        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 22: no Validated fact anywhere in this production path
    // ---------------------------------------------------------------

    #[test]
    fn source_never_constructs_a_validated_fact() {
        let source = include_str!("iam.rs");
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(
            !production.contains("IamFactStatus::Validated"),
            "scorpion_app::iam must never construct IamFactStatus::Validated"
        );
    }

    // ---------------------------------------------------------------
    // Store-not-configured behaves truthfully (matching every sibling
    // module's own convention).
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn unconfigured_store_fails_closed_for_every_entry_point() {
        let lookup = unconfigured_lookup();
        assert!(matches!(
            create_trace_with_environment(&lookup).await.unwrap_err(),
            IamError::NotConfigured
        ));
        let fake = IamTraceId::new().to_string();
        assert!(matches!(
            receive_callback_with_environment(&fake, "GET", "", None, b"", &lookup)
                .await
                .unwrap_err(),
            IamError::NotConfigured
        ));
        assert!(matches!(
            read_trace_with_environment(&fake, &lookup)
                .await
                .unwrap_err(),
            IamError::NotConfigured
        ));
    }

    // =================================================================
    // SCORPION_IAM_OIDC_AND_JWT_INSPECTION_001
    // =================================================================

    // ---------------------------------------------------------------
    // 1/2/3: classification and its documented precedence
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn callback_with_code_classifies_oauth2() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        receive_callback_with_environment(&created.trace_id, "GET", "code=abc", None, b"", &lookup)
            .await
            .unwrap();
        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        assert_eq!(
            readback.observations[0].observation.protocol,
            IamProtocolClassification::OAuth2
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn callback_with_id_token_classifies_oidc() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        let jwt = make_jwt(
            r#"{"alg":"none"}"#,
            r#"{"iss":"https://issuer.example"}"#,
            "",
        );
        receive_callback_with_environment(
            &created.trace_id,
            "GET",
            &format!("id_token={jwt}"),
            None,
            b"",
            &lookup,
        )
        .await
        .unwrap();
        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        assert_eq!(
            readback.observations[0].observation.protocol,
            IamProtocolClassification::Oidc
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn id_token_presence_takes_precedence_over_code_and_state() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        let jwt = make_jwt(
            r#"{"alg":"none"}"#,
            r#"{"iss":"https://issuer.example"}"#,
            "",
        );
        let query = format!("code=abc&state=xyz&id_token={jwt}");
        receive_callback_with_environment(&created.trace_id, "GET", &query, None, b"", &lookup)
            .await
            .unwrap();
        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        assert_eq!(
            readback.observations[0].observation.protocol,
            IamProtocolClassification::Oidc,
            "id_token presence must classify Oidc even with code/state also present"
        );
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 4-17: JWT decode — header/payload facts, audience shapes, nonce,
    // signature presence/verification semantics
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn valid_jwt_decodes_header_and_payload_facts_with_string_audience() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();

        let header = r#"{"alg":"none","typ":"JWT","kid":"scorpion-test-key"}"#;
        let payload = r#"{"iss":"https://issuer.example","aud":"scorpion-test","exp":9999999999,"iat":1000000000,"nbf":999999999,"nonce":"test-nonce"}"#;
        let jwt = make_jwt(header, payload, "sig");

        receive_callback_with_environment(
            &created.trace_id,
            "GET",
            &format!("id_token={jwt}"),
            None,
            b"",
            &lookup,
        )
        .await
        .unwrap();
        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        let facts = &readback.observations[0].observation.facts;
        assert_no_validated(facts);

        // id_token itself stays Redacted (pre-existing generic redaction,
        // unrelated to this frontier's own code).
        assert!(matches!(
            fact(facts, "id_token").status,
            IamFactStatus::Redacted { .. }
        ));

        // Header facts (6/7/8).
        assert_eq!(
            fact(facts, "jwt.header.alg").status,
            IamFactStatus::Observed {
                value: "none".into()
            }
        );
        assert_eq!(
            fact(facts, "jwt.header.typ").status,
            IamFactStatus::Observed {
                value: "JWT".into()
            }
        );
        assert_eq!(
            fact(facts, "jwt.header.kid").status,
            IamFactStatus::Observed {
                value: "scorpion-test-key".into()
            }
        );

        // Payload facts (9/10/12/13/14).
        assert_eq!(
            fact(facts, "jwt.iss").status,
            IamFactStatus::Observed {
                value: "https://issuer.example".into()
            }
        );
        assert_eq!(
            fact(facts, "jwt.aud").status,
            IamFactStatus::Observed {
                value: "scorpion-test".into()
            }
        );
        assert_eq!(
            fact(facts, "jwt.exp").status,
            IamFactStatus::Observed {
                value: "9999999999".into()
            }
        );
        assert_eq!(
            fact(facts, "jwt.iat").status,
            IamFactStatus::Observed {
                value: "1000000000".into()
            }
        );
        assert_eq!(
            fact(facts, "jwt.nbf").status,
            IamFactStatus::Observed {
                value: "999999999".into()
            }
        );

        // 15: nonce is surfaced but NotValidated (no expectation model —
        // see this frontier's own state/nonce finding), never Observed.
        assert_eq!(
            fact(facts, "jwt.nonce").status,
            IamFactStatus::NotValidated {
                value: "test-nonce".into()
            }
        );

        // 16/17: signature presence is truthful; verification is always
        // NotValidated.
        assert_eq!(
            fact(facts, "jwt.signature.present").status,
            IamFactStatus::Observed {
                value: "true".into()
            }
        );
        assert_eq!(
            fact(facts, "jwt.signature.verification").status,
            IamFactStatus::NotValidated {
                value: "not_attempted".into()
            }
        );

        // Not persisted at all per this frontier's scope: raw claims
        // object, sub/email/name/groups/roles were never in the payload
        // above, but confirm the generic claim allowlist itself excludes
        // anything not explicitly named.
        assert!(find_fact(facts, "sub").is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn jwt_audience_array_is_surfaced_truthfully() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();

        let payload = r#"{"iss":"https://issuer.example","aud":["a","b"]}"#;
        let jwt = make_jwt(r#"{"alg":"none"}"#, payload, "sig");
        receive_callback_with_environment(
            &created.trace_id,
            "GET",
            &format!("id_token={jwt}"),
            None,
            b"",
            &lookup,
        )
        .await
        .unwrap();
        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        let facts = &readback.observations[0].observation.facts;
        // Represented deterministically and losslessly as the exact JSON
        // array text — never silently collapsed to one element or joined
        // ambiguously.
        assert_eq!(
            fact(facts, "jwt.aud").status,
            IamFactStatus::Observed {
                value: r#"["a","b"]"#.into()
            }
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn missing_optional_header_and_payload_fields_are_simply_absent() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();

        // No typ/kid in the header; no aud/exp/iat/nbf/nonce in the payload.
        let jwt = make_jwt(
            r#"{"alg":"none"}"#,
            r#"{"iss":"https://issuer.example"}"#,
            "",
        );
        receive_callback_with_environment(
            &created.trace_id,
            "GET",
            &format!("id_token={jwt}"),
            None,
            b"",
            &lookup,
        )
        .await
        .unwrap();
        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        let facts = &readback.observations[0].observation.facts;
        assert!(find_fact(facts, "jwt.header.typ").is_none());
        assert!(find_fact(facts, "jwt.header.kid").is_none());
        assert!(find_fact(facts, "jwt.aud").is_none());
        assert!(find_fact(facts, "jwt.nonce").is_none());
        // Signature segment is empty here — presence must say so truthfully.
        assert_eq!(
            fact(facts, "jwt.signature.present").status,
            IamFactStatus::Observed {
                value: "false".into()
            }
        );
        assert_eq!(
            fact(facts, "jwt.signature.verification").status,
            IamFactStatus::NotValidated {
                value: "not_attempted".into()
            }
        );
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 18-21/33/34: malformed JWT — truthful diagnostics, never a panic,
    // never fabricated claims, callback still durably recorded
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn malformed_jwt_shapes_produce_a_truthful_diagnostic_never_a_fabricated_claim() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);

        let cases: &[(&str, &str)] = &[
            ("only.two", "malformed_segment_count"),
            (
                "not-base64!.also-not-base64!.sig",
                "malformed_base64url_header",
            ),
        ];
        for (raw_id_token, expected_reason) in cases {
            let created = create_trace_with_environment(&lookup).await.unwrap();
            receive_callback_with_environment(
                &created.trace_id,
                "GET",
                &format!("id_token={raw_id_token}"),
                None,
                b"",
                &lookup,
            )
            .await
            .unwrap();
            let readback = read_trace_with_environment(&created.trace_id, &lookup)
                .await
                .unwrap();
            // 33: the callback observation is still durably recorded —
            // it never disappears merely because JWT inspection failed.
            assert_eq!(readback.observations.len(), 1);
            assert_eq!(
                readback.observations[0].observation.protocol,
                IamProtocolClassification::Oidc,
                "protocol stays Oidc on id_token presence regardless of decode success"
            );
            let facts = &readback.observations[0].observation.facts;
            assert!(matches!(
                fact(facts, "id_token").status,
                IamFactStatus::Redacted { .. }
            ));
            assert_eq!(
                fact(facts, "jwt.decode_error").status,
                IamFactStatus::NotValidated {
                    value: (*expected_reason).into()
                }
            );
            // No fabricated claim of any kind.
            assert!(find_fact(facts, "jwt.iss").is_none());
            assert!(find_fact(facts, "jwt.header.alg").is_none());
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_header_json_and_payload_json_are_diagnosed_truthfully_and_never_panic() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        // Valid base64url, but the decoded bytes are not JSON at all.
        let not_json_header = URL_SAFE_NO_PAD.encode(b"not json");
        let valid_payload = URL_SAFE_NO_PAD.encode(br#"{"iss":"x"}"#);
        let jwt = format!("{not_json_header}.{valid_payload}.sig");
        assert_eq!(decode_id_token(&jwt), Err("malformed_header_json"));

        let valid_header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let not_json_payload = URL_SAFE_NO_PAD.encode(b"also not json");
        let jwt = format!("{valid_header}.{not_json_payload}.sig");
        assert_eq!(decode_id_token(&jwt), Err("malformed_payload_json"));

        // 34: garbage input of every shape must never panic.
        for garbage in ["", ".", "..", "a.b.c.d", "🙂.🙂.🙂", "a..c"] {
            let _ = decode_id_token(garbage);
        }
    }

    // ---------------------------------------------------------------
    // 22/23/24: JWT-specific bounds
    // ---------------------------------------------------------------

    #[test]
    fn oversized_raw_jwt_is_rejected() {
        let huge = "a".repeat(JWT_MAX_RAW_BYTES + 1);
        assert_eq!(decode_id_token(&huge), Err("oversized_token"));
    }

    #[test]
    fn oversized_decoded_header_is_rejected() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        // A header whose *decoded* JSON exceeds the header bound, built
        // from one oversized string field so it still parses as valid
        // JSON right up until the size check rejects it first.
        let big_header = format!(
            r#"{{"alg":"none","pad":"{}"}}"#,
            "a".repeat(JWT_MAX_DECODED_HEADER_BYTES + 1)
        );
        let header_b64 = URL_SAFE_NO_PAD.encode(big_header.as_bytes());
        let payload_b64 = URL_SAFE_NO_PAD.encode(br#"{"iss":"x"}"#);
        let jwt = format!("{header_b64}.{payload_b64}.sig");
        assert_eq!(decode_id_token(&jwt), Err("oversized_decoded_header"));
    }

    #[test]
    fn oversized_decoded_payload_is_rejected() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let header_b64 = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let big_payload = format!(
            r#"{{"iss":"x","pad":"{}"}}"#,
            "a".repeat(JWT_MAX_DECODED_PAYLOAD_BYTES + 1)
        );
        let payload_b64 = URL_SAFE_NO_PAD.encode(big_payload.as_bytes());
        let jwt = format!("{header_b64}.{payload_b64}.sig");
        assert_eq!(decode_id_token(&jwt), Err("oversized_decoded_payload"));
    }

    // ---------------------------------------------------------------
    // 25/26/27: raw id_token absent from persisted form, Debug, and any
    // logging path
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn raw_id_token_absent_from_persisted_readback_and_debug_output() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();

        let jwt = make_jwt(
            r#"{"alg":"none"}"#,
            r#"{"iss":"https://issuer.example","nonce":"test-nonce"}"#,
            "unique-signature-sentinel-bytes",
        );
        receive_callback_with_environment(
            &created.trace_id,
            "GET",
            &format!("id_token={jwt}"),
            None,
            b"",
            &lookup,
        )
        .await
        .unwrap();

        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        // 26: Debug output of the whole readback.
        let debug = format!("{readback:?}");
        assert!(!debug.contains(&jwt));

        // 25: the exact durably persisted bytes.
        let store = open_store(&lookup).await.unwrap();
        let history = store.read_history(&created.trace_id).await.unwrap();
        for (_, bytes, _) in &history {
            let text = String::from_utf8_lossy(bytes);
            assert!(!text.contains(&jwt));
            assert!(!text.contains("unique-signature-sentinel-bytes"));
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn no_eprintln_in_jwt_inspection_ever_references_raw_token_material() {
        // 27: this module's only logging calls are `eprintln!` on
        // persistence-backend errors (sqlx internals), never on request
        // content. Confirm no `eprintln!`/`println!` call exists anywhere
        // between the OIDC/JWT section markers at all.
        let source = include_str!("iam.rs");
        let start = source
            .find("SCORPION_IAM_OIDC_AND_JWT_INSPECTION_001")
            .unwrap();
        let jwt_section = &source[start..];
        let production = jwt_section.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("eprintln!"));
        assert!(!production.contains("println!"));
    }

    // ---------------------------------------------------------------
    // 28/29/30: existing sensitive-name redaction stays intact;
    // access_token is never JWT-decoded even when JWT-shaped
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn code_access_token_refresh_token_remain_redacted_and_access_token_is_never_jwt_decoded()
    {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();

        // access_token is JWT-*shaped* (three dot-separated segments) —
        // it must still only ever be Redacted, never decoded.
        let jwt_shaped_access_token = make_jwt(r#"{"alg":"none"}"#, r#"{"iss":"x"}"#, "sig");
        let query = format!(
            "code=auth-code-sentinel&refresh_token=refresh-sentinel&access_token={jwt_shaped_access_token}"
        );
        receive_callback_with_environment(&created.trace_id, "GET", &query, None, b"", &lookup)
            .await
            .unwrap();

        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        let facts = &readback.observations[0].observation.facts;
        assert!(matches!(
            fact(facts, "code").status,
            IamFactStatus::Redacted { .. }
        ));
        assert!(matches!(
            fact(facts, "access_token").status,
            IamFactStatus::Redacted { .. }
        ));
        assert!(matches!(
            fact(facts, "refresh_token").status,
            IamFactStatus::Redacted { .. }
        ));
        // No jwt.* fact exists at all — access_token was never decoded.
        assert!(!facts.iter().any(|f| f.name.starts_with("jwt.")));
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------
    // 31: no signature-authenticity path constructs Validated (also
    // covered generally by `source_never_constructs_a_validated_fact`,
    // which scans this entire file including the OIDC/JWT section)
    // ---------------------------------------------------------------

    #[test]
    fn jwt_section_never_constructs_a_validated_fact() {
        let source = include_str!("iam.rs");
        let start = source
            .find("SCORPION_IAM_OIDC_AND_JWT_INSPECTION_001")
            .unwrap();
        let jwt_section = &source[start..];
        let production = jwt_section.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("IamFactStatus::Validated"));
    }

    // ---------------------------------------------------------------
    // 32: generic non-IAM-looking callback still works, classified
    // Generic, exactly as before this frontier
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn plain_generic_callback_with_no_oauth_oidc_signals_stays_generic() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let created = create_trace_with_environment(&lookup).await.unwrap();
        receive_callback_with_environment(&created.trace_id, "GET", "foo=bar", None, b"", &lookup)
            .await
            .unwrap();
        let readback = read_trace_with_environment(&created.trace_id, &lookup)
            .await
            .unwrap();
        assert_eq!(
            readback.observations[0].observation.protocol,
            IamProtocolClassification::Generic
        );
        assert_eq!(
            fact(&readback.observations[0].observation.facts, "foo").status,
            IamFactStatus::Observed {
                value: "bar".into()
            }
        );
        let _ = std::fs::remove_file(&path);
    }
}
