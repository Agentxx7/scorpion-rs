//! Canonical transport-request conversion for MCP tool schemas — the ONE
//! place an incoming `"transport": {...}` JSON field becomes
//! `spider::features::transport::TransportPolicy`, reused by every
//! applicable tool (`spider_scrape`, `spider_crawl`, `spider_links`, and
//! the four source-reader tools).
//!
//! [`TransportParam`] is deliberately its own small wire-format struct —
//! not a direct `#[derive(JsonSchema)]` on `spider`'s own
//! `TransportPolicy`/`TransportRequest` (`spider` has no reason to depend
//! on `schemars`, and the raw `TransportPolicy` enum is never the public
//! MCP wire contract; see `spider::features::transport::TransportRequest`'s
//! doc comment). Converts into that same core `TransportRequest` exactly
//! once, so this crate's request-validation logic can never independently
//! drift from the CLI's.

use rmcp::schemars;
use serde::Deserialize;
use spider::features::transport::{
    TransportError, TransportMode, TransportPolicy, TransportRequest,
};

/// Wire-level `mode` value for [`TransportParam`] — the smallest
/// schema-friendly type whose public vocabulary is exactly `"default"` and
/// `"tor"` (Section F of the blocker-fix frontier). Replaces a raw
/// `Option<String>`, which advertised an arbitrary-string JSON Schema and
/// pushed "unknown mode" rejection into runtime string-matching. With this
/// enum, an unrecognized value (e.g. `"onion"`) fails deserialization
/// before a `TransportParam` — let alone a `TransportRequest` — ever
/// exists, and the schema itself documents the exact two allowed values.
/// Still just a wire mirror, structurally identical to
/// `spider::features::transport::TransportMode`: converting it is a plain
/// variant mapping, not a second validation matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TransportModeParam {
    Default,
    Tor,
}

impl From<TransportModeParam> for TransportMode {
    fn from(value: TransportModeParam) -> Self {
        match value {
            TransportModeParam::Default => TransportMode::Default,
            TransportModeParam::Tor => TransportMode::Tor,
        }
    }
}

/// Optional nested `transport` field for applicable tool schemas. Missing
/// entirely (`None` at the call site) means Default behavior — fully
/// additive, byte-identical to every existing request that predates this
/// field (Section P).
#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct TransportParam {
    /// Transport mode: "default" (normal networking) or "tor" (fail-closed
    /// SOCKS5h). Defaults to "default" when omitted.
    #[serde(default)]
    pub mode: Option<TransportModeParam>,
    /// Tor SOCKS5h proxy endpoint — required when mode is "tor", must be
    /// exactly `socks5h://HOST:PORT` (no path/query/fragment/credentials,
    /// explicit port required). Rejected if supplied when mode is
    /// "default" (or omitted).
    #[serde(default)]
    pub proxy: Option<String>,
}

/// Convert an optional `transport` param into the canonical
/// `TransportPolicy`. `None` (the field omitted entirely) is Default
/// transport with zero validation overhead — the exact pre-existing
/// behavior. An unrecognized `mode` string can no longer reach this
/// function at all — `TransportModeParam`'s closed two-variant schema
/// makes that a deserialization failure one layer up, before a
/// `TransportParam` exists.
pub fn resolve(param: Option<TransportParam>) -> Result<TransportPolicy, String> {
    let Some(param) = param else {
        return Ok(TransportPolicy::Default);
    };

    let mode = param.mode.map(TransportMode::from).unwrap_or_default();

    TransportRequest {
        mode,
        proxy: param.proxy,
    }
    .into_policy()
    .map_err(|error| error.to_string())
}

