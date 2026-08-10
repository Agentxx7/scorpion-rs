//! Manual/request-supplied onion seed discovery.
//!
//! Normalizes caller-supplied `.onion` URLs into canonical
//! [`SourceItem`] candidates using the same canonical `.onion`
//! classifier Tor acquisition already relies on
//! ([`crate::features::transport::is_onion_url`]) — never a second,
//! independently maintained `.ends_with(".onion")` check.
//!
//! **Discovery != acquisition.** This module performs *zero* target
//! acquisition: no HTTP request, no Tor/SOCKS connection, no DNS lookup,
//! no filesystem access, no persistence. It never constructs a
//! `Page`, an `EvidenceBundle`, or a `reqwest`/`TransportPolicy` client.
//! A candidate produced here is a *location*, not evidence that
//! anything was ever reached — binding a candidate to an actual Tor
//! fetch is future orchestration's job (outside this crate), not this
//! module's. See [`crate::features::transport`]'s module docs for the
//! acquisition-side half of that boundary.
//!
//! Available unconditionally — classifying a URL string requires no
//! network feature. Actual Tor acquisition remains gated behind the
//! `transport_tor` feature exactly as before; this module changes
//! nothing about that.
//!
//! **Credentials are rejected outright, never stripped and continued**
//! (matching `TorTransportConfig::new`'s existing "no credential
//! support" contract for the Tor proxy endpoint) — a seed carrying
//! `user:pass@`/`user@` fails closed with [`OnionSeedError::CredentialsNotAllowed`],
//! producing no candidate at all.
//!
//! **No `OnionSeedError` variant retains or echoes the supplied seed
//! string.** A seed may contain a password, a query-string token, or a
//! fragment secret — this module's error type is deliberately shaped so
//! none of that can leak into a `Debug`/`Display`/log/serialized error.
//! The only payload any variant carries is [`OnionSeedError::UnsupportedScheme`]'s
//! *parsed scheme* (`"ftp"`, `"mailto"`, …) — never the URL it came from.

use crate::features::source::SourceItem;
use crate::features::transport::is_onion_url;
use std::fmt;

/// Stable adapter-family tag for [`SourceItem::source_type`], matching
/// the short lowercase convention already used by `"feed"`/`"sitemap"`.
const SOURCE_TYPE: &str = "onion_seed";

/// Why a supplied seed was rejected. Deliberately distinct from
/// [`crate::features::transport::TransportError`] — nothing here
/// represents a network/transport failure; every variant is a pure URL
/// classification outcome, decided before any acquisition could even be
/// attempted.
///
/// **Secret-safe by construction**: no variant carries the supplied seed
/// string, or any substring of it that could contain userinfo, a query
/// token, or a fragment. [`OnionSeedError::UnsupportedScheme`] is the
/// only variant with a payload, and that payload is exactly the parsed
/// `url::Url::scheme()` value — a short, fixed vocabulary token
/// (`"ftp"`, `"mailto"`, …), never attacker/operator-supplied secret
/// material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnionSeedError {
    /// The seed did not parse as a canonical URL at all (includes a
    /// missing/empty host, since `url::Url` itself rejects that for
    /// `http`/`https`).
    InvalidUrl,
    /// The seed parsed, but its scheme is not `http` or `https` — the
    /// only schemes a future web-acquisition orchestrator can act on.
    /// Carries only the parsed scheme token, never the full URL.
    UnsupportedScheme(UnsupportedScheme),
    /// The seed carries userinfo (`user:pass@` or `user@`). Rejected
    /// outright — never stripped and silently continued — matching the
    /// "no credential support" contract already established for the Tor
    /// proxy endpoint (`TorTransportConfig::new`).
    CredentialsNotAllowed,
    /// The seed parsed with a supported scheme and no credentials, but
    /// its host is not a `.onion` hostname per [`is_onion_url`].
    NotOnion,
}

