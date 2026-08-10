//! Ephemeral, non-serializable request headers that may contain secrets.
//!
//! Values are always marked sensitive on insertion, cloning, and application.
//! The container exposes no value iterator, plaintext map, persistence API, or
//! network execution behavior.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

/// Ephemeral request headers whose values must never appear in diagnostics.
///
/// This type intentionally implements neither `Display` nor serde traits.
/// Its custom `Debug` output reveals only the number of header names stored.
#[derive(Default)]
pub struct SecretRequestHeaders {
    headers: HeaderMap,
}

impl SecretRequestHeaders {
    /// Construct an empty ephemeral header container.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct header names stored.
    pub fn len(&self) -> usize {
        self.headers.len()
    }

    /// Whether no headers are stored.
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }

    /// Insert or replace one header value, marking it sensitive regardless of
    /// the flag supplied by the caller. Repeated names deterministically
    /// replace their previous value; multi-value append is not supported.
    pub fn insert(&mut self, name: HeaderName, mut value: HeaderValue) {
        value.set_sensitive(true);
        self.headers.insert(name, value);
    }

    /// Parse and insert a header without retaining plaintext strings or
    /// exposing parser details that could echo the supplied secret value.
    pub fn try_insert(&mut self, name: &str, value: &str) -> Result<(), SecretHeaderError> {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| SecretHeaderError::InvalidHeaderName)?;
        let value =
            HeaderValue::from_str(value).map_err(|_| SecretHeaderError::InvalidHeaderValue)?;
        self.insert(name, value);
        Ok(())
    }

    /// Copy these headers into an execution-owned header map. Every copied
    /// value is explicitly re-marked sensitive before replacing the target's
    /// value for the same name.
    pub fn apply_to(&self, target: &mut HeaderMap) {
        for (name, value) in &self.headers {
            let mut value = value.clone();
            value.set_sensitive(true);
            target.insert(name.clone(), value);
        }
    }
}

impl Clone for SecretRequestHeaders {
    fn clone(&self) -> Self {
        let mut cloned = Self::new();
        self.apply_to(&mut cloned.headers);
        cloned
    }
}

impl std::fmt::Debug for SecretRequestHeaders {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretRequestHeaders")
            .field("count", &self.len())
            .finish()
    }
}

/// Secret-safe structural header parsing failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretHeaderError {
    /// Header name does not satisfy HTTP header-name grammar.
    InvalidHeaderName,
    /// Header value does not satisfy HTTP header-value grammar.
    InvalidHeaderValue,
}

impl std::fmt::Display for SecretHeaderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHeaderName => formatter.write_str("invalid secret request header name"),
            Self::InvalidHeaderValue => formatter.write_str("invalid secret request header value"),
        }
    }
}

impl std::error::Error for SecretHeaderError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_len_and_is_empty_are_deterministic() {
        let headers = SecretRequestHeaders::new();
        assert!(headers.is_empty());
        assert_eq!(headers.len(), 0);
        assert_eq!(format!("{headers:?}"), "SecretRequestHeaders { count: 0 }");
    }

    #[test]
    fn insertion_replaces_and_automatically_marks_values_sensitive() {
        let mut headers = SecretRequestHeaders::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Bearer first-secret"),
        );
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Bearer replacement-secret"),
        );

        let mut applied = HeaderMap::new();
        headers.apply_to(&mut applied);
        assert_eq!(headers.len(), 1);
        let value = applied.get("authorization").unwrap();
        assert!(value.is_sensitive());
        assert_eq!(value.as_bytes(), b"Bearer replacement-secret");
    }

    #[test]
    fn different_secret_headers_coexist_and_debug_redacts_every_value() {
        const SENTINELS: [&str; 3] = [
            "Bearer authorization-secret",
            "session=cookie-secret",
            "api-key-secret",
        ];
        let mut headers = SecretRequestHeaders::new();
        headers.try_insert("authorization", SENTINELS[0]).unwrap();
        headers.try_insert("cookie", SENTINELS[1]).unwrap();
        headers.try_insert("x-api-key", SENTINELS[2]).unwrap();

        let debug = format!("{headers:?}");
        assert_eq!(headers.len(), 3);
        assert_eq!(debug, "SecretRequestHeaders { count: 3 }");
        for sentinel in SENTINELS {
            assert!(!debug.contains(sentinel));
            assert!(!debug.contains(sentinel.split('-').next().unwrap()));
        }
    }

    #[test]
    fn clone_and_application_preserve_sensitive_marking() {
        let mut original = SecretRequestHeaders::new();
        original.try_insert("x-secret", "clone-secret").unwrap();
        let cloned = original.clone();
        let mut target = HeaderMap::new();
        target.insert("x-secret", HeaderValue::from_static("old-plain-value"));
        cloned.apply_to(&mut target);

        let value = target.get("x-secret").unwrap();
        assert!(value.is_sensitive());
        assert_eq!(value.as_bytes(), b"clone-secret");
        assert!(!format!("{cloned:?}").contains("clone-secret"));
    }

    #[test]
    fn parse_errors_never_echo_names_or_secret_values() {
        const SECRET: &str = "invalid\nsecret-value-sentinel";
        let mut headers = SecretRequestHeaders::new();
        let invalid_name = headers
            .try_insert("invalid header name", SECRET)
            .unwrap_err();
        assert_eq!(invalid_name, SecretHeaderError::InvalidHeaderName);
        let invalid_value = headers.try_insert("authorization", SECRET).unwrap_err();
        assert_eq!(invalid_value, SecretHeaderError::InvalidHeaderValue);

        for error in [invalid_name, invalid_value] {
            assert!(!format!("{error:?}").contains(SECRET));
            assert!(!error.to_string().contains(SECRET));
            assert!(!error.to_string().contains("authorization"));
        }
        assert!(headers.is_empty());
    }
}