/// Machine-readable classification of a [`TransportError`] — matched
/// directly on the enum, never derived from its `Display` string, so a
/// consumer (e.g. a background crawl session's status) can distinguish
/// failure kinds without parsing prose. Kept narrow and additive: exists
/// solely to give [`crate::state::CrawlSession`] (and any future
/// transport-aware status surface) a stable machine field alongside the
/// existing human-readable message.
pub fn error_code(error: &TransportError) -> &'static str {
    match error {
        TransportError::InvalidEndpoint(_) => "invalid_endpoint",
        TransportError::UnsupportedScheme(_) => "unsupported_scheme",
        TransportError::CredentialsNotSupported => "credentials_not_supported",
        TransportError::TorNotCompiled => "tor_not_compiled",
        TransportError::OnionRequiresTor => "onion_requires_tor",
        TransportError::ProxyBuildFailed(_) => "proxy_build_failed",
        TransportError::RedirectTransportViolation(_) => "redirect_transport_violation",
        TransportError::IncompatibleConfiguration(_) => "incompatible_configuration",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_field_is_default_transport() {
        let policy = resolve(None).unwrap();
        assert!(matches!(policy, TransportPolicy::Default));
    }

    #[test]
    fn explicit_default_mode_no_proxy_is_default_transport() {
        let policy = resolve(Some(TransportParam {
            mode: Some(TransportModeParam::Default),
            proxy: None,
        }))
        .unwrap();
        assert!(matches!(policy, TransportPolicy::Default));
    }

    #[test]
    fn tor_mode_with_valid_proxy_is_tor_transport() {
        let policy = resolve(Some(TransportParam {
            mode: Some(TransportModeParam::Tor),
            proxy: Some("socks5h://127.0.0.1:9050".to_string()),
        }))
        .unwrap();
        assert!(matches!(policy, TransportPolicy::Tor(_)));
    }

    #[test]
    fn tor_mode_without_proxy_is_rejected() {
        assert!(resolve(Some(TransportParam {
            mode: Some(TransportModeParam::Tor),
            proxy: None,
        }))
        .is_err());
    }

    #[test]
    fn default_mode_with_proxy_is_rejected() {
        assert!(resolve(Some(TransportParam {
            mode: Some(TransportModeParam::Default),
            proxy: Some("socks5h://127.0.0.1:9050".to_string()),
        }))
        .is_err());
    }

    /// An unrecognized `mode` string can no longer even construct a
    /// `TransportParam` — `TransportModeParam`'s closed schema rejects it
    /// at JSON deserialization, one layer above `resolve`. This is a
    /// stricter guarantee than the old runtime string-match: malformed
    /// input never reaches request-validation logic at all.
    #[test]
    fn unknown_mode_string_fails_deserialization_before_resolve_is_reached() {
        let json = serde_json::json!({ "mode": "onion" });
        let result: Result<TransportParam, _> = serde_json::from_value(json);
        assert!(result.is_err(), "\"onion\" must not deserialize as a mode");
    }

    #[test]
    fn malformed_proxy_endpoint_is_rejected() {
        assert!(resolve(Some(TransportParam {
            mode: Some(TransportModeParam::Tor),
            proxy: Some("http://127.0.0.1:9050".to_string()),
        }))
        .is_err());
    }

    /// The advertised JSON Schema for `mode` exposes exactly the two
    /// public values — never an arbitrary-string surface (Section F/K).
    #[test]
    fn mode_schema_advertises_exactly_default_and_tor() {
        let schema = schemars::schema_for!(TransportModeParam);
        let value = serde_json::to_value(schema).unwrap();
        let enum_values: Vec<String> = value["enum"]
            .as_array()
            .expect("mode schema must advertise an enum of allowed values")
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(enum_values, vec!["default".to_string(), "tor".to_string()]);
    }

    #[test]
    fn error_codes_are_stable_and_distinct() {
        assert_eq!(
            error_code(&TransportError::TorNotCompiled),
            "tor_not_compiled"
        );
        assert_eq!(
            error_code(&TransportError::OnionRequiresTor),
            "onion_requires_tor"
        );
        assert_eq!(
            error_code(&TransportError::IncompatibleConfiguration("x".to_string())),
            "incompatible_configuration"
        );
    }
}