/// The rejected scheme, as a short fixed-vocabulary token — never the
/// full URL. Wrapping this (rather than a bare `String`) makes it
/// structurally impossible to accidentally widen this payload to carry
/// more than a scheme name later without a deliberate, visible type
/// change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedScheme(String);

impl fmt::Display for UnsupportedScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for OnionSeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OnionSeedError::InvalidUrl => write!(f, "onion seed is not a valid URL"),
            OnionSeedError::UnsupportedScheme(scheme) => write!(
                f,
                "onion seed scheme \"{scheme}\" is not supported — only http/https are accepted"
            ),
            OnionSeedError::CredentialsNotAllowed => write!(
                f,
                "onion seed must not include userinfo/credentials (user:pass@) — rejected \
                 outright, never stripped and continued"
            ),
            OnionSeedError::NotOnion => {
                write!(f, "onion seed host is not a .onion hostname")
            }
        }
    }
}

impl std::error::Error for OnionSeedError {}

/// Normalize exactly one caller-supplied onion seed URL into a candidate
/// [`SourceItem`]. Pure, network-independent classification — see the
/// module docs for the full zero-acquisition guarantee.
///
/// Validation order (each step decided before the next runs, and none of
/// it can leak the seed into an error — see [`OnionSeedError`]):
///
/// 1. The URL must parse canonically (`url::Url::parse`); a missing host
///    is one way that fails, since `http`/`https` require an authority.
/// 2. The scheme must be `http` or `https`.
/// 3. Userinfo (`user:pass@`/`user@`) must be absent — a credential-
///    bearing seed is rejected outright, never stripped and continued.
/// 4. The host must be `.onion` per [`is_onion_url`] — case-insensitive,
///    exact suffix match only (`abc.onion.example.com` is rejected;
///    `is_onion_url` is the single canonical seam, never reimplemented
///    here).
/// 5. The accepted URL is normalized into a candidate.
///
/// A manually supplied seed carries no provider-native identifier — the
/// URL is a candidate *location*, not proof of a source-native ID — so
/// [`SourceItem::source_item_id`] is always `None`. It also has no
/// containing source document (no feed/sitemap declared it — the caller
/// named it directly), so [`SourceItem::discovered_via`] is always
/// `None`, never a self-reference to `url` or an invented placeholder.
/// No other field not truthfully supplied by the seed (title, author,
/// date, snippet, content, evidence, HTTP status, transport provenance)
/// is populated.
pub fn normalize_onion_seed(seed: &str) -> Result<SourceItem, OnionSeedError> {
    let url = url::Url::parse(seed).map_err(|_| OnionSeedError::InvalidUrl)?;

    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(OnionSeedError::UnsupportedScheme(UnsupportedScheme(
                other.to_string(),
            )))
        }
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(OnionSeedError::CredentialsNotAllowed);
    }

    if !is_onion_url(&url) {
        return Err(OnionSeedError::NotOnion);
    }

    Ok(SourceItem {
        source_type: SOURCE_TYPE.to_string(),
        source_item_id: None,
        url: Some(url.as_str().to_string()),
        title: None,
        snippet: None,
        authors: Vec::new(),
        published_at: None,
        updated_at: None,
        // No enclosing document declared this candidate — the caller
        // named it directly, so there is truthfully no "discovered via"
        // source. Never a self-reference to `url`.
        discovered_via: None,
        media_references: Vec::new(),
    })
}

/// Normalize a caller-supplied batch of onion seed URLs. Caller order and
/// duplicates are both preserved exactly — this module never
/// deduplicates or reorders candidates, and never invents a ranking. One
/// `Result` per input seed, in input order, so per-seed acceptance or
/// rejection is always traceable to its original position; one invalid
/// seed never discards the others.
pub fn normalize_onion_seeds<I, S>(seeds: I) -> Vec<Result<SourceItem, OnionSeedError>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    seeds
        .into_iter()
        .map(|seed| normalize_onion_seed(seed.as_ref()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const USERNAME_SENTINEL: &str = "sekretuser12345";
    const PASSWORD_SENTINEL: &str = "hunter2sekretpw";
    const QUERY_SENTINEL: &str = "sekrettoken98765";
    const FRAGMENT_SENTINEL: &str = "sekretfrag24680";

    fn ok_url(result: &Result<SourceItem, OnionSeedError>) -> &str {
        result.as_ref().unwrap().url.as_deref().unwrap()
    }

    /// No `Debug`/`Display` rendering of any [`OnionSeedError`] variant,
    /// across a battery of secret-bearing inputs, ever contains any of
    /// the sentinel secrets planted in those inputs. This is the direct,
    /// structural proof Section C requires: not "we believe it's safe"
    /// but "we planted secrets and grepped for them."
    fn assert_error_never_leaks_sentinels(error: &OnionSeedError) {
        let debug = format!("{error:?}");
        let display = format!("{error}");
        for sentinel in [
            USERNAME_SENTINEL,
            PASSWORD_SENTINEL,
            QUERY_SENTINEL,
            FRAGMENT_SENTINEL,
        ] {
            assert!(
                !debug.contains(sentinel),
                "Debug leaked a sentinel secret: {debug:?}"
            );
            assert!(
                !display.contains(sentinel),
                "Display leaked a sentinel secret: {display:?}"
            );
        }
    }

    /// 1. Valid onion root URL accepted.
    #[test]
    fn valid_onion_root_url_accepted() {
        let item = normalize_onion_seed("http://abc234567890defg.onion/").unwrap();
        assert_eq!(item.url.as_deref(), Some("http://abc234567890defg.onion/"));
        assert_eq!(item.source_type, "onion_seed");
    }

    /// 2. Valid onion path accepted.
    #[test]
    fn valid_onion_path_accepted() {
        let item = normalize_onion_seed("http://abc.onion/forum/thread/1").unwrap();
        assert_eq!(item.url.as_deref(), Some("http://abc.onion/forum/thread/1"));
    }

    /// 3. Valid onion query accepted, and fragment is preserved as
    ///    canonical URL semantics dictate.
    #[test]
    fn valid_onion_query_and_fragment_accepted() {
        let item = normalize_onion_seed("http://abc.onion/search?q=test#section").unwrap();
        assert_eq!(
            item.url.as_deref(),
            Some("http://abc.onion/search?q=test#section")
        );
    }

    /// 4. Clearnet rejected.
    #[test]
    fn clearnet_url_rejected() {
        assert_eq!(
            normalize_onion_seed("https://example.com/").unwrap_err(),
            OnionSeedError::NotOnion
        );
    }

    /// 5. Malformed URL rejected.
    #[test]
    fn malformed_url_rejected() {
        assert_eq!(
            normalize_onion_seed("not a url").unwrap_err(),
            OnionSeedError::InvalidUrl
        );
    }

    /// 6. Hostname containing ".onion" but not ending as onion rejected
    ///    (the classic suffix trick).
    #[test]
    fn onion_substring_not_suffix_rejected() {
        assert_eq!(
            normalize_onion_seed("http://abc.onion.example.com/").unwrap_err(),
            OnionSeedError::NotOnion
        );
    }

    /// 7. Onion-like suffix trick — a host that merely contains "onion"
    ///    as a substring or near-miss suffix, never a real `.onion` suffix.
    #[test]
    fn onion_lookalike_suffix_rejected() {
        for seed in [
            "http://fakeonion/",
            "http://abc.onion.evil.test/",
            "http://abc-onion.test/",
        ] {
            assert_eq!(
                normalize_onion_seed(seed).unwrap_err(),
                OnionSeedError::NotOnion,
                "{seed} must be rejected as not onion"
            );
        }
    }

    /// 8. Uppercase host representation is normalized (by `url::Url`
    ///    itself) and still correctly recognized as onion, with the stored
    ///    URL reflecting the normalized (lowercased) host.
    #[test]
    fn uppercase_host_is_normalized_and_accepted() {
        let item = normalize_onion_seed("HTTP://ABC.ONION/Path").unwrap();
        assert_eq!(item.url.as_deref(), Some("http://abc.onion/Path"));
    }

    /// 9. Missing host rejected.
    #[test]
    fn missing_host_rejected() {
        assert_eq!(
            normalize_onion_seed("http://").unwrap_err(),
            OnionSeedError::InvalidUrl
        );
    }

    /// 10. Unsupported scheme rejected, even for an otherwise-onion host —
    ///     scheme is checked before onion-ness. The payload is exactly the
    ///     parsed scheme token, nothing else.
    #[test]
    fn unsupported_scheme_rejected() {
        let error = normalize_onion_seed("ftp://abc.onion/").unwrap_err();
        assert_eq!(
            error,
            OnionSeedError::UnsupportedScheme(UnsupportedScheme("ftp".to_string()))
        );
        assert!(error.to_string().contains("ftp"));
    }

    /// B: username-only seed rejected — fail closed, no candidate.
    #[test]
    fn username_only_seed_rejected() {
        assert_eq!(
            normalize_onion_seed(&format!("http://{USERNAME_SENTINEL}@abc.onion/")).unwrap_err(),
            OnionSeedError::CredentialsNotAllowed
        );
    }

    /// B: username+password seed rejected — fail closed, no candidate.
    #[test]
    fn username_and_password_seed_rejected() {
        assert_eq!(
            normalize_onion_seed(&format!(
                "http://{USERNAME_SENTINEL}:{PASSWORD_SENTINEL}@abc.onion/"
            ))
            .unwrap_err(),
            OnionSeedError::CredentialsNotAllowed
        );
    }

    /// B: credential seed with path/query rejected — the whole seed is
    /// rejected, not just the credentials silently dropped.
    #[test]
    fn credential_seed_with_path_and_query_rejected() {
        assert_eq!(
            normalize_onion_seed(&format!(
                "https://{USERNAME_SENTINEL}:{PASSWORD_SENTINEL}@abc.onion/forum?q={QUERY_SENTINEL}#{FRAGMENT_SENTINEL}"
            ))
            .unwrap_err(),
            OnionSeedError::CredentialsNotAllowed
        );
    }

    /// C: the CredentialsNotAllowed error itself never contains the
    /// planted username/password/query/fragment sentinels — proven via
    /// Debug and Display, not by inspection.
    #[test]
    fn credentials_error_does_not_contain_username_sentinel() {
        let error =
            normalize_onion_seed(&format!("http://{USERNAME_SENTINEL}@abc.onion/")).unwrap_err();
        assert_error_never_leaks_sentinels(&error);
    }

    #[test]
    fn credentials_error_does_not_contain_password_sentinel() {
        let error = normalize_onion_seed(&format!("http://user:{PASSWORD_SENTINEL}@abc.onion/"))
            .unwrap_err();
        assert_error_never_leaks_sentinels(&error);
    }

    #[test]
    fn credentials_error_does_not_contain_query_token_sentinel() {
        let error = normalize_onion_seed(&format!(
            "http://user:pass@abc.onion/?token={QUERY_SENTINEL}#{FRAGMENT_SENTINEL}"
        ))
        .unwrap_err();
        assert_error_never_leaks_sentinels(&error);
    }

    /// C (7): a clearnet URL carrying credentials and secrets must not
    /// fall through to `NotOnion` (or any variant) that echoes the raw
    /// input — `CredentialsNotAllowed` fires first, and carries nothing.
    #[test]
    fn clearnet_credential_seed_does_not_echo_raw_url_or_secrets() {
        let seed = format!(
            "https://{USERNAME_SENTINEL}:{PASSWORD_SENTINEL}@example.com/path?token={QUERY_SENTINEL}#{FRAGMENT_SENTINEL}"
        );
        let error = normalize_onion_seed(&seed).unwrap_err();
        assert_eq!(error, OnionSeedError::CredentialsNotAllowed);
        assert_error_never_leaks_sentinels(&error);
    }

    /// C: an ordinary clearnet `NotOnion` rejection (no credentials
    /// involved) also never echoes the raw URL — proven with a seed that
    /// still carries a query-string sentinel, to catch any accidental
    /// "helpfully include the URL in the error" regression.
    #[test]
    fn clearnet_not_onion_error_does_not_echo_raw_url_or_secrets() {
        let seed = format!("https://example.com/?token={QUERY_SENTINEL}#{FRAGMENT_SENTINEL}");
        let error = normalize_onion_seed(&seed).unwrap_err();
        assert_eq!(error, OnionSeedError::NotOnion);
        assert_error_never_leaks_sentinels(&error);
    }

    /// 11. Multiple seeds preserve caller order.
    #[test]
    fn multiple_seeds_preserve_caller_order() {
        let results =
            normalize_onion_seeds(["http://a.onion/", "http://b.onion/", "http://c.onion/"]);
        assert_eq!(results.len(), 3);
        assert_eq!(ok_url(&results[0]), "http://a.onion/");
        assert_eq!(ok_url(&results[1]), "http://b.onion/");
        assert_eq!(ok_url(&results[2]), "http://c.onion/");
    }

    /// 12. Duplicates are preserved verbatim — no deduplication, no
    ///     invented ranking.
    #[test]
    fn duplicate_seeds_are_preserved() {
        let results =
            normalize_onion_seeds(["http://a.onion/", "http://a.onion/", "http://a.onion/"]);
        assert_eq!(results.len(), 3);
        for result in &results {
            assert_eq!(ok_url(result), "http://a.onion/");
        }
    }

    /// 13. Mixed valid/invalid input reports truthfully per seed — one
    ///     invalid seed does not discard the others, and alignment
    ///     between input position and result is preserved.
    #[test]
    fn mixed_valid_and_invalid_input_reports_per_seed() {
        let results = normalize_onion_seeds([
            "http://a.onion/",
            "https://example.com/",
            "not a url",
            "http://b.onion/path",
        ]);
        assert_eq!(results.len(), 4);
        assert!(results[0].is_ok());
        assert_eq!(results[1], Err(OnionSeedError::NotOnion));
        assert_eq!(results[2], Err(OnionSeedError::InvalidUrl));
        assert!(results[3].is_ok());
        assert_eq!(ok_url(&results[0]), "http://a.onion/");
        assert_eq!(ok_url(&results[3]), "http://b.onion/path");
    }

    /// 14. No evidence/provenance fabricated: every field not truthfully
    ///     supplied by a bare URL stays empty/`None`, including
    ///     `source_item_id` — a manual URL is a candidate location, not
    ///     proof of a native source ID — and `discovered_via`, which has
    ///     no containing source document to point to.
    #[test]
    fn no_evidence_or_identity_is_fabricated() {
        let item = normalize_onion_seed("http://abc.onion/page").unwrap();
        assert_eq!(item.source_item_id, None);
        assert_eq!(item.title, None);
        assert_eq!(item.snippet, None);
        assert!(item.authors.is_empty());
        assert_eq!(item.published_at, None);
        assert_eq!(item.updated_at, None);
        assert!(item.media_references.is_empty());
    }

    /// 8/D: manual seed `discovered_via` is genuinely absent — never the
    /// candidate's own URL as a self-reference, never an empty string,
    /// never a "manual://" placeholder.
    #[test]
    fn manual_seed_discovered_via_is_absent() {
        let item = normalize_onion_seed("http://abc.onion/page").unwrap();
        assert_eq!(item.discovered_via, None);
    }

    /// This module's produced type is exactly the existing `SourceItem`
    /// — no `OnionSourceItem`/`ResearchCandidate` parallel model.
    #[test]
    fn produces_the_existing_source_item_type_directly() {
        let _: SourceItem = normalize_onion_seed("http://abc.onion/").unwrap();
    }

    /// K: distinct onion services are independent candidates in the same
    /// list — this module never applies same-onion crawl-boundary
    /// restrictions (those exist only once an actual crawl starts).
    #[test]
    fn distinct_onion_services_are_independent_candidates() {
        let results = normalize_onion_seeds([
            "http://onion-a.onion/",
            "http://onion-b.onion/",
            "http://onion-c.onion/",
        ]);
        assert!(results.iter().all(|r| r.is_ok()));
        let hosts: Vec<&str> = results
            .iter()
            .map(|r| r.as_ref().unwrap().url.as_deref().unwrap())
            .collect();
        assert_eq!(
            hosts,
            [
                "http://onion-a.onion/",
                "http://onion-b.onion/",
                "http://onion-c.onion/"
            ]
        );
    }

    /// H/J (zero acquisition): a hostile/unresolvable `.onion` host is
    /// accepted as a candidate exactly like any other — no attempt to
    /// contact it happens, since there is nothing in this module capable
    /// of doing so. This is proven structurally (module imports: no
    /// `reqwest`/`tokio` networking/socket/DNS type anywhere in this
    /// file; `normalize_onion_seed`'s signature is synchronous and takes
    /// no client), not by timing a wall clock — timing assertions are
    /// flaky and prove nothing a determined caller couldn't fake with a
    /// fast-failing mock. This test only proves the *functional*
    /// contract: acceptance for a hostile host is identical to
    /// acceptance for any other syntactically valid onion host.
    #[test]
    fn hostile_unreachable_host_is_classified_not_contacted() {
        let item = normalize_onion_seed(
            "http://thishostwillneverresolveorconnectxxxxxxxxxxxxxxxxxxxxxxxxxxxx.onion/",
        )
        .unwrap();
        assert_eq!(
            item.url.as_deref(),
            Some("http://thishostwillneverresolveorconnectxxxxxxxxxxxxxxxxxxxxxxxxxxxx.onion/")
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serializes_through_source_item_wire_shape_unchanged() {
        let item = normalize_onion_seed("http://abc.onion/page").unwrap();
        let value = serde_json::to_value(&item).unwrap();
        assert_eq!(value["source_type"], "onion_seed");
        assert_eq!(value["url"], "http://abc.onion/page");
        assert_eq!(value["source_item_id"], serde_json::Value::Null);
        assert_eq!(value["discovered_via"], serde_json::Value::Null);
        for forbidden in [
            "retrieved_at",
            "status_code",
            "observed_status_code",
            "response_body_hash",
            "screenshot",
            "evidence",
        ] {
            assert!(
                value.get(forbidden).is_none(),
                "unexpected field {forbidden}"
            );
        }
    }

    /// Errors carry no un-typed string beyond `UnsupportedScheme`'s
    /// scheme token — `Debug`-formatting every variant produced by a
    /// battery of secret-bearing inputs never contains any planted
    /// sentinel, proven across the whole surface at once (not just the
    /// individually-targeted tests above).
    #[test]
    fn no_error_variant_leaks_seed_secrets_across_a_battery_of_inputs() {
        let seeds = [
            format!("http://{USERNAME_SENTINEL}@abc.onion/"),
            format!("http://{USERNAME_SENTINEL}:{PASSWORD_SENTINEL}@abc.onion/"),
            format!(
                "https://{USERNAME_SENTINEL}:{PASSWORD_SENTINEL}@example.com/?t={QUERY_SENTINEL}#{FRAGMENT_SENTINEL}"
            ),
            format!("https://example.com/?t={QUERY_SENTINEL}#{FRAGMENT_SENTINEL}"),
            format!("ftp://{USERNAME_SENTINEL}@abc.onion/"),
            format!("not a url with {PASSWORD_SENTINEL} in it"),
        ];
        for seed in seeds {
            if let Err(error) = normalize_onion_seed(&seed) {
                assert_error_never_leaks_sentinels(&error);
            }
        }
    }
}
