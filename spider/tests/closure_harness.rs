//! `SCORPION_CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_001`.
//!
//! Verifies `docs/frontier/ledger/*.toml` capability-lifecycle claims
//! against reality rather than trusting them. Each ledger entry claims a
//! stage (DESIGNED -> IMPLEMENTED -> VERIFIED -> WIRED ->
//! PRODUCTION_REACHABLE -> ADVERSARIALLY_VERIFIED -> CI_ENFORCED ->
//! CLOSED); this file independently recomputes how far the evidence
//! actually reaches and fails if a claim outruns it. See
//! `docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md`
//! for the full design.
//!
//! This file was hardened against a documented adversarial-review pass
//! (an external reviewer, "Kimi") that falsified the first version: text
//! presence anywhere in a file (including inside a comment), a symbol
//! chain with no proof the caller's body actually calls the callee, a
//! generic entry-point symbol standing in for any capability, a
//! blob/tree SHA accepted as a commit, and a workflow *comment* mentioning
//! a command name all previously satisfied checks that should have
//! rejected them. Every fix below is paired with a doc comment explaining
//! the exact bypass it closes.
//!
//! Run with:
//! `cargo test -p spider --test closure_harness --features "chrome cache cache_request"`

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use syn::{visit::Visit, ExprCall, ExprMethodCall, ImplItem, Item, ItemFn, ItemImpl, Type};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spider crate must be inside workspace")
        .to_path_buf()
}

/// Override point for `spider/tests/closure_harness_behavioral_contract.rs`
/// (an independent test binary, not this file) to point the *real*
/// verifier at a temporary directory of deliberately-invalid fixtures,
/// spawned as a real subprocess, and assert it fails. Unset in every
/// normal run, including CI — defaults to the real ledger.
fn ledger_dir() -> PathBuf {
    match std::env::var("CLOSURE_HARNESS_LEDGER_DIR_OVERRIDE") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => workspace_root().join("docs/frontier/ledger"),
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

// =====================================================================
// Comment-aware source analysis. Bypass class closed: "a comment must
// not satisfy an implementation/call/root symbol" — the prior version
// used plain `contents.contains(symbol)`, satisfiable by a doc comment
// mentioning the symbol's name in prose.
// =====================================================================

/// Strips a trailing `//` line comment, respecting simple double-quoted
/// string literals (so a URL like `"https://x"` is not truncated at the
/// `//`). Does not handle raw strings or multi-line block comments — a
/// deliberate, documented limitation given this codebase's near-universal
/// use of plain double-quoted strings and `//`/`///`/`//!` comments
/// (confirmed by inspection throughout this frontier's work), not a
/// general-purpose Rust lexer.
fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 2,
            b'"' => {
                in_string = !in_string;
                i += 1;
            }
            b'/' if !in_string && i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                return &line[..i];
            }
            _ => i += 1,
        }
    }
    line
}

/// The file's content with every line comment stripped. All symbol
/// presence/definition/call checks below run against this, never the raw
/// source, so a comment can never satisfy them.
fn code_only(source: &str) -> String {
    source
        .lines()
        .map(strip_line_comment)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The portion of a file's comment-stripped code *before* its first test
/// module — this codebase's near-universal convention (confirmed
/// throughout this session) is `#[cfg(test)] mod tests { ... }` or
/// `#[test]`/`#[tokio::test]` blocks at the end of the file. Bypass class
/// closed: "test files/comments cannot satisfy production-root evidence"
/// — a symbol that only appears inside a test module is not a production
/// definition, even if it's a real, non-comment function.
#[allow(dead_code)]
fn production_code_only(source: &str) -> String {
    let code = code_only(source);
    let cut = [
        code.find("#[cfg(test)]"),
        code.find("\nmod tests"),
        code.find("\nmod test "),
    ]
    .into_iter()
    .flatten()
    .min();
    match cut {
        Some(index) => code[..index].to_string(),
        None => code,
    }
}

/// Whether `code` (comment-stripped) contains a genuine Rust *definition*
/// of `symbol` (bare name — a `Type::method` qualifier is stripped, since
/// Rust defines methods as `fn method(...)` inside `impl Type { ... }`,
/// never spelled `Type::method` at the definition site). Bypass class
/// closed: "IMPLEMENTED evidence must identify the actual implementation,
/// not merely exist somewhere in the file" — the prior version accepted
/// any raw text occurrence, including inside a string literal or comment
/// mentioning the name.
#[allow(dead_code)]
fn contains_definition(code: &str, symbol: &str) -> bool {
    let bare = symbol.rsplit("::").next().unwrap_or(symbol);
    [
        format!("fn {bare}("),
        format!("fn {bare}<"),
        format!("struct {bare}"),
        format!("enum {bare}"),
        format!("trait {bare}"),
        format!("type {bare}"),
        format!("const {bare}"),
        format!("static {bare}"),
    ]
    .iter()
    .any(|pattern| code.contains(pattern.as_str()))
}

fn attrs_have_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| match &a.meta {
        syn::Meta::List(list) if list.path.is_ident("cfg") => {
            list.tokens.to_string().contains("test")
        }
        _ => false,
    })
}

fn is_test_attributed(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test") || a.path().segments.last().is_some_and(|s| s.ident == "test")
    })
}

/// Splits a symbol into its optional `Type::` qualifier and bare trailing
/// name — `"Page::new_streaming"` -> `(Some("Page"), "new_streaming")`,
/// `"crawl_establish"` -> `(None, "crawl_establish")`.
fn split_type_qualifier(symbol: &str) -> (Option<&str>, &str) {
    match symbol.rsplit_once("::") {
        Some((prefix, bare)) => (Some(prefix.rsplit("::").next().unwrap_or(prefix)), bare),
        None => (None, symbol),
    }
}

// =====================================================================
// Canonical `Website` identity (Codex adversarial review, round 5 —
// "maturity-bearing canonical identity is still inferred from syntactic
// names... same identifier != same canonical symbol"). Every prior round
// of this hardening effort (rounds 2-4) still ultimately asked "does
// this name/shape look right" — a locally-shadowed struct is untrusted,
// a badly-qualified path is untrusted, a bare unqualified path is
// trusted *by default* unless something specifically disqualifies it.
// That "innocent until proven guilty" posture is exactly what kept
// producing new reproducers: whatever specific disqualifying shape this
// harness didn't yet check for (a same-file local struct with no `use`
// import at all; a `mod Website { fn crawl() {} }` masquerading as an
// associated call; a locally-shadowed `macro_rules! join`) trivially
// walked through, because the model's default answer was "trust it."
//
// This section replaces that with the opposite posture: canonical
// `Website` identity is proven affirmatively, from a small, fixed,
// exhaustively-enumerated set of *known-good* forms, or it is
// NOT_PROVEN — no absence-of-a-red-flag reasoning anywhere. The ground
// truth this harness treats as given (not derived) is exactly one fact:
// the real `Website` type is defined at `spider/src/website.rs`. Every
// other reference to it — a bare name used elsewhere, a qualified path,
// an `impl` block's self_ty — must trace back to that one fact through
// one of exactly two provable channels:
//   1. the code lives *in* `spider/src/website.rs` itself (the
//      definition site — trivially "the real one," since it's where
//      `Website` is defined, not referenced);
//   2. the code lives elsewhere and has a `use` import (or writes an
//      inline path) whose *complete*, fully-qualified text is *exactly*
//      one of the finite legitimate spellings for that file's own
//      "world" — `crate::website::Website` from inside the `spider`
//      crate itself, `spider::website::Website` from a known external
//      shipping-artifact crate (`spider_cli`/`spider_mcp`/
//      `spider_worker`, which depend on the published `spider` crate).
// Every other spelling — `crate::decoy::Website`, `self::decoy::Website`,
// `super::decoy::Website`, `external::Website`, a locally-defined
// `struct Website`/`mod Website` with no matching import at all, a
// rename (`pub use crate::decoy::Other as Website;`) — is not on that
// list and therefore proves nothing, regardless of how closely it
// resembles a trusted shape. This deliberately does not attempt
// `self::`/`super::`-relative resolution (which would require knowing a
// referencing file's own module depth relative to crate root — genuine
// name resolution, which a single isolated file's `syn::File` cannot
// provide): "do not build a rust compiler." No real code in this
// repository uses either form to reach `Website` (grep-confirmed), so
// this costs no real coverage.
// =====================================================================

/// The one real definition site of the canonical `Website` type — the
/// single fact this harness treats as ground truth.
const CANONICAL_WEBSITE_DEFINITION_FILE: &str = "spider/src/website.rs";

/// The finite, exhaustively-enumerated set of complete path spellings
/// that legitimately name the canonical `Website` type, depending on
/// which "world" (crate) the referencing file lives in.
fn canonical_website_paths(is_spider_own_crate_source: bool) -> &'static [&'static str] {
    if is_spider_own_crate_source {
        &["crate::website::Website"]
    } else {
        &["spider::website::Website"]
    }
}

/// Whether a *bare*, unqualified `Website` reference anywhere in `file`
/// is affirmatively proven to be the canonical type — either `file` *is*
/// the canonical definition site, or `file` contains a `use` import
/// whose complete, fully-qualified source path (the path *before* any
/// `as` rename) is exactly one of `canonical_website_paths`. A rename
/// only matters when its *target* name is `Website` (only then does the
/// bare name `Website` refer to this import at all); the *source* path
/// being renamed must still itself be a canonical spelling — `pub use
/// crate::decoy::Other as Website;` renames a wholly unrelated path,
/// which is exactly as unproven as `use crate::decoy::Website;` would
/// be. A glob import can never establish this proof (its brought-in
/// names cannot be enumerated structurally, so it is never affirmative
/// evidence of anything).
fn file_proves_bare_website(
    file: &syn::File,
    is_canonical_definition_site: bool,
    is_spider_own_crate_source: bool,
) -> bool {
    if is_canonical_definition_site {
        return true;
    }
    let canonical_paths = canonical_website_paths(is_spider_own_crate_source);
    fn use_tree_proves_website(
        tree: &syn::UseTree,
        prefix: &mut Vec<String>,
        canonical_paths: &[&str],
    ) -> bool {
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                let proven = use_tree_proves_website(&p.tree, prefix, canonical_paths);
                prefix.pop();
                proven
            }
            syn::UseTree::Name(n) => {
                n.ident == CANONICAL_PRODUCTION_TYPE && {
                    let mut full = prefix.clone();
                    full.push(n.ident.to_string());
                    canonical_paths.contains(&full.join("::").as_str())
                }
            }
            syn::UseTree::Rename(r) => {
                r.rename == CANONICAL_PRODUCTION_TYPE && {
                    let mut full = prefix.clone();
                    full.push(r.ident.to_string());
                    canonical_paths.contains(&full.join("::").as_str())
                }
            }
            syn::UseTree::Group(g) => g
                .items
                .iter()
                .any(|t| use_tree_proves_website(t, prefix, canonical_paths)),
            syn::UseTree::Glob(_) => false,
        }
    }
    file.items.iter().any(|item| match item {
        Item::Use(use_item) => {
            let mut prefix = Vec::new();
            use_tree_proves_website(&use_item.tree, &mut prefix, canonical_paths)
        }
        _ => false,
    })
}

/// The single shared canonical-identity resolution consumed by every
/// maturity-bearing path in this harness: `ast_contains_production_definition`'s
/// owner tracking, `ast_function_calls`'s strict and vendor-permissive
/// receiver/associated-call resolution, and `ast_any_production_call`'s
/// self-receiver/associated-call/constructor binding. `idents` is a
/// path's segments, already stripped of any trailing non-type suffix
/// (a method/associated-function name) by the caller.
///
/// For any type name other than the canonical `Website`, this resolves
/// only a bare, single-segment path — this harness has no canonical-site
/// concept for any other type, and ledger evidence never names one
/// through a qualified `impl` self_ty either, so a qualified form is
/// exactly as unsupported (`None`) as it is for `Website`.
///
/// For `Website` specifically, a bare single-segment path is trusted
/// only when `bare_website_trusted` (computed once per file via
/// `file_proves_bare_website`) says so; a qualified path is trusted only
/// when its complete text is exactly one of `canonical_website_paths`.
/// There is no other path to trust — same identifier is never treated as
/// same canonical symbol.
fn canonical_type_owner_name(
    idents: &[String],
    is_spider_own_crate_source: bool,
    bare_website_trusted: bool,
) -> Option<String> {
    if idents.len() == 1 {
        let name = idents[0].clone();
        if name == CANONICAL_PRODUCTION_TYPE {
            return bare_website_trusted.then_some(name);
        }
        return Some(name);
    }
    let joined = idents.join("::");
    canonical_website_paths(is_spider_own_crate_source)
        .contains(&joined.as_str())
        .then(|| CANONICAL_PRODUCTION_TYPE.to_string())
}

/// `path`'s segments as owned identifier strings, in order.
fn path_idents(path: &syn::Path) -> Vec<String> {
    path.segments.iter().map(|s| s.ident.to_string()).collect()
}

/// `ty`'s segments as owned identifier strings, if `ty` is a (possibly
/// referenced) type path at all — `None` for anything else (tuple,
/// slice, etc., none of which can ever be an `impl` self_ty's or a
/// binding's canonical type anyway).
fn type_path_idents(ty: &Type) -> Option<Vec<String>> {
    match ty {
        Type::Path(p) => Some(path_idents(&p.path)),
        Type::Reference(r) => type_path_idents(&r.elem),
        _ => None,
    }
}

/// The feature set this exact `closure_harness` test binary was actually
/// compiled with, read live via `cfg!(feature = "...")` rather than
/// assumed or hardcoded to match an expected CI invocation string. Used
/// by `cfg_predicate_holds` to decide which `#[cfg(...)]`-gated overload
/// of a symbol is the one genuinely reachable under *this* build — not
/// merely present in the source text.
fn active_feature(name: &str, declared_features: &BTreeSet<String>) -> bool {
    // `declared_features` is consulted for *every* name, not only ones
    // this harness binary itself wasn't compiled with — a feature this
    // exact process happens to have compiled in is trivially "declared"
    // by definition, but the reverse must also hold: a feature this
    // capability's own ledger data declares as required is active for
    // evaluating *that capability's* cfg-gated code, even when this
    // particular test binary was not itself built with it (that would
    // otherwise make chrome_remote_cache-gated code invisible to a
    // harness process that never enables chrome_remote_cache, which is
    // exactly the real, deliberate CI split this repo uses).
    if declared_features.iter().any(|f| f == name) {
        return true;
    }
    match name {
        "chrome" => cfg!(feature = "chrome"),
        "cache" => cfg!(feature = "cache"),
        "cache_request" => cfg!(feature = "cache_request"),
        "chrome_remote_cache" => cfg!(feature = "chrome_remote_cache"),
        "cache_chrome_hybrid" => cfg!(feature = "cache_chrome_hybrid"),
        "decentralized" => cfg!(feature = "decentralized"),
        "smart" => cfg!(feature = "smart"),
        "sitemap" => cfg!(feature = "sitemap"),
        "sync" => cfg!(feature = "sync"),
        "serde" => cfg!(feature = "serde"),
        "webdriver" => cfg!(feature = "webdriver"),
        _ => false,
    }
}

/// Splits `a,b,c`'s text on top-level commas — commas not nested inside
/// another `(...)` group — so `all(a, any(b, c))`'s inner text splits into
/// exactly `["a", "any(b, c)"]`, not three pieces.
fn split_cfg_args(inner: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for character in inner.chars() {
        match character {
            '(' => {
                depth += 1;
                current.push(character);
            }
            ')' => {
                depth -= 1;
                current.push(character);
            }
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Minimal recursive-descent evaluator for the subset of `#[cfg(...)]`
/// predicate syntax this codebase's production source actually uses:
/// `feature = "x"`, `test`, `not(P)`, `all(P, Q, ...)`, `any(P, Q, ...)`.
/// Operates on a whitespace-stripped copy of the attribute's token text
/// (`proc_macro2`'s `Display` inserts spaces between tokens, e.g. `not
/// (feature = "x")`, which would otherwise defeat fixed-string prefix
/// matching). Returns `None` — unresolvable — for any construct outside
/// this subset, rather than guessing; every caller treats `None` as
/// "not proven active," never as "compiled."
fn eval_cfg_predicate(tokens: &str, declared_features: &BTreeSet<String>) -> Option<bool> {
    if let Some(inner) = tokens
        .strip_prefix("not(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return eval_cfg_predicate(inner, declared_features).map(|value| !value);
    }
    if let Some(inner) = tokens
        .strip_prefix("all(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut result = true;
        for part in split_cfg_args(inner) {
            result &= eval_cfg_predicate(&part, declared_features)?;
        }
        return Some(result);
    }
    if let Some(inner) = tokens
        .strip_prefix("any(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut result = false;
        for part in split_cfg_args(inner) {
            result |= eval_cfg_predicate(&part, declared_features)?;
        }
        return Some(result);
    }
    if tokens == "test" {
        // This harness's own build never compiles under cfg(test) itself
        // (it is an integration test binary, not a unit test module).
        return Some(false);
    }
    if let Some(rest) = tokens.strip_prefix("feature=") {
        return Some(active_feature(rest.trim_matches('"'), declared_features));
    }
    None
}

/// `Some(true)` if `attrs` carries no `#[cfg(...)]` at all, or every
/// `#[cfg(...)]` attribute present evaluates `true` under this harness's
/// own live-compiled feature set. `Some(false)` if any evaluates `false`.
/// `None` if a predicate uses cfg syntax `eval_cfg_predicate` doesn't
/// understand.
fn cfg_predicate_holds(
    attrs: &[syn::Attribute],
    declared_features: &BTreeSet<String>,
    strict: bool,
) -> Option<bool> {
    // Cfg-activity is only evaluated *strictly* (fail-closed,
    // must-prove-active) for this workspace's own `spider` crate,
    // whose Cargo.toml feature names this file's `cfg!(feature =
    // "...")` calls genuinely reflect. A vendored/third-party crate
    // (`vendor/**`) has its own, disjoint feature namespace this
    // single-crate `cfg!` mechanism cannot resolve (a real case found
    // during this hardening: `vendor/chromey` gates code behind its
    // own internal `_cache` feature, which is not, and cannot be,
    // `spider`'s `cfg!(feature = "_cache")` — always false regardless
    // of what chromey was actually built with). There, cfg gates are
    // treated as decorative for definition-existence purposes, same
    // as before this hardening pass (still excluding cfg(test) and
    // #[test], which use no crate-specific feature semantics).
    if !strict {
        return Some(true);
    }
    let mut result = true;
    for attribute in attrs {
        if let syn::Meta::List(list) = &attribute.meta {
            if list.path.is_ident("cfg") {
                let normalized: String = list
                    .tokens
                    .to_string()
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                match eval_cfg_predicate(&normalized, declared_features) {
                    Some(value) => result &= value,
                    None => return None,
                }
            }
        }
    }
    Some(result)
}

/// An item's owning scope, for ambiguity detection. Both variants carry
/// the full enclosing module path (`""` for the file's top level,
/// `"real"` for `mod real { ... }`, `"real::nested"` for a nested module)
/// so that same-named definitions in *different* modules are never
/// silently collapsed into the same key. Bypass class closed (Codex
/// adversarial review): "preserve enclosing module identity for WIRED
/// definitions and callers... `real::hop` and `unrelated::hop` must
/// remain distinct." Before this, `Free` carried no module information at
/// all — a bare free function named `hop` inside `mod real { }` and an
/// unrelated free function *also* named `hop` inside `mod unrelated { }`
/// in the same file both inserted the exact same `Free` key into the
/// owners set, so the ambiguity check (which relies on distinct keys to
/// detect "more than one candidate") saw only one owner and silently
/// picked whichever body the recursive walk happened to visit — the
/// module-collision bypass this fixes.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
enum DefinitionOwner {
    Free(String),
    Impl(String, String),
}

fn join_module_path(module_path: &str, ident: &str) -> String {
    if module_path.is_empty() {
        ident.to_string()
    } else {
        format!("{module_path}::{ident}")
    }
}

/// Rust-AST definition check used for all maturity-critical evidence. The
/// legacy lexical helper above is retained only for the live-test detector;
/// it is deliberately never used to advance IMPLEMENTED/WIRED stages.
///
/// Bypass classes closed (Codex adversarial review):
///   - "bind Type::method to the actual impl/type, not merely a same-named
///     function somewhere in the file" — a qualified symbol
///     (`Type::method`) now only matches an `impl <Type>` block whose own
///     `self_ty` is exactly that type; a free function or a different
///     impl's same-named method can no longer satisfy it.
///   - "do not combine mutually exclusive cfg definitions into one
///     fictional chain" — an item gated by `#[cfg(...)]` only counts if
///     its predicate evaluates `true` under this harness's own,
///     live-read, actually-compiled feature set (`cfg_predicate_holds`);
///     an inactive or unresolvable overload's source text existing in the
///     file is not "real" evidence.
///   - "same bare-name terminal in another type/file" (the file half is
///     handled by construction: every caller reads the specific declared
///     file) — for a *bare*, unqualified symbol, if cfg-active, non-test
///     definitions exist in more than one distinct owning scope (two
///     different impl types, or a free function alongside an impl
///     method), that is irreducible AST-level ambiguity; this fails
///     closed (returns `false`) rather than silently picking one,
///     matching the "fail closed rather than accepting ambiguous
///     evidence" instruction.
fn ast_contains_production_definition(
    source: &str,
    symbol: &str,
    declared_features: &BTreeSet<String>,
    strict: bool,
    is_canonical_definition_site: bool,
) -> bool {
    let Ok(file) = syn::parse_file(source) else {
        return false;
    };
    let (expected_type, bare) = split_type_qualifier(symbol);
    // Bypass class closed (Codex adversarial review, round 5): whether a
    // *bare* `Website` reference in this file is affirmatively proven to
    // be the canonical type — computed once from the whole file, and
    // threaded down through `walk`'s recursion so every nested module
    // consumes the same file-wide provenance verdict via the shared
    // `canonical_type_owner_name` primitive below. See
    // `file_proves_bare_website`'s doc comment for the full model: this
    // is proof by affirmative provenance (definition site, or an exact
    // canonical `use` import), never absence-of-a-shadow reasoning.
    let bare_website_trusted =
        file_proves_bare_website(&file, is_canonical_definition_site, strict);

    #[allow(
        clippy::too_many_arguments,
        clippy::collapsible_if,
        clippy::collapsible_match
    )]
    fn walk(
        items: &[Item],
        bare: &str,
        module_path: &str,
        under_test: bool,
        declared_features: &BTreeSet<String>,
        strict: bool,
        bare_website_trusted: bool,
        owners: &mut BTreeSet<DefinitionOwner>,
    ) {
        for item in items {
            match item {
                Item::Fn(ItemFn { sig, attrs, .. }) => {
                    if !under_test
                        && sig.ident == bare
                        && !is_test_attributed(attrs)
                        && !attrs_have_cfg_test(attrs)
                        && cfg_predicate_holds(attrs, declared_features, strict) == Some(true)
                    {
                        owners.insert(DefinitionOwner::Free(module_path.to_string()));
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        let child_under_test = under_test
                            || attrs_have_cfg_test(&m.attrs)
                            || m.ident == "tests"
                            || m.ident == "test";
                        let child_module_path = join_module_path(module_path, &m.ident.to_string());
                        walk(
                            inner,
                            bare,
                            &child_module_path,
                            child_under_test,
                            declared_features,
                            strict,
                            bare_website_trusted,
                            owners,
                        );
                    }
                }
                Item::Impl(ItemImpl {
                    items: inner,
                    attrs,
                    self_ty,
                    ..
                }) => {
                    if under_test
                        || attrs_have_cfg_test(attrs)
                        || cfg_predicate_holds(attrs, declared_features, strict) != Some(true)
                    {
                        continue;
                    }
                    // Bypass class closed (Codex adversarial review,
                    // round 5): the owner's identity is resolved through
                    // `canonical_type_owner_name`, the single shared
                    // primitive — a qualified self_ty never resolves for
                    // any type; a bare `Website` self_ty resolves only
                    // when this file has affirmatively proven provenance
                    // (`bare_website_trusted`), not merely the absence of
                    // a detected shadow/bad-import.
                    let owner_name = type_path_idents(self_ty).and_then(|idents| {
                        canonical_type_owner_name(&idents, strict, bare_website_trusted)
                    });
                    for inner_item in inner {
                        if let ImplItem::Fn(f) = inner_item {
                            if f.sig.ident == bare
                                && !is_test_attributed(&f.attrs)
                                && !attrs_have_cfg_test(&f.attrs)
                                && cfg_predicate_holds(&f.attrs, declared_features, strict)
                                    == Some(true)
                            {
                                if let Some(name) = &owner_name {
                                    owners.insert(DefinitionOwner::Impl(
                                        module_path.to_string(),
                                        name.clone(),
                                    ));
                                }
                            }
                        }
                    }
                }
                Item::Struct(s) => {
                    if !under_test
                        && s.ident == bare
                        && !attrs_have_cfg_test(&s.attrs)
                        && cfg_predicate_holds(&s.attrs, declared_features, strict) == Some(true)
                    {
                        owners.insert(DefinitionOwner::Free(module_path.to_string()));
                    }
                }
                Item::Enum(e) => {
                    if !under_test
                        && e.ident == bare
                        && !attrs_have_cfg_test(&e.attrs)
                        && cfg_predicate_holds(&e.attrs, declared_features, strict) == Some(true)
                    {
                        owners.insert(DefinitionOwner::Free(module_path.to_string()));
                    }
                }
                Item::Trait(t) => {
                    if !under_test
                        && t.ident == bare
                        && !attrs_have_cfg_test(&t.attrs)
                        && cfg_predicate_holds(&t.attrs, declared_features, strict) == Some(true)
                    {
                        owners.insert(DefinitionOwner::Free(module_path.to_string()));
                    }
                }
                Item::Type(t) => {
                    if !under_test
                        && t.ident == bare
                        && !attrs_have_cfg_test(&t.attrs)
                        && cfg_predicate_holds(&t.attrs, declared_features, strict) == Some(true)
                    {
                        owners.insert(DefinitionOwner::Free(module_path.to_string()));
                    }
                }
                Item::Const(c) => {
                    if !under_test
                        && c.ident == bare
                        && !attrs_have_cfg_test(&c.attrs)
                        && cfg_predicate_holds(&c.attrs, declared_features, strict) == Some(true)
                    {
                        owners.insert(DefinitionOwner::Free(module_path.to_string()));
                    }
                }
                Item::Static(s) => {
                    if !under_test
                        && s.ident == bare
                        && !attrs_have_cfg_test(&s.attrs)
                        && cfg_predicate_holds(&s.attrs, declared_features, strict) == Some(true)
                    {
                        owners.insert(DefinitionOwner::Free(module_path.to_string()));
                    }
                }
                _ => {}
            }
        }
    }

    let mut owners = BTreeSet::new();
    walk(
        &file.items,
        bare,
        "",
        false,
        declared_features,
        strict,
        bare_website_trusted,
        &mut owners,
    );

    match expected_type {
        // Bypass class closed: "impl/type ownership should preserve
        // sufficient qualified identity so same final-segment names
        // cannot collapse across scopes/modules." A qualified symbol
        // (`Type::method`) is satisfied only when *exactly one* module's
        // `impl <Type>` block defines it — if the same type name is
        // independently implemented in two different modules of the same
        // file (an adversarial or accidental collision the ledger's
        // `Type::method` syntax has no way to disambiguate further),
        // that is unresolved ambiguity and fails closed rather than
        // matching either one.
        Some(expected) => {
            owners
                .iter()
                .filter(|owner| matches!(owner, DefinitionOwner::Impl(_, t) if t == expected))
                .count()
                == 1
        }
        None => owners.len() == 1,
    }
}

/// Bypass class closed (Codex adversarial review): "do not reduce
/// `module_a::tests::foo` and `module_b::tests::foo` to the same `foo`."
/// The prior version stripped `symbol` to its bare final segment and
/// searched *any* module in the file for a function of that name — so a
/// VERIFIED evidence entry citing `module_a::tests::foo` would be
/// satisfied by a completely different `foo` living inside
/// `module_b::tests` in the same file. This version requires the *entire*
/// declared module path (every segment before the final function name) to
/// be walked as an exact, contiguous, in-order chain of `mod` blocks from
/// the file's top level down to the function — a symbol naming a
/// different module chain can no longer be satisfied by an unrelated
/// same-named function elsewhere in the file. A bare symbol with no `::`
/// (the convention for `spider/tests/*.rs` integration-test binaries,
/// which have no enclosing `mod`) still resolves against the file's top
/// level only, unchanged from before.
fn ast_contains_test_definition(source: &str, symbol: &str) -> bool {
    let Ok(file) = syn::parse_file(source) else {
        return false;
    };
    let parts: Vec<&str> = symbol.split("::").filter(|part| !part.is_empty()).collect();
    let Some((fn_name, module_path)) = parts.split_last() else {
        return false;
    };
    fn test_attr(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            a.path().is_ident("test") || a.path().segments.last().is_some_and(|s| s.ident == "test")
        })
    }
    fn find(items: &[Item], module_path: &[&str], fn_name: &str) -> bool {
        match module_path.split_first() {
            None => items.iter().any(|item| match item {
                Item::Fn(f) => f.sig.ident == fn_name && test_attr(&f.attrs),
                _ => false,
            }),
            Some((next_module, rest)) => items.iter().any(|item| match item {
                Item::Mod(m) if m.ident == next_module => m
                    .content
                    .as_ref()
                    .is_some_and(|(_, inner)| find(inner, rest, fn_name)),
                _ => false,
            }),
        }
    }
    find(&file.items, module_path, fn_name)
}

#[allow(clippy::cmp_owned)]
fn ast_function_calls(
    source: &str,
    caller: &str,
    callee: &str,
    declared_features: &BTreeSet<String>,
    strict: bool,
    is_canonical_definition_site: bool,
) -> bool {
    let Ok(file) = syn::parse_file(source) else {
        return false;
    };
    // Bypass class closed (Codex adversarial review, round 5): shared
    // with `ast_contains_production_definition` — whether a *bare*
    // `Website` self_ty/receiver in this file is affirmatively proven to
    // be the canonical type. See `file_proves_bare_website`'s doc
    // comment for the full model; this is threaded through both the
    // strict (`scan`) and vendor-permissive
    // (`scan_vendor_permissive_with_ambiguity_check`) receiver-resolution
    // paths below via the shared `canonical_type_owner_name` primitive.
    let bare_website_trusted =
        file_proves_bare_website(&file, is_canonical_definition_site, strict);
    let (expected_caller_type, caller) = split_type_qualifier(caller);
    let callee_parts: Vec<&str> = callee.split("::").collect();
    let method = *callee_parts.last().unwrap_or(&callee);
    let expected_type = (callee_parts.len() > 1).then(|| callee_parts[callee_parts.len() - 2]);
    struct Calls<'a> {
        method: &'a str,
        qualified: Option<&'a str>,
        expected_type: Option<&'a str>,
        receiver_type: Option<String>,
        hit: bool,
    }
    impl<'ast, 'a> Visit<'ast> for Calls<'a> {
        fn visit_expr_call(&mut self, node: &'ast ExprCall) {
            if let syn::Expr::Path(path) = &*node.func {
                let names: Vec<String> = path
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                let last = names.last().map(String::as_str);
                if last == Some(self.method)
                    && self.qualified.is_none_or(|q| {
                        let path = names.join("::");
                        path == q
                            || path.ends_with(&format!("::{q}"))
                            || (path == format!("Self::{}", self.method)
                                && self.expected_type == self.receiver_type.as_deref())
                    })
                {
                    self.hit = true;
                }
            }
            syn::visit::visit_expr_call(self, node);
        }
        fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
            // Method calls are accepted only on `self`; this binds the hop
            // to the caller's impl receiver and rejects unrelated same-name
            // methods on arbitrary values.
            if node.method == self.method
                && matches!(&*node.receiver, syn::Expr::Path(p) if p.path.is_ident("self") || p.path.segments.last().is_some_and(|segment| {
                    self.expected_type.is_none_or(|expected| {
                        segment.ident.to_string() == expected.to_lowercase()
                    })
                }))
                && self
                    .expected_type
                    .is_none_or(|expected| Some(expected) == self.receiver_type.as_deref())
            {
                self.hit = true;
            }
            syn::visit::visit_expr_method_call(self, node);
        }
    }
    // Bypass class closed (Codex adversarial review, macro adjacency
    // round three — "remove arbitrary macro-derived call adjacency from
    // maturity proof... do not identify macros by final/bare name such
    // as `join!`... calls inside macro invocations do not establish
    // WIRED adjacency"). An earlier version of this harness credited a
    // narrow allowlist of macro *names* known to evaluate all their
    // arguments (`tokio::join!`/bare `join!`) — but a macro name alone
    // is a syntactic identifier, not a canonical symbol: a locally
    // defined `macro_rules! join { ($($tokens:tt)*) => {}; }` shadows
    // the real `tokio::join!` under the exact same bare name, and this
    // harness has no way to tell, from a single file's `syn::File`,
    // which `join!` a given `join!(...)` invocation actually resolves
    // to — that is real macro/name resolution, which this harness does
    // not attempt ("do not build a rust compiler"). There is now no
    // `visit_macro`/`visit_expr_macro` override in `Calls` at all — the
    // default `syn::visit` behavior for a macro invocation does not
    // parse or recurse into its token stream (`TokenStream` is not a
    // typed AST node), so a call expression sitting inside *any* macro
    // invocation's arguments, allowlisted name or not, is structurally
    // invisible to this adjacency scan and can never establish a hit. A
    // production chain that depends on macro expansion to establish
    // real adjacency (this repository's own
    // `tokio::join!(page.spawn_cache_listener(...), ...)` in the cache
    // path among them) cannot be structurally proven by this harness and
    // is NOT_PROVEN — the ledger's own WIRED chain evidence must not
    // route through such a hop; see `SCORPION_CANONICAL_CREDENTIAL_CACHE_ISOLATION_001`'s
    // WIRED chains, revised in this round to keep only the chain that
    // never depended on macro adjacency.
    // Bypass classes closed on the *caller* side (Codex adversarial
    // review, mirroring the same fixes already applied to
    // `ast_contains_production_definition`): "bind Type::method to the
    // actual impl/type" — a qualified caller symbol (`Website::crawl`)
    // now only matches inside an `impl Website` block, never a free
    // function or a different impl's same-named method; "exclude
    // test-only intermediate callers" — recursion no longer descends into
    // `#[cfg(test)]`/`mod tests`, and a `#[test]`-attributed function can
    // never itself be treated as a production caller; "do not combine
    // mutually exclusive cfg definitions into one fictional chain" — a
    // candidate caller body is only inspected if its own `#[cfg(...)]`
    // (if any) evaluates `true` under this capability's declared feature
    // set (`cfg_predicate_holds`), so an inactive overload's body can
    // never supply the call evidence for an active chain.
    /// Whether `attrs` carries a `#[cfg(...)]` attribute at all (any
    /// predicate, resolvable or not) — used only by the vendor/non-strict
    /// ambiguity path below to distinguish "unconditionally real" from
    /// "one of possibly several mutually exclusive overloads."
    fn has_any_cfg_attr(attrs: &[syn::Attribute]) -> bool {
        attrs
            .iter()
            .any(|a| matches!(&a.meta, syn::Meta::List(list) if list.path.is_ident("cfg")))
    }

    /// Vendor (non-strict) cfg-ambiguity resolution. Bypass class closed
    /// (Codex adversarial review): "vendor cfg must not fail open... do
    /// not allow mutually exclusive vendor overloads to be combined into
    /// a fictional chain." Because this harness cannot evaluate a
    /// vendored crate's own feature namespace (`cfg_predicate_holds` is a
    /// no-op under `strict = false`), every non-strict candidate is a
    /// `(has_cfg, hit)` pair collected without filtering by cfg-activity.
    /// A candidate with no `#[cfg(...)]` at all is unconditionally real —
    /// there is nothing mutually exclusive about it — and its result is
    /// trusted directly. When every candidate is cfg-gated, the
    /// call-adjacency question is answered only when every candidate
    /// agrees; genuine disagreement (one gated overload calls the target,
    /// another does not) is unresolved compile-time exclusivity this
    /// harness cannot settle, and now yields "no adjacency proof" rather
    /// than the prior unconditional OR across every overload regardless
    /// of cfg.
    fn resolve_vendor_candidates(candidates: &[(bool, bool)]) -> bool {
        // A genuinely severe latent bug closed here: `Iterator::all` on
        // an empty iterator is vacuously `true` in Rust. Without this
        // explicit check, a caller name with *zero* matching candidates
        // at a given syntactic scope (the overwhelmingly common case —
        // most scopes don't define a function named `caller`) would
        // still make this function return `true`, and the recursive
        // scanner below would treat that as "found a hit" and return
        // immediately without ever inspecting the sibling/child scopes
        // that might have the real candidate — silently fabricating
        // adjacency proof for vendor code from nothing.
        if candidates.is_empty() {
            return false;
        }
        if let Some((_, hit)) = candidates.iter().find(|(has_cfg, _)| !has_cfg) {
            return *hit;
        }
        if candidates.iter().all(|(_, hit)| *hit) {
            return true;
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_vendor_permissive_with_ambiguity_check(
        items: &[Item],
        caller: &str,
        expected_caller_type: Option<&str>,
        under_test: bool,
        bare_website_trusted: bool,
        method: &str,
        qualified: Option<&str>,
        expected_type: Option<&str>,
    ) -> bool {
        fn hit_for(
            block: &syn::Block,
            method: &str,
            qualified: Option<&str>,
            expected_type: Option<&str>,
            receiver_type: Option<String>,
        ) -> bool {
            let mut v = Calls {
                method,
                qualified,
                expected_type,
                receiver_type,
                hit: false,
            };
            v.visit_block(block);
            v.hit
        }
        let mut candidates: Vec<(bool, bool)> = Vec::new();
        if expected_caller_type.is_none() {
            for item in items {
                if let Item::Fn(f) = item {
                    if f.sig.ident == caller
                        && !under_test
                        && !is_test_attributed(&f.attrs)
                        && !attrs_have_cfg_test(&f.attrs)
                    {
                        candidates.push((
                            has_any_cfg_attr(&f.attrs),
                            hit_for(&f.block, method, qualified, expected_type, None),
                        ));
                    }
                }
            }
        }
        for item in items {
            if let Item::Impl(i) = item {
                if under_test || attrs_have_cfg_test(&i.attrs) {
                    continue;
                }
                let receiver_type = type_path_idents(&i.self_ty).and_then(|idents| {
                    canonical_type_owner_name(&idents, false, bare_website_trusted)
                });
                if expected_caller_type
                    .is_some_and(|expected| Some(expected) != receiver_type.as_deref())
                {
                    continue;
                }
                for it in &i.items {
                    if let ImplItem::Fn(f) = it {
                        if f.sig.ident == caller
                            && !is_test_attributed(&f.attrs)
                            && !attrs_have_cfg_test(&f.attrs)
                        {
                            candidates.push((
                                has_any_cfg_attr(&f.attrs),
                                hit_for(
                                    &f.block,
                                    method,
                                    qualified,
                                    expected_type,
                                    receiver_type.clone(),
                                ),
                            ));
                        }
                    }
                }
            }
        }
        if resolve_vendor_candidates(&candidates) {
            return true;
        }
        for item in items {
            if let Item::Mod(m) = item {
                if let Some((_, inner)) = &m.content {
                    let child_under_test = under_test
                        || attrs_have_cfg_test(&m.attrs)
                        || m.ident == "tests"
                        || m.ident == "test";
                    if scan_vendor_permissive_with_ambiguity_check(
                        inner,
                        caller,
                        expected_caller_type,
                        child_under_test,
                        bare_website_trusted,
                        method,
                        qualified,
                        expected_type,
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn scan(
        items: &[Item],
        caller: &str,
        expected_caller_type: Option<&str>,
        under_test: bool,
        declared_features: &BTreeSet<String>,
        strict: bool,
        bare_website_trusted: bool,
        method: &str,
        qualified: Option<&str>,
        expected_type: Option<&str>,
    ) -> bool {
        if !strict {
            return scan_vendor_permissive_with_ambiguity_check(
                items,
                caller,
                expected_caller_type,
                under_test,
                bare_website_trusted,
                method,
                qualified,
                expected_type,
            );
        }
        for item in items {
            match item {
                Item::Fn(f)
                    if f.sig.ident == caller
                        && expected_caller_type.is_none()
                        && !under_test
                        && !is_test_attributed(&f.attrs)
                        && !attrs_have_cfg_test(&f.attrs)
                        && cfg_predicate_holds(&f.attrs, declared_features, strict)
                            == Some(true) =>
                {
                    let mut v = Calls {
                        method,
                        qualified,
                        expected_type,
                        receiver_type: None,
                        hit: false,
                    };
                    v.visit_block(&f.block);
                    if v.hit {
                        return true;
                    }
                }
                Item::Impl(i) => {
                    if under_test
                        || attrs_have_cfg_test(&i.attrs)
                        || cfg_predicate_holds(&i.attrs, declared_features, strict) != Some(true)
                    {
                        continue;
                    }
                    let receiver_type = type_path_idents(&i.self_ty).and_then(|idents| {
                        canonical_type_owner_name(&idents, strict, bare_website_trusted)
                    });
                    if expected_caller_type
                        .is_some_and(|expected| Some(expected) != receiver_type.as_deref())
                    {
                        continue;
                    }
                    for it in &i.items {
                        if let ImplItem::Fn(f) = it {
                            if f.sig.ident == caller
                                && !is_test_attributed(&f.attrs)
                                && !attrs_have_cfg_test(&f.attrs)
                                && cfg_predicate_holds(&f.attrs, declared_features, strict)
                                    == Some(true)
                            {
                                let mut v = Calls {
                                    method,
                                    qualified,
                                    expected_type,
                                    receiver_type: receiver_type.clone(),
                                    hit: false,
                                };
                                v.visit_block(&f.block);
                                if v.hit {
                                    return true;
                                }
                            }
                        }
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        let child_under_test = under_test
                            || attrs_have_cfg_test(&m.attrs)
                            || m.ident == "tests"
                            || m.ident == "test";
                        if scan(
                            inner,
                            caller,
                            expected_caller_type,
                            child_under_test,
                            declared_features,
                            strict,
                            bare_website_trusted,
                            method,
                            qualified,
                            expected_type,
                        ) {
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }
    let qualified = (callee_parts.len() > 1).then_some(callee);
    scan(
        &file.items,
        caller,
        expected_caller_type,
        false,
        declared_features,
        strict,
        bare_website_trusted,
        method,
        qualified,
        expected_type,
    )
}

/// Whether `code` (comment-stripped, typically a single function's body)
/// contains a genuine *call* to `symbol` — a free-function call, a method
/// call (`.symbol(`), or an associated-function call (`::symbol(`).
/// Bypass class closed: "prove real call adjacency for every declared
/// hop" — the prior version only checked the callee's name appeared
/// anywhere in the *caller's file*, not that the caller's own body
/// actually invokes it.
#[allow(dead_code)]
fn contains_call(code: &str, symbol: &str) -> bool {
    let bare = symbol.rsplit("::").next().unwrap_or(symbol);
    [
        format!("{bare}("),
        format!(".{bare}("),
        format!("::{bare}("),
    ]
    .iter()
    .any(|pattern| code.contains(pattern.as_str()))
}

/// Extracts the brace-delimited body of `symbol`'s function definition
/// from `code` (comment-stripped), via a simple brace-depth scan from the
/// first `{` after the `fn NAME(`/`fn NAME<` signature. Best-effort (does
/// not account for `{`/`}` inside string literals), documented, and
/// sufficient for the definitions this harness actually inspects — a
/// wrong extraction fails a hop's adjacency check rather than silently
/// passing, so the failure mode of this limitation is fail-closed, not
/// fail-open.
fn extract_fn_body<'a>(code: &'a str, symbol: &str) -> Option<&'a str> {
    all_fn_bodies(code, symbol).into_iter().next()
}

/// Like `extract_fn_body`, but returns *every* cfg-gated overload's body,
/// not just the first textual match. Bypass class closed: "inspect all
/// matching function definitions, not only the first textual match" — a
/// symbol with multiple `#[cfg(...)]`-gated overloads (this codebase has
/// several, e.g. `crawl_concurrent` has 4) would previously have its
/// adjacency checked against whichever overload happened to appear
/// first in the file, which is not necessarily the one compiled under
/// the feature configuration the chain actually claims.
fn all_fn_bodies<'a>(code: &'a str, symbol: &str) -> Vec<&'a str> {
    let bare = symbol.rsplit("::").next().unwrap_or(symbol);
    let needles = [format!("fn {bare}("), format!("fn {bare}<")];
    let mut bodies = Vec::new();
    for needle in &needles {
        let mut search_from = 0usize;
        while let Some(relative_start) = code[search_from..].find(needle.as_str()) {
            let start = search_from + relative_start;
            if let Some(relative_brace) = code[start..].find('{') {
                let body_start = start + relative_brace;
                let bytes = code.as_bytes();
                let mut depth: i32 = 0;
                let mut index = body_start;
                while index < bytes.len() {
                    match bytes[index] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                bodies.push(&code[body_start..=index]);
                                break;
                            }
                        }
                        _ => {}
                    }
                    index += 1;
                }
            }
            search_from = start + needle.len();
        }
    }
    bodies
}

/// Whether *any* cfg-gated overload of `caller_symbol` in `code` has a
/// body that genuinely calls `callee_symbol`.
#[allow(dead_code)]
fn any_overload_calls(code: &str, caller_symbol: &str, callee_symbol: &str) -> bool {
    all_fn_bodies(code, caller_symbol)
        .into_iter()
        .any(|body| contains_call(body, callee_symbol))
}

/// A production (non-test) source path: under `src/`, not `tests/`.
fn is_production_source_path(relative_path: &str) -> bool {
    (relative_path.contains("/src/") || relative_path.starts_with("src/"))
        && !relative_path.contains("/tests/")
}

/// Whether cfg-activity should be evaluated *strictly* (fail-closed,
/// must-prove-active under this harness's own live `cfg!` reads plus this
/// capability's declared feature requirements) for a given evidence path.
/// True only for this workspace's own `spider/src/**` — the one crate
/// whose Cargo.toml feature names this exact process's `cfg!(feature =
/// "...")` calls actually reflect. Any other tree (most notably
/// `vendor/**`) has its own, disjoint feature namespace this harness has
/// no reliable way to resolve from within `spider`'s own compilation —
/// see `cfg_predicate_holds`'s doc comment for the real case that proved
/// this (`vendor/chromey`'s internal `_cache` feature).
fn is_spider_own_crate_source(relative_path: &str) -> bool {
    relative_path.starts_with("spider/src/")
}

/// Whether `relative_path` is the one real definition site of the
/// canonical `Website` type — the ground truth `file_proves_bare_website`
/// anchors every other maturity-bearing `Website` reference to.
fn is_canonical_website_definition_site(relative_path: &str) -> bool {
    relative_path == CANONICAL_WEBSITE_DEFINITION_FILE
}

// =====================================================================
// Ledger (TOML) loading.
// =====================================================================

struct LedgerFile {
    path: PathBuf,
    filename: String,
    doc: toml::Value,
}

/// Loads every `docs/frontier/ledger/*.toml` file except `TEMPLATE.toml`
/// and `LIVE_NETWORK_TESTS.toml` (registry data, not a capability claim).
fn load_ledger_entries() -> Vec<LedgerFile> {
    let dir = ledger_dir();
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", dir.display()))
    {
        let entry = entry.expect("failed to read ledger directory entry");
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("toml") {
            continue;
        }
        let filename = path
            .file_stem()
            .expect("ledger file must have a stem")
            .to_string_lossy()
            .to_string();
        if filename == "TEMPLATE" || filename == "LIVE_NETWORK_TESTS" {
            continue;
        }
        let table: toml::Table = read(&path)
            .parse()
            .unwrap_or_else(|error| panic!("failed to parse {} as TOML: {error}", path.display()));
        out.push(LedgerFile {
            path,
            filename,
            doc: toml::Value::Table(table),
        });
    }
    assert!(
        !out.is_empty(),
        "expected at least one non-template ledger entry under docs/frontier/ledger — the \
         harness has nothing to verify"
    );
    out
}

fn str_field<'a>(table: &'a toml::Value, key: &str) -> Option<&'a str> {
    table.get(key).and_then(|value| value.as_str())
}

fn str_array(table: &toml::Value, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(|value| value.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn bool_field(table: &toml::Value, key: &str) -> Option<bool> {
    table.get(key).and_then(|value| value.as_bool())
}

fn stage_table<'a>(doc: &'a toml::Value, name: &str) -> Option<&'a toml::Value> {
    doc.get("stages").and_then(|stages| stages.get(name))
}

/// This ledger entry's own declared `PRODUCTION_REACHABLE.feature_requirements`
/// (empty if that table doesn't exist yet). Used, alongside this harness
/// binary's own live-compiled feature set, to decide which `#[cfg(...)]`-
/// gated overload of a WIRED/IMPLEMENTED symbol is genuinely reachable for
/// *this specific capability* — a capability that has documented it needs
/// `chrome_remote_cache`, say, is entitled to have its own cfg-gated
/// production code evaluated as compiled under that feature, even when
/// this specific harness process was not built with it; a capability that
/// has documented no such requirement is not.
fn declared_features(doc: &toml::Value) -> BTreeSet<String> {
    stage_table(doc, "PRODUCTION_REACHABLE")
        .map(|table| {
            str_array(table, "feature_requirements")
                .into_iter()
                .collect()
        })
        .unwrap_or_default()
}

/// The feature set used specifically to resolve WIRED chains'
/// cfg-gated overloads: `declared_features` above, plus
/// `[stages.WIRED].additional_cfg_features` — a WIRED-scoped list, kept
/// deliberately separate from `PRODUCTION_REACHABLE.feature_requirements`
/// rather than folded into it. `feature_requirements` feeds the
/// reachability *verdict* computation with ANY-of-these-enabled
/// semantics (`production_reachable_claims_are_grep_verified_against_shipping_manifests`);
/// a real case found while hardening this file's cfg evaluation is a
/// chain whose hop (`sitemap_crawl_chain`) is only a real, non-stub
/// definition under the `sitemap` feature — a fact that belongs to *this
/// chain's* compilability, not to the reachability verdict's own feature
/// list (adding `sitemap` there would silently change which shipping
/// artifacts count as "reachable," an unrelated effect this fix must not
/// have).
fn declared_features_for_wired(doc: &toml::Value) -> BTreeSet<String> {
    let mut features = declared_features(doc);
    if let Some(wired) = stage_table(doc, "WIRED") {
        features.extend(str_array(wired, "additional_cfg_features"));
    }
    features
}

/// The feature set used specifically to resolve IMPLEMENTED evidence's
/// cfg-gated definitions: `declared_features` above, plus
/// `[stages.IMPLEMENTED].additional_cfg_features` — the same escape hatch
/// `declared_features_for_wired` provides for WIRED chains, for the
/// identical reason. A real case found while hardening this file: a
/// symbol can be gated behind `#[cfg(all(feature = "chrome", feature =
/// "chrome_remote_cache"))]` where only `chrome_remote_cache` belongs in
/// `PRODUCTION_REACHABLE.feature_requirements` (Cargo.toml already makes
/// `chrome_remote_cache` imply `chrome` — see `chrome_remote_cache =
/// ["chrome", ...]` — so every real build enabling it also enables
/// `chrome`); adding `chrome` to `feature_requirements` instead would
/// widen the reachability verdict's ANY-of-these-enabled check to any
/// artifact that enables plain `chrome` for unrelated reasons, an
/// unrelated and incorrect effect this stage-scoped list avoids exactly
/// as `declared_features_for_wired` already does for WIRED.
fn declared_features_for_implemented(doc: &toml::Value) -> BTreeSet<String> {
    let mut features = declared_features(doc);
    if let Some(implemented) = stage_table(doc, "IMPLEMENTED") {
        features.extend(str_array(implemented, "additional_cfg_features"));
    }
    features
}

const STAGE_ORDER: [&str; 8] = [
    "DESIGNED",
    "IMPLEMENTED",
    "VERIFIED",
    "WIRED",
    "PRODUCTION_REACHABLE",
    "ADVERSARIALLY_VERIFIED",
    "CI_ENFORCED",
    "CLOSED",
];

const PROOF_CLASSES: [&str; 5] = [
    "CODE_PROVEN",
    "CI_PROVEN",
    "OPERATOR_OBSERVED",
    "LIVE_ENVIRONMENT_DEPENDENT",
    "UNPROVEN",
];

fn proof_table<'a>(doc: &'a toml::Value, name: &str) -> Option<&'a toml::Value> {
    doc.get("proof").and_then(|proof| proof.get(name))
}

fn commit_is_reachable(sha: &str) -> bool {
    sha.len() == 40
        && sha.chars().all(|character| character.is_ascii_hexdigit())
        && git_object_type(sha).as_deref() == Some("commit")
        && git_commit_reachable_from_head(sha)
}

// =====================================================================
// Git helpers. Bypass class closed: "closure_commit must resolve to a
// commit object, not merely any git object" — `git cat-file -e` (the
// prior check) succeeds for a blob, tree, or tag SHA equally; only
// `git cat-file -t` distinguishes the object kind.
// =====================================================================

fn git_object_type(sha: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root())
        .arg("cat-file")
        .arg("-t")
        .arg(sha)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Whether `sha` is an ancestor of (or equal to) `HEAD` — i.e. genuinely
/// part of this repository's committed history, not merely an object
/// that happens to exist in the object database (a dangling/unreachable
/// commit, or one on an unrelated, unmerged branch, would fail this).
fn git_commit_reachable_from_head(sha: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(workspace_root())
        .arg("merge-base")
        .arg("--is-ancestor")
        .arg(sha)
        .arg("HEAD")
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

// =====================================================================
// Workflow YAML (real parse, not raw-text scan). Bypass classes closed:
// "raw workflow text/comments must not satisfy execution evidence" and
// "gated/non-required/schedule-only commands must not satisfy required
// CI_ENFORCED evidence" — a text scan cannot distinguish a `run:` value
// from a `#`-prefixed YAML comment mentioning the same text, nor can it
// tell whether a step sits behind an `if:` gate. A real parse can.
// =====================================================================

struct WorkflowStep {
    /// The repo-relative workflow file this step came from, in the same
    /// logical namespace a ledger's `ci_workflow_file` field names it by
    /// — `.github/workflows/<filename>` — regardless of the *physical*
    /// directory it was actually read from (which, under
    /// `CLOSURE_HARNESS_WORKFLOWS_DIR_OVERRIDE`, is some scratch temp
    /// dir, not the real `.github/workflows/`). Bypass class closed
    /// (Codex adversarial review): "CI_ENFORCED evidence must retain the
    /// exact workflow file that supplied the qualifying command... do
    /// not scan all workflow files and discard provenance." Before this
    /// field existed, every step from every `.yml` file was flattened
    /// into one undifferentiated list, so a matching command sitting in
    /// the *wrong* workflow file (one never actually enforced by this
    /// repository's real CI configuration) satisfied CI_ENFORCED
    /// identically to one in the right file.
    file: String,
    job: String,
    name: String,
    /// `true` if the job or this specific step carries any `if:`
    /// condition — GitHub Actions runs a step unconditionally only when
    /// no `if:` is present anywhere in its chain; this harness treats any
    /// conditional as "not guaranteed required" without trying to
    /// evaluate the condition's truth value (conservative: something
    /// merely gated behind `if: success()`, GitHub's own implicit
    /// default, is never written explicitly in this repo's workflows, so
    /// this does not currently produce false positives — confirmed by
    /// inspection).
    gated: bool,
    applicable: bool,
    run: Option<String>,
}

/// Override point for `closure_harness_behavioral_contract.rs` (mirrors
/// `CLOSURE_HARNESS_LEDGER_DIR_OVERRIDE`) — lets a fixture-based
/// subprocess test exercise workflow-shaped CI_ENFORCED checks (a
/// schedule-only trigger, a gated step) against a scratch `.yml` file
/// without touching the real `.github/workflows/`. Unset in every normal
/// run, including CI.
fn load_workflow_steps() -> Vec<WorkflowStep> {
    let workflows_dir = match std::env::var("CLOSURE_HARNESS_WORKFLOWS_DIR_OVERRIDE") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => workspace_root().join(".github/workflows"),
    };
    let mut out = Vec::new();
    for entry in fs::read_dir(&workflows_dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", workflows_dir.display()))
    {
        let path = entry.expect("workflow dir entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("yml") {
            continue;
        }
        let file_label = format!(
            ".github/workflows/{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<unknown>")
        );
        let text = read(&path);
        let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text)
            .unwrap_or_else(|error| panic!("failed to parse {} as YAML: {error}", path.display()));
        let triggers = doc.get("on").or_else(|| doc.get("true"));
        let applicable = triggers.and_then(|v| v.as_mapping()).is_some_and(|m| {
            m.keys()
                .any(|k| matches!(k.as_str(), Some("push") | Some("pull_request")))
        });
        let Some(jobs) = doc.get("jobs").and_then(|value| value.as_mapping()) else {
            continue;
        };
        for (job_key, job_value) in jobs {
            let job_name = job_key.as_str().unwrap_or("<unknown>").to_string();
            let job_gated = job_value.get("if").is_some();
            let Some(steps) = job_value.get("steps").and_then(|value| value.as_sequence()) else {
                continue;
            };
            for step in steps {
                let step_gated = job_gated || step.get("if").is_some();
                let step_name = step
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("<unnamed>")
                    .to_string();
                // A `run:` value that is not shell-unambiguous (an
                // unquoted `#` or a backslash) is never trusted as CI
                // evidence by anything downstream — every consumer of
                // `WorkflowStep::run` (self-selection, live-test
                // exclusion, CI_ENFORCED matching) receives `None` rather
                // than text it can't safely reason about. See
                // `shell_text_is_unambiguous`'s doc comment for the exact
                // bypass this closes; this is the uniform enforcement
                // point so no individual check can forget it.
                let run = step
                    .get("run")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
                    .filter(|run| shell_text_is_unambiguous(run));
                out.push(WorkflowStep {
                    file: file_label.clone(),
                    job: job_name.clone(),
                    name: step_name,
                    gated: step_gated,
                    applicable,
                    run,
                });
            }
        }
    }
    out
}

/// True if `text` contains no unquoted `#` and no backslash anywhere —
/// i.e. a real shell would interpret every character in `text` exactly as
/// written, with no possibility that part of it is silently a comment (a
/// real shell truncates execution at an unquoted `#`) or a line
/// continuation joining it with more text. Bypass class closed (Codex
/// adversarial review): "the shell-comment bypass is real" — a `run:`
/// value or ledger `ci_command` like `cargo test -p spider --lib --skip
/// live_test # rest ignored by nothing` reads, to a plain text scan, as
/// containing `--skip live_test`; a real shell run of that exact text
/// still does, since the `#` here isn't at the start of a word — but a
/// value where the `--skip` sits *after* an unquoted `#` would make bash
/// silently drop it while a naive text scan still "sees" it. Rather than
/// modeling exactly when a `#` is dangerous, this rejects its presence
/// entirely (outside a double-quoted string), matching the requested
/// "prefer eliminating shell parsing" posture as closely as the current
/// string-based ledger/workflow representation allows.
fn shell_text_is_unambiguous(text: &str) -> bool {
    let mut in_string = false;
    for character in text.chars() {
        match character {
            '\\' => return false,
            '"' => in_string = !in_string,
            '#' if !in_string => return false,
            _ => {}
        }
    }
    true
}

/// Strict allowlist grammar, not a denylist heuristic: rather than trying
/// to recognize and reject every possible shell wrapper (`echo`, `printf`,
/// `if false; then ... fi`, `$(...)`, backticks, `eval`, POSIX
/// `name() { }` functions, `&&`/`||` chains, `>`/`<` redirection, `;`
/// sequencing, `#` comments, `\` continuations) — an open-ended and
/// always-incomplete list — this only *accepts* a command that is
/// structurally nothing but a direct `cargo test` invocation: the first
/// two whitespace tokens must be exactly `cargo` and `test`, the text
/// must be shell-unambiguous (see `shell_text_is_unambiguous`), no other
/// shell metacharacter appears anywhere in the string, and it does not
/// carry `--no-run` (Codex adversarial review: a `--no-run` step —
/// exactly this repository's own real pre-build step, which compiles
/// `closure_harness` without ever running it — must never satisfy a
/// check whose name is "is this genuinely *executable*"; the self-
/// selection and CI_ENFORCED checks that filter on this function would
/// otherwise have been satisfiable by a step that runs nothing at all).
/// Anything else is rejected outright, without attempting to understand
/// what it does.
fn executable_test_command(command: &str) -> bool {
    let normalized = normalize_whitespace(command);
    const SHELL_METACHARACTERS: [char; 9] = ['|', '&', ';', '$', '`', '<', '>', '(', ')'];
    if normalized
        .chars()
        .any(|c| SHELL_METACHARACTERS.contains(&c))
    {
        return false;
    }
    if !shell_text_is_unambiguous(&normalized) {
        return false;
    }
    if normalized
        .split_whitespace()
        .any(|token| token == "--no-run")
    {
        return false;
    }
    let mut tokens = normalized.split_whitespace();
    tokens.next() == Some("cargo") && tokens.next() == Some("test")
}

// =====================================================================
// Live-network test detection (HIGH 3). Independently re-derives, via
// static analysis, which `#[test]`/`#[tokio::test]` functions in the
// `spider` crate perform a real network call against a non-local,
// non-reserved host — the same analysis performed by hand for this
// frontier's CI redesign, now a permanent, always-run check instead of a
// one-off script. Bypass class closed: "a static --skip list is not
// sufficient mechanical evidence" — this detector runs fresh every time
// and is cross-checked against both the registry (below) and the actual
// CI skip flags, so a new unclassified live test, or a classified one
// that stops being skipped, both fail loudly.
// =====================================================================

struct DetectedLiveTest {
    test_path: String,
    hosts: BTreeSet<String>,
}

fn module_path_for(relative_to_src: &str) -> String {
    let without_ext = relative_to_src.trim_end_matches(".rs");
    let without_ext = without_ext.strip_suffix("/mod").unwrap_or(without_ext);
    without_ext.replace('/', "::")
}

/// Reserved/local hosts that never indicate a real live-network
/// dependency: IANA reserved documentation domains and this codebase's
/// own locally-spawned-server convention.
fn is_reserved_or_local_host(host: &str) -> bool {
    host.ends_with(".invalid")
        || host.ends_with(".test")
        || host.ends_with(".example")
        || host.contains("example.com")
        || host.contains("127.0.0.1")
        || host.contains("localhost")
        || host.starts_with("0.0.0.0")
        || host.ends_with(".onion")
        || host.ends_with("w3.org")
}

fn detect_live_network_tests() -> Vec<DetectedLiveTest> {
    let src_root = workspace_root().join("spider/src");
    let mut files = Vec::new();
    collect_rs_files(&src_root, &mut files);

    let mut results = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(&src_root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let base_module = module_path_for(&relative);
        let source = read(&file);
        let lines: Vec<&str> = source.lines().collect();

        // Track a stack of enclosing `mod NAME { ... }` blocks by brace
        // depth so a nested `mod tests { ... }` contributes `::tests` to
        // the reported test path, matching cargo's own test naming.
        let mut mod_stack: Vec<(usize, String)> = Vec::new(); // (brace_depth_at_open, name)
        let mut depth: usize = 0;

        let mut index = 0usize;
        while index < lines.len() {
            let raw_line = lines[index];
            let line = strip_line_comment(raw_line);
            let trimmed = line.trim_start();

            if let Some(rest) = trimmed
                .strip_prefix("mod ")
                .filter(|_| trimmed.contains('{'))
            {
                let name = rest
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next();
                if let Some(name) = name.filter(|n| !n.is_empty()) {
                    mod_stack.push((depth, name.to_string()));
                }
            }

            let is_test_attr = trimmed.contains("#[tokio::test]") || trimmed == "#[test]";
            if is_test_attr {
                // Scan forward past any further attributes to the fn line.
                let mut j = index + 1;
                while j < lines.len() {
                    let candidate = strip_line_comment(lines[j]).trim_start();
                    if candidate.starts_with('#') {
                        j += 1;
                        continue;
                    }
                    break;
                }
                if let Some(fn_line) = lines.get(j) {
                    let fn_line_code = strip_line_comment(fn_line);
                    if let Some(name) = extract_fn_name(fn_line_code) {
                        // Find this function's body via brace counting
                        // from `j` onward in the comment-stripped source.
                        let body_code = code_only(&lines[j..].join("\n"));
                        if let Some(body) = extract_fn_body(&body_code, &name) {
                            if let Some(hosts) = live_network_hosts_in(body) {
                                let mut path_parts: Vec<String> = vec![base_module.clone()];
                                path_parts.extend(mod_stack.iter().map(|(_, name)| name.clone()));
                                path_parts.push(name);
                                let test_path = path_parts
                                    .into_iter()
                                    .filter(|part| !part.is_empty())
                                    .collect::<Vec<_>>()
                                    .join("::");
                                results.push(DetectedLiveTest { test_path, hosts });
                            }
                        }
                    }
                }
            }

            // Maintain brace depth and pop closed `mod` scopes.
            for character in line.chars() {
                match character {
                    '{' => depth += 1,
                    '}' => {
                        depth = depth.saturating_sub(1);
                        while mod_stack
                            .last()
                            .is_some_and(|(open_depth, _)| *open_depth >= depth)
                        {
                            mod_stack.pop();
                        }
                    }
                    _ => {}
                }
            }
            index += 1;
        }
    }
    results
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn extract_fn_name(fn_line: &str) -> Option<String> {
    let trimmed = fn_line.trim_start();
    let after_fn = trimmed
        .strip_prefix("async fn ")
        .or_else(|| trimmed.strip_prefix("fn "))
        .or_else(|| trimmed.strip_prefix("pub async fn "))
        .or_else(|| trimmed.strip_prefix("pub fn "))
        .or_else(|| trimmed.strip_prefix("pub(crate) async fn "))
        .or_else(|| trimmed.strip_prefix("pub(crate) fn "))?;
    let name: String = after_fn
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// If `body` (a single test function's comment-stripped source) performs
/// a real network call — `.crawl(`/`.scrape(`/`.crawl_smart(`/
/// `reqwest::Client` — against at least one non-reserved, non-local host,
/// returns those hosts. A local listener does not suppress a test that also
/// performs an external crawl. Deliberately does *not* treat bare `.get(`/`.send(`/
/// `reqwest::` (unqualified) as network markers — all three are common
/// enough on non-network types/paths (a cache's `.get`, a channel's
/// `.send`, `reqwest::header::*` construction helpers used without ever
/// dispatching a request) to have produced confirmed false positives
/// when tried; `reqwest::Client` specifically is not used anywhere in
/// this crate's test suite except as an actual HTTP client.
fn live_network_hosts_in(body: &str) -> Option<BTreeSet<String>> {
    let performs_network_call = [".crawl(", ".scrape(", ".crawl_smart(", "reqwest::Client"]
        .iter()
        .any(|marker| body.contains(marker));
    if !performs_network_call {
        return None;
    }
    let mut hosts = BTreeSet::new();
    let mut saw_scheme = false;
    let mut saw_non_reserved_host = false;
    let mut rest = body;
    while let Some(scheme_index) = rest.find("://") {
        saw_scheme = true;
        let after_scheme = &rest[scheme_index + 3..];
        let host: String = after_scheme
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '-')
            .collect();
        let host_len = host.len().min(after_scheme.len());
        if !host.is_empty() && !is_reserved_or_local_host(&host) {
            saw_non_reserved_host = true;
            hosts.insert(host);
        }
        rest = &after_scheme[host_len..];
    }
    if !hosts.is_empty() {
        return Some(hosts);
    }
    let dynamically_composed_url = body.contains("://{") || body.contains("format!(\"https://");
    if !saw_scheme || (!saw_non_reserved_host && !dynamically_composed_url) {
        return None;
    }
    // Unknown/dynamically-composed destinations are deliberately classified
    // as live rather than silently treated as deterministic. A local listener
    // does not exempt a test that also performs an external crawl.
    let has_explicit_local_url = body.contains("127.0.0.1") || body.contains("localhost");
    (!has_explicit_local_url).then(|| {
        let mut unknown = BTreeSet::new();
        unknown.insert("<dynamic-or-unresolved-host>".to_string());
        unknown
    })
}

// =====================================================================
// Live-network registry (HIGH 3).
// =====================================================================

struct LiveNetworkRegistryEntry {
    test_name: String,
}

/// A real `cargo test` invocation's *complete* structural semantics,
/// parsed once from a `run:` command: package, `--lib`/`--test NAME`
/// target selection, the exact `--features` token set, positional test
/// filters, `--skip` patterns, and `--exact`. This is the single
/// canonical representation both CI_ENFORCED evidence (parsed from the
/// ledger's own structured fields — no shell text) and a real workflow
/// `run:` step (parsed from shell text, but only ever after
/// `executable_test_command` has confirmed it is unambiguous) are reduced
/// to before being compared — field-by-field structural equality, never
/// string equality. Bypass class closed (Codex adversarial review):
/// "quoting tricks, continuations, aliases, wrappers and unrelated shell
/// syntax must not be able to alter execution semantics while preserving
/// a different verifier interpretation" — two commands with the exact
/// same real cargo-test meaning but superficially different text (token
/// order, whitespace) now compare equal; two commands with the exact same
/// text but different real meaning cannot occur, because meaning is what
/// is compared.
#[derive(Debug, Clone)]
struct TestSelection {
    package: Option<String>,
    lib: bool,
    test_targets: BTreeSet<String>,
    features: BTreeSet<String>,
    skip_patterns: Vec<String>,
    positional_filters: Vec<String>,
    exact: bool,
}

/// Structural equality between two `TestSelection`s — every field
/// compared as an order-independent set where order carries no cargo
/// semantics (`--skip`/positional filters may be repeated in any order),
/// plus exact equality on `package`/`lib`/`exact`.
fn same_test_selection(a: &TestSelection, b: &TestSelection) -> bool {
    a.package == b.package
        && a.lib == b.lib
        && a.exact == b.exact
        && a.test_targets == b.test_targets
        && a.features == b.features
        && a.skip_patterns.iter().collect::<BTreeSet<_>>()
            == b.skip_patterns.iter().collect::<BTreeSet<_>>()
        && a.positional_filters.iter().collect::<BTreeSet<_>>()
            == b.positional_filters.iter().collect::<BTreeSet<_>>()
}

/// Parses a real `run:` command into its `cargo test` positional
/// test-name filter(s) (found *before* ` -- `, e.g. `cargo test -p
/// spider --lib --features chrome_remote_cache utils::tests::foo --
/// --exact` — `utils::tests::foo` is cargo's own filter, not a libtest
/// arg) and its libtest harness args (`--skip`/`--exact`, found *after*
/// ` -- `). Getting this split wrong in either direction is exactly the
/// kind of mistake this checker exists to catch in *ledger* data, so it
/// must not make the same mistake itself: an earlier version of this
/// function only looked after ` -- ` and silently treated every
/// positional filter as absent, making any `--exact <single test>`
/// command look like it excluded every other registered test — caught by
/// a mutation test, fixed here.
/// Cargo-level (before ` -- `) flags whose execution-semantics effect
/// this model does not represent, and which change whether the cited
/// evidence actually executes: `--no-run` compiles but runs nothing;
/// `--no-default-features`/`--all-features` change the active feature set
/// independent of any `--features` flag; `--doc` selects doctests only;
/// `--tests`/`--bins`/`--benches`/`--examples`/`--bin`/`--example`/`--bench`
/// select a whole class of targets this model has no way to name. Bypass
/// class closed (Codex adversarial review): "unknown or unsupported
/// execution-affecting flags must cause CI evidence to be rejected, not
/// silently discarded."
const CARGO_LEVEL_REJECTED_FLAGS: [&str; 11] = [
    "--no-run",
    "--no-default-features",
    "--all-features",
    "--doc",
    "--tests",
    "--bins",
    "--benches",
    "--examples",
    "--bin",
    "--example",
    "--bench",
];

/// Libtest-level (after ` -- `) flags whose presence changes *which*
/// tests run, not merely filters among a fixed set: `--list` prints test
/// names and runs nothing; `--ignored` inverts normal selection to run
/// only `#[ignore]`-attributed tests (excluding a cited test that isn't
/// `#[ignore]`d, or wrongly including one that is, either way not the
/// plain "run this test" semantics `CI_ENFORCED` claims).
const LIBTEST_REJECTED_FLAGS: [&str; 2] = ["--list", "--ignored"];

/// Parses a real `run:` command into its complete `cargo test` /
/// libtest execution semantics — package, `--lib`/`--test NAME`
/// targets, `--features`, positional filters, `--skip`, `--exact` — or
/// `None` if the command uses any flag (cargo-level or libtest-level)
/// this model does not fully account for the effect of. `None` is a
/// first-class, fail-closed outcome: every caller treats it as "this
/// command's execution semantics cannot be trusted," never as "no
/// selection, so nothing is excluded" — the same posture an unparseable
/// `cfg` predicate gets from `eval_cfg_predicate`. An earlier version of
/// the tokenizer had a permissive catch-all (`other if
/// other.starts_with("--") => i += 1`) that silently skipped *any*
/// unrecognized flag, which is exactly how `--no-run`/`--list`/`--ignored`
/// previously went unnoticed.
/// Whether `token` syntactically looks like a Cargo/libtest option rather
/// than a plain value — starts with `-` and is more than just `-` alone.
/// Bypass class closed (Codex adversarial review): "a recognized
/// value-taking option must reject another option token as its value" —
/// `cargo test -p --lib` is not a real invocation of "package named
/// `--lib`"; a real `cargo`/libtest parser would see `--lib` as its own
/// flag and `-p` as missing its required value, and reject the whole
/// command. This model must reach the same conclusion, not silently
/// consume `--lib` as if it were an ordinary package/target/feature/skip
/// value.
fn looks_like_an_option(token: &str) -> bool {
    token.starts_with('-') && token.len() > 1
}

fn parse_test_selection(run: &str) -> Option<TestSelection> {
    let (before, after) = run.split_once(" -- ").unwrap_or((run, ""));

    let mut positional_filters = Vec::new();
    let mut package = None;
    let mut lib = false;
    let mut test_targets = BTreeSet::new();
    let mut features: BTreeSet<String> = BTreeSet::new();
    let before_tokens: Vec<&str> = before.split_whitespace().collect();
    let mut i = 0;
    while i < before_tokens.len() {
        match before_tokens[i] {
            "cargo" | "test" => i += 1,
            "--lib" => {
                lib = true;
                i += 1;
            }
            // Bypass class closed (Codex adversarial review): "reject at
            // minimum: -p without package, --test without target." A
            // trailing `-p`/`--test` with nothing after it previously
            // silently left `package`/`test_targets` unset rather than
            // being treated as malformed input.
            // Bypass class closed (Codex adversarial review): "a
            // recognized value-taking option must reject another option
            // token as its value." `cargo test -p --lib` previously
            // consumed the literal token `"--lib"` as the package name —
            // real Cargo would instead see `--lib` as its own flag and
            // reject the command for `-p` missing a value. A value token
            // that itself looks like an option can never be a real
            // value.
            "-p" => {
                let name = before_tokens.get(i + 1)?;
                if looks_like_an_option(name) {
                    return None;
                }
                package = Some((*name).to_string());
                i += 2;
            }
            "--test" => {
                let name = before_tokens.get(i + 1)?;
                if looks_like_an_option(name) {
                    return None;
                }
                test_targets.insert((*name).to_string());
                i += 2;
            }
            // Bypass class closed: "repeated --features occurrences
            // merge; comma-separated features split into individual
            // feature tokens; supported short form -F must either be
            // modeled exactly or rejected; reject bare --features without
            // value." The prior implementation delegated to
            // `parse_features_flag`, which only ever looked at the
            // *first* `--features` occurrence in the whole command
            // string and split only on whitespace — a second
            // `--features` flag was silently dropped, and
            // `--features "a,b"` (comma-separated, no spaces) was parsed
            // as one bogus feature token `"a,b"` instead of two. `-F`
            // (cargo's real short alias for `--features`) previously
            // wasn't recognized as a flag *at all* — it fell through to
            // the positional-filter catch-all, silently reinterpreting a
            // cargo option as a test-name filter.
            "--features" | "-F" => {
                i += 1;
                let first = before_tokens.get(i)?;
                let mut collected = String::new();
                if first.starts_with('"') {
                    let mut closed = false;
                    while let Some(token) = before_tokens.get(i) {
                        collected.push(' ');
                        collected.push_str(token.trim_matches('"'));
                        let ends_quoted = token.ends_with('"');
                        i += 1;
                        if ends_quoted {
                            closed = true;
                            break;
                        }
                    }
                    if !closed {
                        return None; // unterminated quote — malformed
                    }
                } else {
                    // An unquoted value that itself looks like an option
                    // (e.g. `--features --lib`) is not a real feature
                    // value — real Cargo would parse `--lib` as its own
                    // flag, leaving `--features` without one.
                    if looks_like_an_option(first) {
                        return None;
                    }
                    collected.push_str(first);
                    i += 1;
                }
                let mut any_token = false;
                for token in collected.split(|c: char| c == ',' || c.is_whitespace()) {
                    if !token.is_empty() {
                        features.insert(token.to_string());
                        any_token = true;
                    }
                }
                if !any_token {
                    return None; // --features/-F with an empty value
                }
            }
            other if CARGO_LEVEL_REJECTED_FLAGS.contains(&other) => return None,
            other if other.starts_with("--") => return None,
            // Bypass class closed: "every unknown short option must fail
            // closed... do not silently reinterpret unknown options as
            // positional filters." A single-dash token that isn't `-p`
            // or `-F` (both matched above) is an unrecognized short
            // option, not a test name.
            other if other.starts_with('-') && other.len() > 1 => return None,
            other => {
                positional_filters.push(other.trim_matches('"').to_string());
                i += 1;
            }
        }
    }

    let after_tokens: Vec<&str> = after.split_whitespace().collect();
    let mut skip_patterns = Vec::new();
    let mut exact = false;
    let mut j = 0;
    while j < after_tokens.len() {
        match after_tokens[j] {
            // Bypass class closed: "reject --skip without value."
            "--skip" => {
                let pattern = after_tokens.get(j + 1)?;
                // Bypass class closed: `-- --skip --exact` previously
                // consumed the literal token `"--exact"` as a skip
                // pattern, silently discarding the real `--exact` flag
                // instead of failing on the malformed command.
                if looks_like_an_option(pattern) {
                    return None;
                }
                skip_patterns.push((*pattern).to_string());
                j += 2;
            }
            "--exact" => {
                exact = true;
                j += 1;
            }
            other if LIBTEST_REJECTED_FLAGS.contains(&other) => return None,
            _ => return None,
        }
    }

    Some(TestSelection {
        package,
        lib,
        test_targets,
        features,
        skip_patterns,
        positional_filters,
        exact,
    })
}

/// Whether a parsed `TestSelection` definitely excludes `test_name` from
/// the tests that will actually run.
fn selection_excludes(selection: &TestSelection, test_name: &str) -> bool {
    if selection
        .skip_patterns
        .iter()
        .any(|pattern| test_name.contains(pattern.as_str()))
    {
        return true;
    }
    if !selection.positional_filters.is_empty() {
        return if selection.exact {
            !selection
                .positional_filters
                .iter()
                .any(|filter| filter == test_name)
        } else {
            !selection
                .positional_filters
                .iter()
                .any(|filter| test_name.contains(filter.as_str()))
        };
    }
    false
}

fn load_live_network_registry() -> Vec<LiveNetworkRegistryEntry> {
    let path = ledger_dir().join("LIVE_NETWORK_TESTS.toml");
    let table: toml::Table = read(&path)
        .parse()
        .unwrap_or_else(|error| panic!("failed to parse {} as TOML: {error}", path.display()));
    let doc = toml::Value::Table(table);
    doc.get("tests")
        .and_then(|value| value.as_array())
        .unwrap_or_else(|| panic!("{}: expected a top-level `tests` array", path.display()))
        .iter()
        .map(|entry| LiveNetworkRegistryEntry {
            test_name: str_field(entry, "test_name")
                .unwrap_or_else(|| panic!("{}: entry missing `test_name`", path.display()))
                .to_string(),
        })
        .collect()
}

#[test]
fn live_network_registry_matches_detected_reality() {
    let detected_tests = detect_live_network_tests();
    let detected: BTreeSet<String> = detected_tests
        .iter()
        .map(|test| test.test_path.clone())
        .collect();
    let registered: BTreeSet<String> = load_live_network_registry()
        .into_iter()
        .map(|entry| entry.test_name)
        .collect();

    let unclassified: Vec<String> = detected
        .difference(&registered)
        .map(|test_path| {
            let hosts = detected_tests
                .iter()
                .find(|test| &test.test_path == test_path)
                .map(|test| test.hosts.iter().cloned().collect::<Vec<_>>().join(","))
                .unwrap_or_default();
            format!("{test_path} (hosts: {hosts})")
        })
        .collect();
    assert!(
        unclassified.is_empty(),
        "new, unclassified live-network test(s) detected: {unclassified:?} — add each to \
         docs/frontier/ledger/LIVE_NETWORK_TESTS.toml (with its host and a `--skip` entry in \
         every required spider_core step) before this can pass"
    );

    let stale: Vec<&String> = registered.difference(&detected).collect();
    assert!(
        stale.is_empty(),
        "docs/frontier/ledger/LIVE_NETWORK_TESTS.toml lists {stale:?}, but static analysis no \
         longer detects a live-network call in that test — remove the stale entry (and its \
         --skip flags) or explain why detection is wrong"
    );
}

#[test]
fn required_ci_excludes_every_registered_live_test() {
    let registry = load_live_network_registry();
    let steps = load_workflow_steps();
    // Every required, non-gated step that runs `cargo test -p spider
    // --lib` must, on its own, exclude every registered live test — not
    // "some step somewhere excludes it" (the bug this replaces: a single
    // unrelated --exact step targeting a *different* test made every
    // registry entry look covered, because --exact alone was treated as
    // sufficient regardless of which test it actually named).
    let relevant_steps: Vec<&WorkflowStep> = steps
        .iter()
        .filter(|step| {
            step.applicable
                && !step.gated
                && step.run.as_deref().is_some_and(|run| {
                    // `--no-run` compiles the test binary but never
                    // executes it — it can never leak a live-network test
                    // execution regardless of --skip flags, so it is not
                    // a "relevant step" for this check at all (distinct
                    // from being "covered": there's nothing to exclude
                    // from a run that never happens).
                    run.contains("cargo test")
                        && run.contains("-p spider")
                        && run.contains("--lib")
                        && !run.contains("--no-run")
                })
        })
        .collect();
    assert!(
        !relevant_steps.is_empty(),
        "expected at least one required `cargo test -p spider --lib` step to check live-test \
         exclusion against"
    );
    for entry in &registry {
        for step in &relevant_steps {
            let run = step.run.as_deref().unwrap();
            // `parse_test_selection` returning `None` means this step uses
            // an execution-affecting flag this model doesn't fully account
            // for — fail closed: it cannot be trusted to exclude anything,
            // exactly as if no --skip/--exact were present at all.
            let excluded = parse_test_selection(run)
                .is_some_and(|selection| selection_excludes(&selection, &entry.test_name));
            assert!(
                excluded,
                "registered live-network test {:?} is not excluded from required step (job \
                 {:?}, run {run:?}) — it can enter the deterministic required suite, or the \
                 step's own execution semantics could not be fully modeled (an unrecognized \
                 flag) and are therefore untrusted. A --skip flag naming it, or a --exact \
                 filter naming something else, is required in EVERY such step, not just one \
                 of them",
                entry.test_name, step.job
            );
        }
    }
}

// =====================================================================
// Stage checks.
// =====================================================================

#[test]
fn ledger_entry_id_matches_its_filename() {
    for entry in load_ledger_entries() {
        let id = str_field(&entry.doc, "id")
            .unwrap_or_else(|| panic!("{}: missing top-level `id`", entry.path.display()));
        assert_eq!(
            id,
            entry.filename,
            "{}: top-level `id` must equal the filename (without .toml)",
            entry.path.display()
        );
    }
}

#[test]
fn designed_stage_references_a_real_sdd_file() {
    for entry in load_ledger_entries() {
        let designed = stage_table(&entry.doc, "DESIGNED").unwrap_or_else(|| {
            panic!(
                "{}: every ledger entry must have [stages.DESIGNED]",
                entry.path.display()
            )
        });
        let sdd = str_field(designed, "sdd")
            .unwrap_or_else(|| panic!("{}: [stages.DESIGNED] missing `sdd`", entry.path.display()));
        assert!(
            workspace_root().join(sdd).is_file(),
            "{}: DESIGNED.sdd = {sdd:?} does not exist on disk",
            entry.path.display()
        );
    }
}

/// IMPLEMENTED evidence entries are `"path:symbol"` — verified as a real,
/// non-comment Rust *definition*, not merely text presence anywhere in
/// the file (see `contains_definition`'s doc comment for the exact
/// bypass this closes).
#[test]
fn implemented_stage_evidence_references_real_definitions_not_comments() {
    for entry in load_ledger_entries() {
        let Some(implemented) = stage_table(&entry.doc, "IMPLEMENTED") else {
            continue;
        };
        let evidence = str_array(implemented, "evidence");
        assert!(
            !evidence.is_empty(),
            "{}: [stages.IMPLEMENTED] evidence must not be empty once the table exists",
            entry.path.display()
        );
        let features = declared_features_for_implemented(&entry.doc);
        for item in evidence {
            let (file_part, symbol) = item.split_once(':').unwrap_or_else(|| {
                panic!(
                    "{}: IMPLEMENTED evidence {item:?} must be `path:symbol`",
                    entry.path.display()
                )
            });
            let source = read(&workspace_root().join(file_part));
            assert!(
                ast_contains_production_definition(
                    &source,
                    symbol,
                    &features,
                    is_spider_own_crate_source(file_part),
                    is_canonical_website_definition_site(file_part)
                ),
                "{}: IMPLEMENTED evidence {item:?} — no real (non-comment) definition of \
                 {symbol:?} found in {file_part:?}",
                entry.path.display()
            );
        }
    }
}

/// VERIFIED evidence entries are `"path::test_path"` — each must resolve
/// to a real `#[test]`/`#[tokio::test]`-attributed function definition,
/// not merely a string that happens to appear in the file (a comment, a
/// prose description, or a typo'd name would previously have been
/// silently accepted).
#[test]
fn verified_stage_evidence_resolves_to_real_test_definitions() {
    for entry in load_ledger_entries() {
        let Some(verified) = stage_table(&entry.doc, "VERIFIED") else {
            continue;
        };
        assert_eq!(
            bool_field(verified, "test_only"),
            Some(true),
            "{}: [stages.VERIFIED] must explicitly set test_only = true",
            entry.path.display()
        );
        let evidence = str_array(verified, "evidence");
        assert!(
            !evidence.is_empty(),
            "{}: [stages.VERIFIED] evidence must not be empty once the table exists",
            entry.path.display()
        );
        for item in &evidence {
            let (file_part, symbol) = item.split_once(':').unwrap_or_else(|| {
                panic!(
                    "{}: VERIFIED evidence {item:?} must be `path:symbol`",
                    entry.path.display()
                )
            });
            let symbol = symbol.trim_start_matches(':');
            let source = read(&workspace_root().join(file_part));
            assert!(
                ast_contains_test_definition(&source, symbol),
                "{}: VERIFIED evidence {item:?} — no real (non-comment) function definition \
                 of {symbol:?} found in {file_part:?}",
                entry.path.display()
            );
        }
        // Structural cross-check: last_verified_command, if present, must
        // at minimum select the right compilation target for where this
        // evidence lives (a `src/`-defined unit test needs `--lib`; a
        // `tests/*.rs` integration test needs `--test <binary>`) — this
        // does not execute the command (a nested `cargo test` subprocess
        // risks a target-directory lock conflict with the outer test
        // run), but it does reject the most basic "cited the wrong
        // command" mistake.
        if let Some(command) = str_field(verified, "last_verified_command") {
            for item in &evidence {
                let (file_part, _) = item.split_once(':').unwrap();
                if file_part.starts_with("spider/src/") {
                    assert!(
                        command.contains("--lib"),
                        "{}: VERIFIED evidence {item:?} lives in spider/src, but \
                         last_verified_command {command:?} has no --lib target selector",
                        entry.path.display()
                    );
                } else if let Some(test_binary) = file_part
                    .strip_prefix("spider/tests/")
                    .and_then(|rest| rest.strip_suffix(".rs"))
                {
                    assert!(
                        command.contains(&format!("--test {test_binary}")),
                        "{}: VERIFIED evidence {item:?} lives in spider/tests/{test_binary}.rs, \
                         but last_verified_command {command:?} has no `--test {test_binary}` \
                         selector",
                        entry.path.display()
                    );
                }
            }
        }
    }
}

/// Symbols recognized as genuine, non-test, non-niche production entry
/// points into the `spider` library's own crawl API — the same methods
/// `spider_cli`/`spider_mcp`'s own source calls (grep-confirmed
/// elsewhere). A WIRED chain must be rooted at one of these, not at an
/// arbitrary function that could itself be an uncalled island.
const RECOGNIZED_PRODUCTION_ROOTS: [&str; 3] =
    ["Website::crawl", "Website::scrape", "Website::crawl_smart"];

/// Rejects "compiled != wired" and "prove real call adjacency for every
/// declared hop": every hop must be a genuine, non-comment *definition*,
/// and every caller's *function body* (not just its file) must contain a
/// genuine *call* to the next hop — a chain listing two symbols that both
/// happen to appear somewhere in the same file, with no proof one calls
/// the other, previously passed this check.
#[test]
fn wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source() {
    for entry in load_ledger_entries() {
        let Some(wired) = stage_table(&entry.doc, "WIRED") else {
            continue;
        };
        let callers = str_array(wired, "callers");
        assert!(
            !callers.is_empty(),
            "{}: [stages.WIRED] callers must not be empty once the table exists",
            entry.path.display()
        );
        let features = declared_features_for_wired(&entry.doc);
        for chain in callers {
            let hops: Vec<&str> = chain.split("->").map(str::trim).collect();
            assert!(
                hops.len() >= 2,
                "{}: WIRED chain {chain:?} must have at least two hops",
                entry.path.display()
            );
            // Bypass class closed (Codex adversarial review): "do not
            // reduce path/to/file.rs:CapabilityType::method to only
            // method" — the prior version stripped both the file and any
            // type qualifier off both sides before comparing, so a WIRED
            // chain terminating at `UnrelatedType::apply` in a completely
            // different file would still "bind" to IMPLEMENTED evidence
            // for `CapabilityType::apply` merely because the bare method
            // names matched. The full `path:symbol` identity (file *and*
            // fully qualified symbol, exactly as written in the ledger)
            // must match verbatim.
            let implemented_evidence: BTreeSet<String> = stage_table(&entry.doc, "IMPLEMENTED")
                .map(|table| str_array(table, "evidence").into_iter().collect())
                .unwrap_or_default();
            let terminal = hops.last().map(|hop| normalize_whitespace(hop));
            assert!(
                terminal.as_deref().is_some_and(|hop| implemented_evidence
                    .iter()
                    .any(|evidence| normalize_whitespace(evidence) == hop)),
                "{}: WIRED chain terminal {terminal:?} must bind, as a complete path:symbol \
                 identity (not merely a bare trailing method name), to one of this capability's \
                 own IMPLEMENTED evidence entries {implemented_evidence:?}",
                entry.path.display()
            );

            let (root_file, root_symbol) = hops[0].split_once(':').unwrap_or_else(|| {
                panic!(
                    "{}: WIRED chain root {:?} must be `path:symbol`",
                    entry.path.display(),
                    hops[0]
                )
            });
            assert!(
                RECOGNIZED_PRODUCTION_ROOTS.contains(&root_symbol),
                "{}: WIRED chain root {root_symbol:?} is not one of \
                 RECOGNIZED_PRODUCTION_ROOTS {RECOGNIZED_PRODUCTION_ROOTS:?}",
                entry.path.display()
            );
            assert!(
                is_production_source_path(root_file),
                "{}: WIRED chain root file {root_file:?} is not a production `src/` path",
                entry.path.display()
            );
            let root_source = read(&workspace_root().join(root_file));
            assert!(
                ast_contains_production_definition(
                    &root_source,
                    root_symbol,
                    &features,
                    is_spider_own_crate_source(root_file),
                    is_canonical_website_definition_site(root_file)
                ),
                "{}: WIRED chain root {root_symbol:?} has no real, non-test, non-comment \
                 definition in {root_file:?}",
                entry.path.display()
            );

            // Adjacency: for each consecutive pair, the caller's own
            // function body (not merely its file) must contain a call to
            // the callee.
            let mut hop_files_and_symbols = Vec::new();
            for hop in &hops {
                let (file_part, symbol) = hop.split_once(':').unwrap_or_else(|| {
                    panic!(
                        "{}: WIRED chain hop {hop:?} must be `path:symbol`",
                        entry.path.display()
                    )
                });
                hop_files_and_symbols.push((file_part, symbol));
            }
            for window in hop_files_and_symbols.windows(2) {
                let (caller_file, caller_symbol) = window[0];
                let (_, callee_symbol) = window[1];
                let caller_source = read(&workspace_root().join(caller_file));
                let strict = is_spider_own_crate_source(caller_file);
                let is_canonical_site = is_canonical_website_definition_site(caller_file);
                assert!(
                    ast_contains_production_definition(
                        &caller_source,
                        caller_symbol,
                        &features,
                        strict,
                        is_canonical_site
                    ),
                    "{}: WIRED chain — could not locate any function body for \
                     {caller_symbol:?} in {caller_file:?} to check adjacency to \
                     {callee_symbol:?}",
                    entry.path.display()
                );
                assert!(
                    ast_function_calls(
                        &caller_source,
                        caller_symbol,
                        callee_symbol,
                        &features,
                        strict,
                        is_canonical_site
                    ),
                    "{}: WIRED chain hop {caller_symbol:?} -> {callee_symbol:?} — \
                     {caller_symbol:?}'s own function body in {caller_file:?} does not call \
                     {callee_symbol:?}; both symbols merely co-occurring in the same file is \
                     not adjacency",
                    entry.path.display()
                );
            }
            // The terminal hop only ever appears as a *callee* above —
            // its own existence (and, critically, module-scoped
            // ambiguity) was never independently verified. Bypass class
            // closed (Codex adversarial review): "a declared WIRED hop
            // must resolve to one specific production definition" — this
            // applies to the terminal exactly as much as every
            // intermediate hop.
            if let Some((terminal_file, terminal_symbol)) = hop_files_and_symbols.last() {
                let terminal_source = read(&workspace_root().join(terminal_file));
                let strict = is_spider_own_crate_source(terminal_file);
                assert!(
                    ast_contains_production_definition(
                        &terminal_source,
                        terminal_symbol,
                        &features,
                        strict,
                        is_canonical_website_definition_site(terminal_file)
                    ),
                    "{}: WIRED chain terminal {terminal_symbol:?} in {terminal_file:?} does not \
                     resolve to exactly one specific production definition (missing, or \
                     ambiguous across modules/types)",
                    entry.path.display()
                );
            }
        }
    }
}

/// Rejects "author-selected generic symbols... cannot prove arbitrary
/// capabilities reachable": PRODUCTION_REACHABLE.entry_point_symbols must
/// be a subset of the root symbols this *same capability's* own WIRED
/// chains were just proven (above) to terminate at — a capability cannot
/// invent a disconnected, generic entry point (`.crawl(` on its own,
/// unconnected to this capability's actual implementation) as reachability
/// proof.
#[test]
fn production_reachable_entry_points_are_bound_to_this_capabilitys_own_wired_roots() {
    for entry in load_ledger_entries() {
        let Some(production_reachable) = stage_table(&entry.doc, "PRODUCTION_REACHABLE") else {
            continue;
        };
        let Some(wired) = stage_table(&entry.doc, "WIRED") else {
            panic!(
                "{}: PRODUCTION_REACHABLE present without WIRED",
                entry.path.display()
            );
        };
        let wired_roots: BTreeSet<String> = str_array(wired, "callers")
            .iter()
            .filter_map(|chain| chain.split("->").next())
            .filter_map(|first_hop| first_hop.trim().split_once(':'))
            .map(|(_, symbol)| symbol.to_string())
            .collect();

        let entry_points = str_array(production_reachable, "entry_point_symbols");
        assert!(
            !entry_points.is_empty(),
            "{}: PRODUCTION_REACHABLE.entry_point_symbols must not be empty once the table \
             exists",
            entry.path.display()
        );
        for claimed_entry_point in &entry_points {
            assert!(
                wired_roots.contains(claimed_entry_point),
                "{}: PRODUCTION_REACHABLE.entry_point_symbols claims {claimed_entry_point:?}, \
                 but this capability's own [stages.WIRED].callers never proved a chain rooted \
                 there — an entry point not already proven to reach this capability's \
                 IMPLEMENTED evidence cannot be used as reachability proof (this is what \
                 rejects a generic symbol like \".crawl(\" standing in for any capability)",
                entry.path.display()
            );
        }
    }
}

/// Crates/binaries this repository actually ships, and where to find their
/// real manifests and source.
fn known_shipping_artifacts() -> Vec<(&'static str, PathBuf)> {
    let root = workspace_root();
    vec![
        ("spider_cli", root.join("spider_cli/Cargo.toml")),
        ("spider_mcp", root.join("spider_mcp/Cargo.toml")),
        ("spider_worker", root.join("spider_worker/Cargo.toml")),
    ]
}

fn artifact_src_dir(name: &str) -> Option<PathBuf> {
    let root = workspace_root();
    match name {
        "spider_cli" => Some(root.join("spider_cli/src")),
        "spider_mcp" => Some(root.join("spider_mcp/src")),
        "spider_worker" => Some(root.join("spider_worker/src")),
        _ => None,
    }
}

/// A feature flag merely compiling a dependency's module into an
/// artifact's dependency tree is not proof that artifact's own code ever
/// calls it. Excludes each file's own test module (`production_code_only`)
/// so a symbol only reachable from that artifact's *own* `#[cfg(test)]`
/// code cannot count either — bypass class closed: "test-only
/// symbols/modules must not count."
fn artifact_source_calls_symbol(artifact: &str, symbol: &str) -> bool {
    !artifact_source_files_calling_symbol(artifact, symbol).is_empty()
}

/// Every file in `artifact`'s own `src/` tree whose (trusted, non-test)
/// production code genuinely calls `symbol` — the specific evidence
/// `artifact_source_calls_symbol` reduces to a bool. Exposed separately
/// so `CLOSED`/`ADVERSARIALLY_VERIFIED` revision binding can pin the
/// *exact* files a `PRODUCTION_REACHABLE = MET` claim rests on, not the
/// whole `src/` tree.
fn artifact_source_files_calling_symbol(artifact: &str, symbol: &str) -> Vec<PathBuf> {
    let Some(src_dir) = artifact_src_dir(artifact) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    collect_rs_files(&src_dir, &mut files);
    files
        .into_iter()
        .filter(|file| {
            let relative = file
                .strip_prefix(workspace_root())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let is_canonical_definition_site = is_canonical_website_definition_site(&relative);
            ast_any_production_call(&read(file), symbol, is_canonical_definition_site)
        })
        .collect()
}

/// Canonical type name a `PRODUCTION_REACHABLE` root symbol like
/// `Website::crawl` is bound to. `RECOGNIZED_PRODUCTION_ROOTS` today only
/// ever names `Website::*`, so this is a single constant rather than a
/// derivation from the symbol string — but it is checked explicitly
/// (below) rather than assumed, so a future root on a different type
/// would fail loudly instead of silently reusing the wrong binding.
const CANONICAL_PRODUCTION_TYPE: &str = "Website";

/// Local bindings within `source` whose type is *provably* the canonical
/// production type — either an explicit type annotation (`let x: Website`,
/// `&mut Website`, `&Website`) or direct construction
/// (`Website::new(...)`/`Website::default()`/`Website::from(...)`), where
/// "provably" now means the shared `canonical_type_owner_name` model:
/// affirmative provenance, not merely a plausible-looking path. Anything
/// else (an untyped `let`, a variable merely *named* `website`, a value
/// returned from some other function) is not trusted.
fn website_typed_bindings(
    file: &syn::File,
    is_spider_own_crate_source: bool,
    bare_website_trusted: bool,
) -> BTreeSet<String> {
    struct Bindings<'a> {
        names: BTreeSet<String>,
        is_spider_own_crate_source: bool,
        bare_website_trusted: bool,
        _marker: std::marker::PhantomData<&'a ()>,
    }
    fn type_is_website(
        ty: &syn::Type,
        is_spider_own_crate_source: bool,
        bare_website_trusted: bool,
    ) -> bool {
        match ty {
            syn::Type::Path(p) => {
                canonical_type_owner_name(
                    &path_idents(&p.path),
                    is_spider_own_crate_source,
                    bare_website_trusted,
                )
                .as_deref()
                    == Some(CANONICAL_PRODUCTION_TYPE)
            }
            syn::Type::Reference(r) => {
                type_is_website(&r.elem, is_spider_own_crate_source, bare_website_trusted)
            }
            _ => false,
        }
    }
    fn expr_constructs_website(
        expr: &syn::Expr,
        is_spider_own_crate_source: bool,
        bare_website_trusted: bool,
    ) -> bool {
        match expr {
            syn::Expr::Call(call) => match &*call.func {
                syn::Expr::Path(p) if p.path.segments.len() >= 2 => {
                    let idents = path_idents(&p.path);
                    let (method, type_prefix) = idents.split_last().expect("len >= 2");
                    matches!(method.as_str(), "new" | "default" | "from")
                        && canonical_type_owner_name(
                            type_prefix,
                            is_spider_own_crate_source,
                            bare_website_trusted,
                        )
                        .as_deref()
                            == Some(CANONICAL_PRODUCTION_TYPE)
                }
                _ => false,
            },
            syn::Expr::Reference(r) => {
                expr_constructs_website(&r.expr, is_spider_own_crate_source, bare_website_trusted)
            }
            syn::Expr::MethodCall(m) => expr_constructs_website(
                &m.receiver,
                is_spider_own_crate_source,
                bare_website_trusted,
            ),
            _ => false,
        }
    }
    impl<'ast, 'a> Visit<'ast> for Bindings<'a> {
        fn visit_local(&mut self, node: &'ast syn::Local) {
            // An untyped `let x = ...` has pat = Pat::Ident directly; a
            // typed `let x: T = ...` has pat = Pat::Type { pat:
            // Pat::Ident, ty, .. } — the identifier is nested one level
            // deeper in the typed case. Both are handled explicitly.
            let (ident, type_annotated) = match &node.pat {
                syn::Pat::Ident(pat_ident) => (Some(&pat_ident.ident), false),
                syn::Pat::Type(pat_type) => match &*pat_type.pat {
                    syn::Pat::Ident(pat_ident) => (
                        Some(&pat_ident.ident),
                        type_is_website(
                            &pat_type.ty,
                            self.is_spider_own_crate_source,
                            self.bare_website_trusted,
                        ),
                    ),
                    _ => (None, false),
                },
                _ => (None, false),
            };
            if let Some(ident) = ident {
                let constructed = node.init.as_ref().is_some_and(|init| {
                    expr_constructs_website(
                        &init.expr,
                        self.is_spider_own_crate_source,
                        self.bare_website_trusted,
                    )
                });
                if type_annotated || constructed {
                    self.names.insert(ident.to_string());
                }
            }
            syn::visit::visit_local(self, node);
        }
        // A function parameter's own explicit type annotation
        // (`fn f(website: &mut Website)`, `fn f(website: Website)`) is
        // exactly as provable as a `let x: Website = ...` binding's — the
        // doc comment above already names `&mut Website` as a recognized
        // form, but a bare parameter pattern was never actually reached
        // by `visit_local` (which only fires for `let` statements): a
        // real production caller (`spider_mcp/src/tools/mod.rs:
        // spawn_crawl_task(mut website: Website, ...)`,
        // `spider_cli/src/main.rs:crawl_with_mode(website: &mut Website,
        // ...)`) that receives its `Website` as a parameter and calls
        // `website.crawl()` from inside that same function body was
        // therefore never recognized as a canonical call at all — found
        // while building this frontier's own PRODUCTION_REACHABLE
        // evidence for `SCORPION_CANONICAL_CAPTCHA_MACHINE_READABLE_
        // CAPABILITY_COVERAGE_001` against real `spider_mcp` source.
        // `visit_pat_type` is syn's own override point for *every* typed
        // pattern — function parameters, closure parameters, and (redundantly
        // but harmlessly, since `self.names` is a set) the `Pat::Type` a
        // typed `let` already reaches via `visit_local` above — so this
        // one addition covers the missing case without duplicating or
        // replacing the existing `let`-specific handling (which alone
        // still carries the `expr_constructs_website` check `visit_pat_type`
        // has no equivalent need for: a bare parameter has no initializer
        // expression to inspect).
        fn visit_pat_type(&mut self, node: &'ast syn::PatType) {
            if let syn::Pat::Ident(pat_ident) = &*node.pat {
                if type_is_website(
                    &node.ty,
                    self.is_spider_own_crate_source,
                    self.bare_website_trusted,
                ) {
                    self.names.insert(pat_ident.ident.to_string());
                }
            }
            syn::visit::visit_pat_type(self, node);
        }
    }
    let mut bindings = Bindings {
        names: BTreeSet::new(),
        is_spider_own_crate_source,
        bare_website_trusted,
        _marker: std::marker::PhantomData,
    };
    bindings.visit_file(file);
    bindings.names
}

/// Bypass classes closed (operator adversarial review, hardened again in
/// round 5): "an unrelated struct Website with crawl()", "an unrelated
/// variable named website calling crawl()", "an unrelated module/type
/// exposing the same method name" (including `mod Website { pub fn
/// crawl() {} }` — a *module*, not a struct at all, calling
/// `Website::crawl()` from elsewhere previously matched on raw text
/// alone), "a generic crawl() call disconnected from the capability's
/// WIRED root". This version requires every receiver/callee-prefix
/// binding to resolve through the single shared
/// `canonical_type_owner_name` provenance model: for a `self`-receiver,
/// the enclosing `impl` block's self_ty must resolve to the canonical
/// type; for a named-variable receiver, the name must be in
/// `website_typed_bindings`; for an associated/qualified call, the
/// callee path's type prefix (everything before the trailing method
/// name) must independently resolve to the canonical type — never a bare
/// text match against the declared symbol string, which cannot
/// distinguish a module path from a type path at all. `ast_any_production_call`
/// is only ever invoked against known external shipping-artifact source
/// trees (never `spider`'s own crate source), so the canonical spelling
/// this file's "world" accepts is always `spider::website::Website` (see
/// `canonical_website_paths`).
fn ast_any_production_call(source: &str, symbol: &str, is_canonical_definition_site: bool) -> bool {
    let Ok(file) = syn::parse_file(source) else {
        return false;
    };
    let bare_website_trusted = file_proves_bare_website(&file, is_canonical_definition_site, false);
    let website_bindings = website_typed_bindings(&file, false, bare_website_trusted);
    let (expected_type, method) = split_type_qualifier(symbol);
    struct Calls<'a> {
        method: &'a str,
        expected_type: Option<&'a str>,
        website_bindings: &'a BTreeSet<String>,
        current_impl_type: Option<String>,
        bare_website_trusted: bool,
        hit: bool,
    }
    impl<'ast, 'a> Visit<'ast> for Calls<'a> {
        fn visit_item_fn(&mut self, node: &'ast ItemFn) {
            // Bypass class closed (Codex adversarial review): "#[test]
            // consumers inside src/" — a `#[test] fn foo() {
            // website.crawl() }` sitting directly in a shipping artifact's
            // `src/` tree (not wrapped in `#[cfg(test)]`) previously still
            // counted as a real production call site, even though `cargo
            // test`-only code is never part of the shipped binary.
            if attrs_have_cfg_test(&node.attrs) || is_test_attributed(&node.attrs) {
                return;
            }
            syn::visit::visit_item_fn(self, node);
        }
        fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
            // Bypass class closed: "test-only consumers inside
            // #[cfg(test)] modules" — a conventionally-named `mod tests`
            // (or one explicitly `#[cfg(test)]`-gated, covered
            // regardless of name) must never contribute a production
            // call site, matching the same convention already enforced
            // for `ast_contains_production_definition`.
            if attrs_have_cfg_test(&node.attrs) || node.ident == "tests" || node.ident == "test" {
                return;
            }
            syn::visit::visit_item_mod(self, node);
        }
        fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
            let previous = self.current_impl_type.take();
            self.current_impl_type = type_path_idents(&node.self_ty).and_then(|idents| {
                canonical_type_owner_name(&idents, false, self.bare_website_trusted)
            });
            syn::visit::visit_item_impl(self, node);
            self.current_impl_type = previous;
        }
        fn visit_expr_call(&mut self, node: &'ast ExprCall) {
            // Bypass class closed (Codex adversarial review, round 5,
            // exact demonstrated reproducer): `mod Website { pub fn
            // crawl() {} }` followed by `Website::crawl()` previously
            // satisfied this check on raw path-text equality alone —
            // `syn` cannot distinguish a module path from a type path at
            // a *reference* site (only at the definition can the two be
            // told apart), so "same identifier" was silently treated as
            // "same canonical symbol." The callee path's type prefix
            // (every segment before the trailing method name) is now
            // independently resolved through the shared
            // `canonical_type_owner_name` model — a decoy module or type
            // sharing only the bare name `Website`, with no affirmative
            // canonical-provenance proof, resolves to no owner at all,
            // regardless of what the call's raw text looks like.
            if let syn::Expr::Path(path) = &*node.func {
                let idents = path_idents(&path.path);
                if let Some((last, type_prefix)) = idents.split_last() {
                    if last == self.method && !type_prefix.is_empty() {
                        let owner = canonical_type_owner_name(
                            type_prefix,
                            false,
                            self.bare_website_trusted,
                        );
                        if owner.as_deref() == self.expected_type {
                            self.hit = true;
                        }
                    }
                }
            }
            syn::visit::visit_expr_call(self, node);
        }
        fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
            let method_matches = node.method == self.method;
            let receiver_is_bound_website = match &*node.receiver {
                syn::Expr::Path(p) if p.path.is_ident("self") => {
                    self.current_impl_type.as_deref() == Some(CANONICAL_PRODUCTION_TYPE)
                }
                syn::Expr::Path(p) => p
                    .path
                    .get_ident()
                    .is_some_and(|ident| self.website_bindings.contains(&ident.to_string())),
                _ => false,
            };
            if method_matches && receiver_is_bound_website {
                self.hit = true;
            }
            syn::visit::visit_expr_method_call(self, node);
        }
    }
    let mut calls = Calls {
        method,
        expected_type,
        website_bindings: &website_bindings,
        current_impl_type: None,
        bare_website_trusted,
        hit: false,
    };
    calls.visit_file(&file);
    calls.hit
}

fn artifact_enables_feature(cargo_toml_path: &Path, feature: &str) -> bool {
    manifest_enables_feature(&read(cargo_toml_path), feature)
}

fn manifest_enables_feature(contents: &str, feature: &str) -> bool {
    let Ok(table) = contents.parse::<toml::Table>() else {
        return false;
    };
    let doc = toml::Value::Table(table);
    let Some(features) = doc.get("features").and_then(|v| v.as_table()) else {
        return false;
    };
    let direct = features
        .get(feature)
        .and_then(|v| v.as_array())
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.as_str()
                    .is_some_and(|value| value == format!("spider/{feature}"))
            })
        });
    if direct {
        return true;
    }
    // A dependency feature table is also structural TOML, never a text
    // search. This covers manifests that enable spider's feature directly.
    doc.get("dependencies")
        .and_then(|v| v.get("spider"))
        .and_then(|v| v.get("features"))
        .and_then(|v| v.as_array())
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(feature)))
}

#[test]
fn production_reachable_claims_are_grep_verified_against_shipping_manifests() {
    let known = known_shipping_artifacts();
    for entry in load_ledger_entries() {
        let Some(production_reachable) = stage_table(&entry.doc, "PRODUCTION_REACHABLE") else {
            continue;
        };
        let verdict = str_field(production_reachable, "verdict").unwrap_or_else(|| {
            panic!(
                "{}: [stages.PRODUCTION_REACHABLE] missing `verdict`",
                entry.path.display()
            )
        });
        let feature_requirements = str_array(production_reachable, "feature_requirements");
        let shipping_artifacts = str_array(production_reachable, "shipping_artifacts");
        let entry_point_symbols = str_array(production_reachable, "entry_point_symbols");

        assert_eq!(
            bool_field(production_reachable, "siblings_enumerated"),
            Some(true),
            "{}: PRODUCTION_REACHABLE.siblings_enumerated must be explicitly true",
            entry.path.display()
        );
        assert!(
            production_reachable
                .get("siblings")
                .and_then(|value| value.as_array())
                .is_some(),
            "{}: PRODUCTION_REACHABLE.siblings must be present (an array, possibly empty)",
            entry.path.display()
        );
        assert!(
            !str_field(production_reachable, "siblings_note")
                .unwrap_or_default()
                .is_empty(),
            "{}: PRODUCTION_REACHABLE.siblings_note must explain the siblings list",
            entry.path.display()
        );

        // An artifact only counts as "actually reachable" when its
        // Cargo.toml enables one of feature_requirements AND its own
        // (non-test) src/ calls one of the WIRED-bound entry points.
        let mut actually_reachable_via: Vec<&str> = Vec::new();
        if !feature_requirements.is_empty() {
            for (name, cargo_toml) in &known {
                if !cargo_toml.is_file() {
                    continue;
                }
                let feature_enabled = feature_requirements
                    .iter()
                    .any(|feature| artifact_enables_feature(cargo_toml, feature));
                let entry_point_called = entry_point_symbols
                    .iter()
                    .any(|symbol| artifact_source_calls_symbol(name, symbol));
                if feature_enabled && entry_point_called {
                    actually_reachable_via.push(name);
                }
            }
        }

        match verdict {
            "MET" => {
                assert!(
                    !shipping_artifacts.is_empty(),
                    "{}: PRODUCTION_REACHABLE.verdict = MET requires a non-empty \
                     shipping_artifacts list",
                    entry.path.display()
                );
                for claimed in &shipping_artifacts {
                    assert!(
                        actually_reachable_via
                            .iter()
                            .any(|actual| actual == claimed),
                        "{}: claims shipping_artifacts includes {claimed:?}, but no \
                         WIRED-bound entry point is both enabled and actually called in \
                         {claimed}'s own (non-test) source — false MET rejected",
                        entry.path.display()
                    );
                }
            }
            "NOT_MET" => {
                assert!(
                    shipping_artifacts.is_empty(),
                    "{}: PRODUCTION_REACHABLE.verdict = NOT_MET but shipping_artifacts is \
                     non-empty",
                    entry.path.display()
                );
                assert!(
                    actually_reachable_via.is_empty(),
                    "{}: claims NOT_MET, but {actually_reachable_via:?} now both enable and \
                     call a WIRED-bound entry point — this ledger entry is stale",
                    entry.path.display()
                );
            }
            other => panic!(
                "{}: PRODUCTION_REACHABLE.verdict must be \"MET\" or \"NOT_MET\", got {other:?}",
                entry.path.display()
            ),
        }
    }
}

#[test]
fn production_reachable_evidence_never_reuses_verified_test_paths_verbatim() {
    for entry in load_ledger_entries() {
        let Some(verified) = stage_table(&entry.doc, "VERIFIED") else {
            continue;
        };
        let Some(production_reachable) = stage_table(&entry.doc, "PRODUCTION_REACHABLE") else {
            continue;
        };
        let test_evidence: BTreeSet<String> = str_array(verified, "evidence").into_iter().collect();
        for artifact in str_array(production_reachable, "shipping_artifacts") {
            assert!(
                !test_evidence.contains(&artifact),
                "{}: PRODUCTION_REACHABLE.shipping_artifacts reuses a VERIFIED test-evidence \
                 entry {artifact:?} verbatim",
                entry.path.display()
            );
        }
    }
}

/// Rejects "prose/non-empty fields are insufficient" for
/// ADVERSARIALLY_VERIFIED: requires a `reviewed_commit` that is a real,
/// history-reachable commit, a `capability_id` that matches this ledger
/// entry's own `id` — binding the review evidence to *this* capability,
/// not a free-text claim that could be copy-pasted from an unrelated
/// frontier — and, via `assert_commit_binds_relevant_files`, the exact
/// same full evidence-file revision binding `CLOSED.closed_commit`
/// requires (Codex adversarial review: "either bind
/// ADVERSARIALLY_VERIFIED to the same relevant evidence revision model [as
/// CLOSED], or lower its guarantee so it cannot advance maturity based on
/// an unrelated ancestor" — this takes the former: an
/// ADVERSARIALLY_VERIFIED claim citing a real, reachable, but
/// content-stale ancestor is rejected exactly as a CLOSED claim would be).
#[test]
fn adversarially_verified_binds_a_real_reviewed_commit_to_this_capability() {
    for entry in load_ledger_entries() {
        let Some(adversarially_verified) = stage_table(&entry.doc, "ADVERSARIALLY_VERIFIED") else {
            continue;
        };
        let capability_id = str_field(&entry.doc, "id").unwrap();
        let claimed_capability_id = str_field(adversarially_verified, "capability_id")
            .unwrap_or_else(|| {
                panic!(
                    "{}: [stages.ADVERSARIALLY_VERIFIED] missing `capability_id`",
                    entry.path.display()
                )
            });
        assert_eq!(
            claimed_capability_id,
            capability_id,
            "{}: ADVERSARIALLY_VERIFIED.capability_id {claimed_capability_id:?} does not match \
             this file's own id {capability_id:?}",
            entry.path.display()
        );
        let reviewed_commit =
            str_field(adversarially_verified, "reviewed_commit").unwrap_or_else(|| {
                panic!(
                    "{}: [stages.ADVERSARIALLY_VERIFIED] missing `reviewed_commit`",
                    entry.path.display()
                )
            });
        assert_eq!(
            git_object_type(reviewed_commit).as_deref(),
            Some("commit"),
            "{}: ADVERSARIALLY_VERIFIED.reviewed_commit {reviewed_commit:?} is not a commit \
             object",
            entry.path.display()
        );
        assert!(
            git_commit_reachable_from_head(reviewed_commit),
            "{}: ADVERSARIALLY_VERIFIED.reviewed_commit {reviewed_commit:?} is not reachable \
             from HEAD",
            entry.path.display()
        );
        assert!(
            !str_array(adversarially_verified, "bypass_attempts").is_empty(),
            "{}: [stages.ADVERSARIALLY_VERIFIED] bypass_attempts must not be empty",
            entry.path.display()
        );
        assert_commit_binds_relevant_files(
            reviewed_commit,
            &entry,
            "ADVERSARIALLY_VERIFIED.reviewed_commit",
        );
    }
}

/// Rejects "raw workflow text/comments must not satisfy execution
/// evidence" and "gated/non-required/schedule-only commands must not
/// satisfy required CI_ENFORCED evidence": the declared command must
/// structurally match a real, parsed `run:` value of a step that is not
/// behind any `if:` gate — a YAML comment, or a live-network-gated step,
/// can no longer satisfy this.
///
/// Bypass class closed (Codex adversarial review): "finish the previously
/// deferred argv-schema decision... do not continue expanding shell-string
/// heuristics." `CI_ENFORCED` no longer carries a free-text `ci_command`
/// shell string at all. It declares its command as discrete, structurally
/// typed TOML fields — `package`, `lib`, `test_targets`, `feature_set`,
/// `positional_filters`, `exact`, `skip` — parsed with no shell semantics
/// whatsoever (a `#`/backslash/quoting trick in TOML data is just
/// characters in a string; there is no shell to reinterpret it). The real
/// workflow side is still, unavoidably, GitHub Actions shell text — that
/// cannot change — but it is only ever considered after
/// `executable_test_command` has confirmed the whole `run:` value is an
/// unambiguous direct `cargo test` invocation (see that function's and
/// `shell_text_is_unambiguous`'s doc comments), and is then reduced to the
/// exact same structural `TestSelection` representation via
/// `parse_test_selection`. The two are compared with `same_test_selection`
/// — field-by-field structural (order-independent, set-based) equality,
/// never string equality — so two commands with identical real cargo-test
/// meaning but superficially different text now match, and a shell trick
/// that changes real execution while preserving old string-equality
/// (or vice versa) is categorically impossible: there is no string
/// comparison left to fool.
#[test]
fn ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match() {
    let steps = load_workflow_steps();
    for entry in load_ledger_entries() {
        let Some(ci_enforced) = stage_table(&entry.doc, "CI_ENFORCED") else {
            continue;
        };
        let package = str_field(ci_enforced, "package").unwrap_or_else(|| {
            panic!(
                "{}: [stages.CI_ENFORCED] missing `package`",
                entry.path.display()
            )
        });
        let lib = bool_field(ci_enforced, "lib").unwrap_or_else(|| {
            panic!(
                "{}: [stages.CI_ENFORCED] missing `lib` (must be an explicit bool)",
                entry.path.display()
            )
        });
        let test_targets: BTreeSet<String> =
            str_array(ci_enforced, "test_targets").into_iter().collect();
        let feature_set = str_field(ci_enforced, "feature_set").unwrap_or_else(|| {
            panic!(
                "{}: [stages.CI_ENFORCED] missing `feature_set`",
                entry.path.display()
            )
        });
        let features: BTreeSet<String> =
            feature_set.split_whitespace().map(str::to_string).collect();
        let positional_filters = str_array(ci_enforced, "positional_filters");
        let exact = bool_field(ci_enforced, "exact").unwrap_or_else(|| {
            panic!(
                "{}: [stages.CI_ENFORCED] missing `exact` (must be an explicit bool)",
                entry.path.display()
            )
        });
        let skip = str_array(ci_enforced, "skip");
        // Bypass class closed (Codex adversarial review): "CI_ENFORCED
        // evidence must retain the exact workflow file that supplied the
        // qualifying command... ci_workflow_file must actually constrain
        // evidence discovery." Previously this field was documented in
        // the ledger schema and present in every fixture, but never once
        // read by the verifier — a matching command living in *any*
        // workflow file anywhere under `.github/workflows/` satisfied
        // CI_ENFORCED, including one that this repository's real CI
        // configuration never actually runs.
        let ci_workflow_file = str_field(ci_enforced, "ci_workflow_file").unwrap_or_else(|| {
            panic!(
                "{}: [stages.CI_ENFORCED] missing `ci_workflow_file`",
                entry.path.display()
            )
        });
        assert!(
            lib || !test_targets.is_empty(),
            "{}: [stages.CI_ENFORCED] must select `lib = true` or at least one entry in \
             `test_targets` — a command that selects nothing cannot be evidence of anything",
            entry.path.display()
        );

        let declared = TestSelection {
            package: Some(package.to_string()),
            lib,
            test_targets: test_targets.clone(),
            features: features.clone(),
            skip_patterns: skip.clone(),
            positional_filters: positional_filters.clone(),
            exact,
        };

        let matching_step = steps.iter().find(|step| {
            step.file == ci_workflow_file
                && step.applicable
                && !step.gated
                && step.run.as_deref().is_some_and(executable_test_command)
                && step.run.as_deref().is_some_and(|run| {
                    parse_test_selection(run)
                        .is_some_and(|selection| same_test_selection(&selection, &declared))
                })
        });
        let matching_step = matching_step.unwrap_or_else(|| {
            panic!(
                "{}: CI_ENFORCED's declared structural command (package={package:?}, lib={lib}, \
                 test_targets={test_targets:?}, feature_set={feature_set:?}, \
                 positional_filters={positional_filters:?}, exact={exact}, skip={skip:?}) does \
                 not structurally match any real, parsed, non-gated, actually-executable `run:` \
                 step in the declared ci_workflow_file ({ci_workflow_file:?}) — a matching \
                 command sitting in a *different* workflow file, a comment, a gated step, or a \
                 step whose real cargo-test semantics differ in any field does not count",
                entry.path.display()
            )
        });
        assert!(
            matching_step.applicable,
            "{}: CI_ENFORCED command is not in a push/PR workflow",
            entry.path.display()
        );

        // Bind CI_ENFORCED to VERIFIED: the declared structural command
        // must actually be capable of running every VERIFIED test
        // evidence entry that lives in the same target (mirrors the
        // structural check in
        // verified_stage_evidence_resolves_to_real_test_definitions), and
        // must not then filter that test back out via --skip/--exact/a
        // non-matching positional filter (all three checked directly
        // against the declared structured fields, no `ci_command` string
        // involved).
        if let Some(verified) = stage_table(&entry.doc, "VERIFIED") {
            for item in str_array(verified, "evidence") {
                let Some((file_part, symbol)) = item.split_once(':') else {
                    continue;
                };
                let bare_symbol = symbol
                    .trim_start_matches(':')
                    .rsplit("::")
                    .next()
                    .unwrap_or(symbol);
                if file_part.starts_with("spider/src/") {
                    assert!(
                        declared.lib,
                        "{}: CI_ENFORCED does not declare lib = true, but VERIFIED cites \
                         {item:?} from spider/src",
                        entry.path.display()
                    );
                } else if let Some(test_binary) = file_part
                    .strip_prefix("spider/tests/")
                    .and_then(|rest| rest.strip_suffix(".rs"))
                {
                    assert!(
                        declared.test_targets.contains(test_binary),
                        "{}: CI_ENFORCED.test_targets does not include {test_binary:?}, but \
                         VERIFIED cites {item:?}",
                        entry.path.display()
                    );
                }
                let skip_excludes = declared.skip_patterns.iter().any(|pattern| {
                    bare_symbol.contains(pattern.as_str()) || pattern.contains(bare_symbol)
                });
                let exact_excludes = declared.exact
                    && !declared.positional_filters.is_empty()
                    && !declared.positional_filters.iter().any(|filter| {
                        filter == bare_symbol || filter.ends_with(&format!("::{bare_symbol}"))
                    });
                let non_exact_positional_excludes = !declared.exact
                    && !declared.positional_filters.is_empty()
                    && !declared.positional_filters.iter().any(|filter| {
                        bare_symbol.contains(filter.as_str()) || filter.contains(bare_symbol)
                    });
                assert!(
                    !skip_excludes && !exact_excludes && !non_exact_positional_excludes,
                    "{}: CI_ENFORCED selects the right target for VERIFIED evidence {item:?}, \
                     but its own skip/exact/positional-filter fields exclude the exact test \
                     {bare_symbol:?} from actually running",
                    entry.path.display()
                );
            }
        }
    }
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn assert_proof_capability(entry: &LedgerFile, record: &toml::Value, class: &str) {
    let id = str_field(&entry.doc, "id").expect("ledger id was checked separately");
    assert_eq!(
        str_field(record, "capability_id"),
        Some(id),
        "{}: {class} proof must be attached to its own capability {id:?}",
        entry.path.display()
    );
}

fn ci_command_identity(table: &toml::Value) -> String {
    let mut tests = str_array(table, "test_targets");
    let mut features: Vec<_> = str_field(table, "feature_set")
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let mut filters = str_array(table, "positional_filters");
    let mut skips = str_array(table, "skip");
    tests.sort();
    features.sort_unstable();
    filters.sort();
    skips.sort();
    format!(
        "package={};lib={};tests={};features={};filters={};exact={};skip={}",
        str_field(table, "package").unwrap_or_default(),
        bool_field(table, "lib").unwrap_or(false),
        tests.join(","),
        features.join(","),
        filters.join(","),
        bool_field(table, "exact").unwrap_or(false),
        skips.join(",")
    )
}

/// Proof class validation is deliberately independent from maturity. A
/// configured CI command is CI_ENFORCED; only an explicit, commit-bound real
/// Actions run record is CI_PROVEN. Operator and live-environment records are
/// likewise distinct, and UNPROVEN is only an honest absence marker.
#[test]
fn proof_class_records_are_typed_bound_and_non_substitutable() {
    for entry in load_ledger_entries() {
        let required = str_array(&entry.doc, "required_proof_classes");
        assert!(
            !required.is_empty(),
            "{}: required_proof_classes must explicitly describe closure proof",
            entry.path.display()
        );
        let required_set: BTreeSet<_> = required.iter().map(String::as_str).collect();
        assert_eq!(
            required_set.len(),
            required.len(),
            "{}: required_proof_classes contains duplicates",
            entry.path.display()
        );
        for class in &required {
            assert!(
                PROOF_CLASSES.contains(&class.as_str()),
                "{}: unknown required proof class {class:?}",
                entry.path.display()
            );
            assert_ne!(
                class,
                "UNPROVEN",
                "{}: UNPROVEN records absence and can never be required proof",
                entry.path.display()
            );
        }
        if required_set.contains("LIVE_ENVIRONMENT_DEPENDENT") {
            assert!(
                required_set.contains("OPERATOR_OBSERVED"),
                "{}: live-environment classification cannot count as observation; require OPERATOR_OBSERVED too",
                entry.path.display()
            );
        }

        if let Some(record) = proof_table(&entry.doc, "CODE_PROVEN") {
            assert_proof_capability(&entry, record, "CODE_PROVEN");
            let commit = str_field(record, "commit")
                .unwrap_or_else(|| panic!("{}: CODE_PROVEN missing commit", entry.path.display()));
            assert!(
                commit_is_reachable(commit),
                "{}: CODE_PROVEN commit must be a reachable full commit SHA",
                entry.path.display()
            );
            assert!(
                !str_array(record, "evidence").is_empty(),
                "{}: CODE_PROVEN requires concrete evidence",
                entry.path.display()
            );
        }

        if let Some(record) = proof_table(&entry.doc, "CI_PROVEN") {
            assert_proof_capability(&entry, record, "CI_PROVEN");
            let commit = str_field(record, "commit").unwrap_or_default();
            let run_commit = str_field(record, "run_commit").unwrap_or_default();
            assert!(
                commit_is_reachable(commit),
                "{}: CI_PROVEN commit must be the reachable full SHA actually executed",
                entry.path.display()
            );
            assert_eq!(
                run_commit,
                commit,
                "{}: CI_PROVEN run_commit must exactly match the claimed executed commit",
                entry.path.display()
            );
            let workflow = str_field(record, "workflow").unwrap_or_default();
            let run_id = str_field(record, "run_id").unwrap_or_default();
            let run_url = str_field(record, "run_url").unwrap_or_default();
            let job = str_field(record, "job").unwrap_or_default();
            let step = str_field(record, "step").unwrap_or_default();
            let command_identity = str_field(record, "command_identity").unwrap_or_default();
            assert!(
                !workflow.is_empty()
                    && !run_id.is_empty()
                    && run_id.chars().all(|c| c.is_ascii_digit())
                    && run_url.contains("/actions/runs/")
                    && run_url.ends_with(run_id)
                    && !job.is_empty()
                    && !step.is_empty()
                    && !command_identity.is_empty(),
                "{}: CI_PROVEN requires workflow, numeric run_id, matching Actions run_url, job, step, and command_identity",
                entry.path.display()
            );
            assert_eq!(
                str_field(record, "conclusion"),
                Some("success"),
                "{}: CI_PROVEN conclusion must be success",
                entry.path.display()
            );
            let ci_enforced = stage_table(&entry.doc, "CI_ENFORCED").unwrap_or_else(|| {
                panic!(
                    "{}: CI_PROVEN cannot substitute for missing CI_ENFORCED",
                    entry.path.display()
                )
            });
            assert_eq!(
                ci_command_identity(ci_enforced),
                command_identity,
                "{}: CI_PROVEN command_identity must equal CI_ENFORCED command_identity",
                entry.path.display()
            );
            assert_eq!(
                str_field(ci_enforced, "ci_workflow_file"),
                Some(workflow),
                "{}: CI_PROVEN workflow must equal CI_ENFORCED workflow",
                entry.path.display()
            );
            let configured_step = load_workflow_steps().into_iter().any(|configured| {
                configured.file == workflow
                    && configured.job == job
                    && configured.name == step
                    && configured.applicable
                    && !configured.gated
                    && configured.run.as_deref().is_some_and(|run| {
                        parse_test_selection(run).is_some_and(|selection| {
                            ci_command_identity(ci_enforced) == command_identity
                                && same_test_selection(
                                    &selection,
                                    &TestSelection {
                                        package: str_field(ci_enforced, "package")
                                            .map(str::to_string),
                                        lib: bool_field(ci_enforced, "lib").unwrap_or(false),
                                        test_targets: str_array(ci_enforced, "test_targets")
                                            .into_iter()
                                            .collect(),
                                        features: str_field(ci_enforced, "feature_set")
                                            .unwrap_or_default()
                                            .split_whitespace()
                                            .map(str::to_string)
                                            .collect(),
                                        skip_patterns: str_array(ci_enforced, "skip"),
                                        positional_filters: str_array(
                                            ci_enforced,
                                            "positional_filters",
                                        ),
                                        exact: bool_field(ci_enforced, "exact").unwrap_or(false),
                                    },
                                )
                        })
                    })
            });
            assert!(configured_step, "{}: CI_PROVEN job/step/command identity does not match the required configured workflow step", entry.path.display());
        }

        if let Some(records) = entry
            .doc
            .get("proof")
            .and_then(|p| p.get("OPERATOR_OBSERVED"))
        {
            let records: Vec<&toml::Value> = if records.is_array() {
                records.as_array().unwrap().iter().collect()
            } else {
                vec![records]
            };
            assert!(
                !records.is_empty(),
                "{}: empty OPERATOR_OBSERVED",
                entry.path.display()
            );
            for record in records {
                assert_proof_capability(&entry, record, "OPERATOR_OBSERVED");
                let commit = str_field(record, "commit").unwrap_or_default();
                assert!(
                    commit_is_reachable(commit),
                    "{}: OPERATOR_OBSERVED needs a reachable full commit SHA",
                    entry.path.display()
                );
                for field in ["command", "purpose", "result"] {
                    assert!(
                        !str_field(record, field)
                            .unwrap_or_default()
                            .trim()
                            .is_empty(),
                        "{}: OPERATOR_OBSERVED missing concrete {field}",
                        entry.path.display()
                    );
                }
            }
        }

        if let Some(record) = proof_table(&entry.doc, "LIVE_ENVIRONMENT_DEPENDENT") {
            assert_proof_capability(&entry, record, "LIVE_ENVIRONMENT_DEPENDENT");
            assert!(
                !str_array(record, "requirements").is_empty(),
                "{}: live classification needs environment requirements",
                entry.path.display()
            );
            assert_eq!(
                bool_field(record, "observed"),
                Some(false),
                "{}: classification alone must say observed = false",
                entry.path.display()
            );
        }

        if let Some(record) = proof_table(&entry.doc, "UNPROVEN") {
            assert_proof_capability(&entry, record, "UNPROVEN");
            assert!(
                !str_array(record, "missing").is_empty(),
                "{}: UNPROVEN must name missing proof",
                entry.path.display()
            );
            assert!(
                !str_field(record, "reason").unwrap_or_default().is_empty(),
                "{}: UNPROVEN must explain why",
                entry.path.display()
            );
        }
    }
}

#[test]
fn closed_requires_every_declared_proof_class_independently() {
    for entry in load_ledger_entries() {
        if stage_table(&entry.doc, "CLOSED").is_none() {
            continue;
        }
        let required = str_array(&entry.doc, "required_proof_classes");
        for class in &required {
            let satisfied = match class.as_str() {
                "CODE_PROVEN" | "CI_PROVEN" | "LIVE_ENVIRONMENT_DEPENDENT" => {
                    proof_table(&entry.doc, class).is_some()
                }
                "OPERATOR_OBSERVED" => entry
                    .doc
                    .get("proof")
                    .and_then(|p| p.get("OPERATOR_OBSERVED"))
                    .is_some(),
                "UNPROVEN" => false,
                _ => false,
            };
            assert!(
                satisfied,
                "{}: CLOSED missing required independent proof class {class}",
                entry.path.display()
            );
        }
        if required.iter().any(|c| c == "CI_PROVEN") {
            assert!(
                stage_table(&entry.doc, "CI_ENFORCED").is_some(),
                "{}: CI_PROVEN cannot replace CI_ENFORCED maturity",
                entry.path.display()
            );
        }
    }
}

/// Rejects "closure_commit must resolve to a commit object, not merely
/// any git object" (a blob or tree SHA previously passed `git cat-file
/// -e`) and requires it be reachable from `HEAD` — the intended history,
/// not a dangling or unrelated-branch commit.
#[test]
fn closed_stage_commit_is_a_real_commit_reachable_from_history() {
    for entry in load_ledger_entries() {
        let Some(closed) = stage_table(&entry.doc, "CLOSED") else {
            continue;
        };
        let commit = str_field(closed, "closed_commit").unwrap_or_else(|| {
            panic!(
                "{}: [stages.CLOSED] missing `closed_commit`",
                entry.path.display()
            )
        });
        assert!(
            commit.len() >= 7
                && commit
                    .chars()
                    .all(|character| character.is_ascii_hexdigit()),
            "{}: CLOSED.closed_commit {commit:?} does not look like a hex SHA",
            entry.path.display()
        );
        assert_eq!(
            git_object_type(commit).as_deref(),
            Some("commit"),
            "{}: CLOSED.closed_commit {commit:?} does not resolve to a commit object (a blob \
             or tree SHA must not satisfy this)",
            entry.path.display()
        );
        assert!(
            git_commit_reachable_from_head(commit),
            "{}: CLOSED.closed_commit {commit:?} is a real commit but is not reachable from \
             HEAD",
            entry.path.display()
        );
        assert_commit_binds_relevant_files(commit, &entry, "CLOSED.closed_commit");
    }
}

/// Every file this capability's closure claim actually rests on — not
/// merely the ledger entry and the verifier's own source: the ledger
/// entry itself, the live-network registry, `closure_harness.rs` and its
/// companion test binaries, the CI workflow, every known shipping
/// artifact's manifest, and every file named in this entry's own
/// IMPLEMENTED/VERIFIED evidence.
///
/// `expected_path`s that live under `ledger_dir()` (the ledger entry
/// itself, and the live-network registry) are read via that
/// env-override-aware function rather than a hardcoded
/// `workspace_root()`-relative path, so revision-binding checks built on
/// this remain exercisable against a
/// `CLOSURE_HARNESS_LEDGER_DIR_OVERRIDE`-pointed fixture directory (see
/// closure_harness_behavioral_contract.rs) — a hardcoded real path there
/// would always compare the *real* repo's file, silently defeating
/// fixture-based testing. Every other path is a fixed, non-ledger file
/// always read from the real workspace.
fn closure_relevant_files(entry: &LedgerFile) -> Vec<(PathBuf, String)> {
    let mut relevant_files: Vec<(PathBuf, String)> = vec![
        (
            ledger_dir().join(format!("{}.toml", entry.filename)),
            format!("docs/frontier/ledger/{}.toml", entry.filename),
        ),
        (
            ledger_dir().join("LIVE_NETWORK_TESTS.toml"),
            "docs/frontier/ledger/LIVE_NETWORK_TESTS.toml".to_string(),
        ),
        (
            workspace_root().join("spider/tests/closure_harness.rs"),
            "spider/tests/closure_harness.rs".to_string(),
        ),
        (
            workspace_root().join("spider/tests/closure_harness_behavioral_contract.rs"),
            "spider/tests/closure_harness_behavioral_contract.rs".to_string(),
        ),
        (
            workspace_root().join("spider/tests/closure_harness_integrity.rs"),
            "spider/tests/closure_harness_integrity.rs".to_string(),
        ),
        // Feature/cfg resolution for every WIRED/IMPLEMENTED evidence
        // check universally depends on `spider/Cargo.toml`'s own
        // `[features]` table (Codex adversarial review: "spider/Cargo.toml
        // where feature/cfg semantics depend on it").
        (
            workspace_root().join("spider/Cargo.toml"),
            "spider/Cargo.toml".to_string(),
        ),
    ];
    for (name, cargo_toml) in known_shipping_artifacts() {
        if cargo_toml.is_file() {
            relevant_files.push((cargo_toml, format!("{name}/Cargo.toml")));
        }
    }
    // Bypass class closed (Codex adversarial review): "revision binding
    // update: include the actual workflow that supplied CI_ENFORCED
    // evidence, not a hardcoded rust.yml." Bound to the ledger's own
    // declared `ci_workflow_file` — the exact file
    // `ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match`
    // required the qualifying command to live in — rather than a fixed
    // path. When no `[stages.CI_ENFORCED]` table exists yet (e.g. an
    // `ADVERSARIALLY_VERIFIED`-only entry, which precedes `CI_ENFORCED`
    // in the stage order), no workflow file is CI-derived evidence for
    // this capability at all, so none is bound — inventing a file this
    // entry's evidence never actually rested on would itself be a false
    // claim of provenance.
    if let Some(ci_enforced) = stage_table(&entry.doc, "CI_ENFORCED") {
        if let Some(ci_workflow_file) = str_field(ci_enforced, "ci_workflow_file") {
            relevant_files.push((
                workspace_root().join(ci_workflow_file),
                ci_workflow_file.to_string(),
            ));
        }
    }
    for stage_name in ["IMPLEMENTED", "VERIFIED"] {
        if let Some(table) = stage_table(&entry.doc, stage_name) {
            for item in str_array(table, "evidence") {
                if let Some((file_part, _)) = item.split_once(':') {
                    relevant_files.push((workspace_root().join(file_part), file_part.to_string()));
                }
            }
        }
    }
    // Every WIRED intermediate hop's own file — not only the terminal
    // hop, which happens to already be covered by IMPLEMENTED evidence
    // above (Codex adversarial review: "every WIRED intermediate source
    // file"). A historical commit whose ledger/harness bytes match but
    // whose call-chain source has since drifted must fail this binding,
    // not only a commit whose *terminal* file drifted.
    if let Some(wired) = stage_table(&entry.doc, "WIRED") {
        for chain in str_array(wired, "callers") {
            for hop in chain.split("->").map(str::trim) {
                if let Some((file_part, _)) = hop.split_once(':') {
                    relevant_files.push((workspace_root().join(file_part), file_part.to_string()));
                }
            }
        }
    }
    // The exact shipping-artifact source file(s) a `PRODUCTION_REACHABLE
    // = MET` verdict rests on (Codex adversarial review: "shipping
    // artifact source files used for PRODUCTION_REACHABLE") — not the
    // whole `src/` tree, only the files this same harness independently
    // re-derives as actually calling a WIRED-bound entry point.
    if let Some(production_reachable) = stage_table(&entry.doc, "PRODUCTION_REACHABLE") {
        if str_field(production_reachable, "verdict") == Some("MET") {
            for artifact in str_array(production_reachable, "shipping_artifacts") {
                for entry_point in str_array(production_reachable, "entry_point_symbols") {
                    for file in artifact_source_files_calling_symbol(&artifact, &entry_point) {
                        let relative = file
                            .strip_prefix(workspace_root())
                            .map(|p| p.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_else(|_| file.to_string_lossy().replace('\\', "/"));
                        relevant_files.push((file, relative));
                    }
                }
            }
        }
    }
    relevant_files.sort_by(|a, b| a.1.cmp(&b.1));
    relevant_files.dedup_by(|a, b| a.1 == b.1);
    relevant_files
}

/// Asserts `commit` contains byte-identical copies, at that exact
/// revision, of every file `closure_relevant_files` names for `entry`.
/// Shared by both `CLOSED.closed_commit` and
/// `ADVERSARIALLY_VERIFIED.reviewed_commit` — bypass class closed (Codex
/// adversarial review): "review ADVERSARIALLY_VERIFIED binding... bind it
/// to the same relevant evidence revision model [as CLOSED]." A
/// historical ancestor whose ledger and harness bytes happen to match
/// today's but whose implementation or test file has since drifted fails
/// this check under either stage.
fn assert_commit_binds_relevant_files(commit: &str, entry: &LedgerFile, field_name: &str) {
    for (expected_path, relative) in closure_relevant_files(entry) {
        let expected = read(&expected_path);
        let output = Command::new("git")
            .arg("-C")
            .arg(workspace_root())
            .args(["show", &format!("{commit}:{relative}")])
            .output()
            .unwrap_or_else(|error| panic!("git show failed: {error}"));
        assert!(
            output.status.success() && output.stdout == expected.as_bytes(),
            "{}: {field_name}'s revision does not contain the exact current {relative} — a \
             stale evidence file at the claimed revision means this is not the revision this \
             harness actually just verified",
            entry.path.display()
        );
    }
}

/// Rejects "implementation/test evidence without production reachability":
/// ADVERSARIALLY_VERIFIED/CI_ENFORCED/CLOSED must be omitted entirely
/// whenever PRODUCTION_REACHABLE.verdict isn't MET.
#[test]
fn later_stages_are_withheld_when_production_reachable_is_not_met() {
    for entry in load_ledger_entries() {
        let production_reachable_met = stage_table(&entry.doc, "PRODUCTION_REACHABLE")
            .and_then(|table| str_field(table, "verdict"))
            == Some("MET");
        if production_reachable_met {
            continue;
        }
        for later_stage in ["ADVERSARIALLY_VERIFIED", "CI_ENFORCED", "CLOSED"] {
            assert!(
                stage_table(&entry.doc, later_stage).is_none(),
                "{}: [stages.{later_stage}] is present but PRODUCTION_REACHABLE.verdict is \
                 not MET",
                entry.path.display()
            );
        }
    }
}

#[test]
fn claimed_stage_never_exceeds_the_harness_computed_stage() {
    for entry in load_ledger_entries() {
        let claimed = str_field(&entry.doc, "stage")
            .unwrap_or_else(|| panic!("{}: missing top-level `stage`", entry.path.display()));
        let claimed_index = STAGE_ORDER
            .iter()
            .position(|stage| *stage == claimed)
            .unwrap_or_else(|| {
                panic!(
                    "{}: `stage` = {claimed:?} is not one of {STAGE_ORDER:?}",
                    entry.path.display()
                )
            });

        let mut true_index = 0usize;
        for (index, stage_name) in STAGE_ORDER.iter().enumerate() {
            if *stage_name == "PRODUCTION_REACHABLE" {
                let met = stage_table(&entry.doc, stage_name)
                    .and_then(|table| str_field(table, "verdict"))
                    == Some("MET");
                if !met {
                    break;
                }
                true_index = index;
                continue;
            }
            if stage_table(&entry.doc, stage_name).is_some() {
                true_index = index;
            } else {
                break;
            }
        }

        assert!(
            claimed_index <= true_index,
            "{}: claims stage = {claimed:?} (position {claimed_index}) but the last stage \
             backed by valid evidence is {:?} (position {true_index})",
            entry.path.display(),
            STAGE_ORDER[true_index]
        );
    }
}

/// Self-check (HIGH 2): the tool that enforces "your CI_ENFORCED command
/// must be a real, non-gated `run:` step" must itself be provably invoked
/// that way — via `--test closure_harness` as a real flag inside a real,
/// parsed, non-gated step's `run:` value, not merely the string
/// "closure_harness" appearing anywhere in the workflow file (a comment
/// mentioning the binary's name, with the actual `--test` flag removed,
/// previously satisfied this).
///
/// Bypass class closed (Codex adversarial review): "the CI self-selection
/// check must use the same strict executable-command validation as
/// CI_ENFORCED — an `echo --test closure_harness` or equivalent
/// non-executing command must not satisfy self-enforcement." The prior
/// version only checked token adjacency anywhere in `run:`, which an
/// `echo --test closure_harness` (prints the flag, runs nothing) would
/// have satisfied just as well as a real invocation. `executable_test_command`
/// is now required on the *whole* `run:` value, exactly as CI_ENFORCED
/// itself requires. Also required: the same real-required-CI presence for
/// `closure_harness_behavioral_contract` (the independent behavioral
/// suite — not previously wired into CI at all), `architecture_guardrails`,
/// and `closure_harness_integrity` (the retained integrity sentinel) —
/// "required CI must explicitly run" all four, not merely
/// `closure_harness` alone.
#[test]
fn closure_harness_itself_is_a_real_required_test_flag_in_ci() {
    let steps = load_workflow_steps();
    for required_binary in [
        "closure_harness",
        "closure_harness_behavioral_contract",
        "closure_harness_integrity",
        "architecture_guardrails",
    ] {
        let found = steps.iter().any(|step| {
            step.applicable
                && !step.gated
                && step.run.as_deref().is_some_and(executable_test_command)
                && step.run.as_deref().is_some_and(|run| {
                    run.split_whitespace()
                        .collect::<Vec<_>>()
                        .windows(2)
                        .any(|pair| pair == ["--test", required_binary])
                })
        });
        assert!(
            found,
            "no real, non-gated, actually-executable workflow step has `--test \
             {required_binary}` as an actual, adjacent pair of tokens in its parsed `run:` \
             value — a comment, or a non-executing wrapper (`echo ...`), does not count"
        );
    }
}

#[test]
fn structural_parser_rejects_known_adversarial_fixtures() {
    let test_only = "#[cfg(test)] mod tests { fn secret() {} }";
    assert!(!ast_contains_production_definition(
        test_only,
        "secret",
        &BTreeSet::new(),
        true,
        false
    ));
    let raw = r##"const X: &str = r#"fn secret("#;"##;
    assert!(!ast_contains_production_definition(
        raw,
        "secret",
        &BTreeSet::new(),
        true,
        false
    ));
    let block = "/* fn secret() {} */ fn real() {}";
    assert!(!ast_contains_production_definition(
        block,
        "secret",
        &BTreeSet::new(),
        true,
        false
    ));

    // Module-qualified WIRED identity (Codex adversarial review): a bare
    // free-function name that exists in *two different modules* of the
    // same file is irreducible ambiguity — a `hop` inside `mod real` and
    // an unrelated same-named `hop` inside `mod unrelated` must not
    // collapse into a single, silently-resolved owner.
    let module_collision = "mod real { pub fn hop() {} } mod unrelated { pub fn hop() {} }";
    assert!(!ast_contains_production_definition(
        module_collision,
        "hop",
        &BTreeSet::new(),
        true,
        false
    ));
    // The same bare name defined in only *one* module is unambiguous.
    let single_module_definition = "mod real { pub fn hop() {} }";
    assert!(ast_contains_production_definition(
        single_module_definition,
        "hop",
        &BTreeSet::new(),
        true,
        false
    ));
    let unrelated = "struct Other; impl Other { fn hop(&self) {} } fn caller() { Other.hop(); }";
    assert!(!ast_function_calls(
        unrelated,
        "caller",
        "Other::hop",
        &BTreeSet::new(),
        true,
        false
    ));
    let same_name =
        "struct Other; impl Other { fn hop(&self) {} } fn caller() { let x = Other; x.hop(); }";
    assert!(!ast_function_calls(
        same_name,
        "caller",
        "Session::hop",
        &BTreeSet::new(),
        true,
        false
    ));
    let self_same_name =
        "struct Other; impl Other { fn caller(&self) { self.hop(); } fn hop(&self) {} }";
    assert!(!ast_function_calls(
        self_same_name,
        "caller",
        "Session::hop",
        &BTreeSet::new(),
        true,
        false
    ));
    let braces = "fn caller() { let _ = \"{ }\"; } fn hop() {}";
    assert!(!ast_function_calls(
        braces,
        "caller",
        "Session::hop",
        &BTreeSet::new(),
        true,
        false
    ));

    // Vendor cfg must not fail open (Codex adversarial review): under
    // `strict = false` (a vendored crate's unresolved feature namespace),
    // two mutually-exclusive `#[cfg(...)]`-gated overloads of the same
    // caller that *disagree* on whether they call the target must not be
    // combined into a fictional chain — neither can be trusted, so
    // adjacency must be refused.
    let vendor_disagreeing_overloads = "impl Vendor { #[cfg(feature = \"a\")] fn caller(&self) { self.target(); } #[cfg(feature = \"b\")] fn caller(&self) {} fn target(&self) {} }";
    assert!(!ast_function_calls(
        vendor_disagreeing_overloads,
        "caller",
        "target",
        &BTreeSet::new(),
        false,
        false
    ));
    // Agreeing cfg-gated overloads (every candidate calls the target) are
    // still trusted — the ambiguity is real, but it does not change the
    // yes/no answer to "does it call the target."
    let vendor_agreeing_overloads = "impl Vendor { #[cfg(feature = \"a\")] fn caller(&self) { self.target(); } #[cfg(feature = \"b\")] fn caller(&self) { self.target(); } fn target(&self) {} }";
    assert!(ast_function_calls(
        vendor_agreeing_overloads,
        "caller",
        "target",
        &BTreeSet::new(),
        false,
        false
    ));
    // A single, unconditionally-real (no cfg at all) definition is
    // trusted directly, same as before this fix.
    let vendor_single_definition =
        "impl Vendor { fn caller(&self) { self.target(); } fn target(&self) {} }";
    assert!(ast_function_calls(
        vendor_single_definition,
        "caller",
        "target",
        &BTreeSet::new(),
        false,
        false
    ));
    // Regression for a real latent bug found while hardening this:
    // `Iterator::all` on an *empty* candidate list is vacuously `true`
    // in Rust. A caller name that exists *nowhere* in the file must
    // resolve to "no adjacency proof," not accidentally "proof found"
    // merely because zero candidates were collected at some scanned
    // scope.
    let vendor_no_matching_candidate_at_all =
        "impl Vendor { fn unrelated_fn(&self) { self.target(); } fn target(&self) {} }";
    assert!(!ast_function_calls(
        vendor_no_matching_candidate_at_all,
        "caller",
        "target",
        &BTreeSet::new(),
        false,
        false
    ));
    // The same, but with the real candidate nested inside a sibling
    // module the empty-scope bug would previously have short-circuited
    // past without ever reaching.
    let vendor_real_candidate_in_nested_module = "fn unrelated_toplevel() {} mod inner { impl Vendor { fn caller(&self) { self.target(); } fn target(&self) {} } }";
    assert!(ast_function_calls(
        vendor_real_candidate_in_nested_module,
        "caller",
        "target",
        &BTreeSet::new(),
        false,
        false
    ));
    // Vendor cfg re-check after module-qualified identity (Codex
    // adversarial review): a bare caller name existing in two different
    // *unrelated* modules (not cfg-alternate overloads of the same
    // thing) must not let one module's genuine call stand in for the
    // whole symbol — the shared, mode-independent
    // `ast_contains_production_definition` existence gate (checked
    // before adjacency in the real WIRED loop) already rejects this as
    // ambiguous under `strict = false` exactly as it does under
    // `strict = true`, since module-path tracking applies to both.
    let vendor_module_ambiguous_bare_name =
        "mod real { fn hop() { real_target(); } fn real_target() {} } mod unrelated { fn hop() {} }";
    assert!(!ast_contains_production_definition(
        vendor_module_ambiguous_bare_name,
        "hop",
        &BTreeSet::new(),
        false,
        false
    ));

    // =================================================================
    // Canonical `Website` identity, round 5 (Codex adversarial review —
    // "same identifier != same canonical symbol... crate-local !=
    // canonical... maturity evidence must distinguish this identity from
    // every same-named decoy... if exact canonical provenance cannot be
    // proven, NOT_PROVEN"). Every fixture below exercises the single
    // shared `canonical_type_owner_name`/`file_proves_bare_website`
    // model, consumed identically by WIRED definition lookup
    // (`ast_contains_production_definition`), WIRED adjacency
    // (`ast_function_calls`), and PRODUCTION_REACHABLE
    // (`ast_any_production_call`). `ast_contains_production_definition`/
    // `ast_function_calls` exercise the *spider-own-crate* world
    // (`strict = true`, canonical spelling `crate::website::Website`);
    // `ast_any_production_call` is only ever invoked against known
    // external artifact source trees, so its world is always
    // `spider::website::Website`.
    // =================================================================

    // (A) Bare local struct Website + impl Website cannot prove WIRED —
    // the exact reproducer named by the reviewer. A same-file local
    // definition is *not* affirmative proof of canonical identity; only
    // the real definition site or a canonical import/qualified path is.
    let bare_local_struct_and_impl_website = "struct Website; impl Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(!ast_contains_production_definition(
        bare_local_struct_and_impl_website,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));
    assert!(!ast_function_calls(
        bare_local_struct_and_impl_website,
        "Website::crawl",
        "fake_next",
        &BTreeSet::new(),
        true,
        false
    ));
    // The bare-name collapse variants from earlier rounds — a qualified
    // `impl` self_ty nested in a crate-local module, or naming an
    // entirely unrelated path — are equally unproven, under both the
    // strict and vendor-permissive paths.
    let wired_decoy_nested_module_impl = "mod decoy { pub struct Website; } impl decoy::Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(!ast_contains_production_definition(
        wired_decoy_nested_module_impl,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));
    assert!(!ast_function_calls(
        wired_decoy_nested_module_impl,
        "Website::crawl",
        "fake_next",
        &BTreeSet::new(),
        true,
        false
    ));
    assert!(!ast_function_calls(
        wired_decoy_nested_module_impl,
        "Website::crawl",
        "fake_next",
        &BTreeSet::new(),
        false,
        false
    ));

    // (B) Crate-local imported decoy Website cannot prove WIRED —
    // `use crate::decoy::Website;` names a real, crate-local path, but
    // not the *canonical* one; "crate-local" and "canonical" are not the
    // same thing.
    let wired_crate_local_decoy_import = "use crate::decoy::Website; impl Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(!ast_contains_production_definition(
        wired_crate_local_decoy_import,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));
    assert!(!ast_function_calls(
        wired_crate_local_decoy_import,
        "Website::crawl",
        "fake_next",
        &BTreeSet::new(),
        true,
        false
    ));
    // Self/super-relative decoy imports are equally unproven — this
    // harness does not attempt `self::`/`super::`-relative resolution at
    // all (no real code uses either to reach `Website`), so neither is
    // ever on the canonical-paths allowlist regardless of what they
    // might resolve to in a real compiler.
    let wired_self_relative_decoy_import = "use self::decoy::Website; impl Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(!ast_contains_production_definition(
        wired_self_relative_decoy_import,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));
    let wired_super_relative_decoy_import = "use super::decoy::Website; impl Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(!ast_contains_production_definition(
        wired_super_relative_decoy_import,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));
    // A rename whose *renamed-to* name is `Website` but whose *source*
    // path is an unrelated decoy is exactly as unproven — `pub use
    // crate::decoy::Other as Website;` is not `crate::website::Website`
    // no matter what local name it is given.
    let wired_renamed_decoy_import = "pub use crate::decoy::Other as Website; impl Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(!ast_contains_production_definition(
        wired_renamed_decoy_import,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));
    // `enum Website`/`type Website = Other` shadows, and a `mod Website`
    // (a *module*, not a type, sharing the bare name) are equally
    // unproven — none of them is the real definition site, and none is
    // reached through a canonical import.
    let wired_enum_website_shadow = "enum Website { A } impl Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(!ast_contains_production_definition(
        wired_enum_website_shadow,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));
    let wired_type_alias_website_shadow = "type Website = Other; struct Other; impl Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(!ast_contains_production_definition(
        wired_type_alias_website_shadow,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));

    // (D) Sibling modules, each with their own bare `impl Website`
    // defining the same method, remain a genuine, irreducible ambiguity
    // even where both would otherwise be structurally well-formed.
    let wired_sibling_modules_same_impl_and_method = "mod a { pub struct Website; impl Website { pub fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} } } mod b { pub struct Website; impl Website { pub fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} } }";
    assert!(!ast_contains_production_definition(
        wired_sibling_modules_same_impl_and_method,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));

    // (F) Positive control: the genuine, bare, unqualified `impl
    // Website { ... }` shape at the canonical definition site — exactly
    // how the real canonical type is written in
    // `spider/src/website.rs` — must still be recognized as a real
    // WIRED root/adjacency. The hardened identity model does not fail
    // closed on the one shape it exists to keep working.
    let genuine_bare_impl_website =
        "impl Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(ast_contains_production_definition(
        genuine_bare_impl_website,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        true
    ));
    assert!(ast_function_calls(
        genuine_bare_impl_website,
        "Website::crawl",
        "fake_next",
        &BTreeSet::new(),
        true,
        true
    ));
    // Away from the definition site, an exact canonical `use` import —
    // `use crate::website::Website;` — is equally sufficient proof, and
    // so is an inline, fully-qualified `crate::website::Website` path
    // with no `use` at all.
    let genuine_crate_local_import_website = "use crate::website::Website; impl Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(ast_contains_production_definition(
        genuine_crate_local_import_website,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));
    assert!(ast_function_calls(
        genuine_crate_local_import_website,
        "Website::crawl",
        "fake_next",
        &BTreeSet::new(),
        true,
        false
    ));
    let genuine_inline_qualified_impl = "impl crate::website::Website { fn crawl(&mut self) { self.fake_next(); } fn fake_next(&mut self) {} }";
    assert!(ast_contains_production_definition(
        genuine_inline_qualified_impl,
        "Website::crawl",
        &BTreeSet::new(),
        true,
        false
    ));
    assert!(ast_function_calls(
        genuine_inline_qualified_impl,
        "Website::crawl",
        "fake_next",
        &BTreeSet::new(),
        true,
        false
    ));

    // =================================================================
    // Macro adjacency, round 3 (Codex adversarial review — "remove
    // arbitrary macro-derived call adjacency from maturity proof... do
    // not identify macros by final/bare name such as join!... calls
    // inside macro invocations do not establish WIRED adjacency"). There
    // is no macro allowlist at all any more — a call expression sitting
    // inside *any* macro invocation's arguments, under any macro name,
    // is structurally invisible to `ast_function_calls`'s adjacency scan
    // (`Calls` has no `visit_macro`/`visit_expr_macro` override, and the
    // default `syn::visit` behavior does not parse a macro's token
    // stream at all).
    // =================================================================

    let macro_string_literal_is_not_a_call =
        "fn caller() { some_macro!(\"target\"); } fn target() {}";
    assert!(!ast_function_calls(
        macro_string_literal_is_not_a_call,
        "caller",
        "target",
        &BTreeSet::new(),
        true,
        false
    ));
    let macro_bare_identifier_is_not_a_call = "fn caller() { some_macro!(target); } fn target() {}";
    assert!(!ast_function_calls(
        macro_bare_identifier_is_not_a_call,
        "caller",
        "target",
        &BTreeSet::new(),
        true,
        false
    ));
    // A real call parses cleanly as a macro argument, but no macro
    // invocation ever establishes adjacency any more, regardless of the
    // macro's name or how confidently its tokens parse as a call.
    let macro_real_call_as_argument_still_rejected =
        "fn caller() { some_macro!(\"literal\", target()); } fn target() {}";
    assert!(!ast_function_calls(
        macro_real_call_as_argument_still_rejected,
        "caller",
        "target",
        &BTreeSet::new(),
        true,
        false
    ));
    let stringify_never_executes_its_argument =
        "fn caller() { stringify!(target()); } fn target() {}";
    assert!(!ast_function_calls(
        stringify_never_executes_its_argument,
        "caller",
        "target",
        &BTreeSet::new(),
        true,
        false
    ));
    let discard_macro_unknown_execution_semantics =
        "fn caller() { discard!(target()); } fn target() {}";
    assert!(!ast_function_calls(
        discard_macro_unknown_execution_semantics,
        "caller",
        "target",
        &BTreeSet::new(),
        true,
        false
    ));
    // (E) The real production case this policy formerly special-cased —
    // a genuine method call passed as a `tokio::join!` macro argument —
    // is now, deliberately, also rejected: this harness has no way to
    // tell a real `tokio::join!` from a locally-shadowed decoy sharing
    // the exact same bare name.
    let macro_real_tokio_join_argument_no_longer_credited = "impl Website { async fn caller(&mut self) { tokio::join!(self.target(), self.other()); } async fn target(&mut self) {} async fn other(&mut self) {} }";
    assert!(!ast_function_calls(
        macro_real_tokio_join_argument_no_longer_credited,
        "caller",
        "target",
        &BTreeSet::new(),
        true,
        true
    ));
    // The reviewer's exact reproducer: a locally-defined `macro_rules!
    // join` shadowing the real `tokio::join!` under the identical bare
    // name — this harness cannot (and does not attempt to) tell the two
    // apart, so it credits neither.
    let locally_shadowed_join_macro = "macro_rules! join { ($($tokens:tt)*) => {}; } impl Website { async fn caller(&mut self) { join!(self.target()); } async fn target(&mut self) {} }";
    assert!(!ast_function_calls(
        locally_shadowed_join_macro,
        "caller",
        "target",
        &BTreeSet::new(),
        true,
        true
    ));

    // =================================================================
    // PRODUCTION_REACHABLE canonical identity — the same shared model,
    // consumed by `ast_any_production_call`'s self-receiver, named-
    // variable, and associated-call binding. This function is only ever
    // invoked against known external shipping-artifact source trees, so
    // its "world" is always `spider::website::Website`.
    // =================================================================

    // (C) Crate-local imported/re-exported decoy Website cannot prove
    // PRODUCTION_REACHABLE.
    let production_reachable_local_shadow_genuine_looking = "struct Website; impl Website { fn new(_u: &str) -> Self { Website } fn crawl(&mut self) {} } fn caller() { let mut w = Website::new(\"https://example.com\"); w.crawl(); }";
    assert!(!ast_any_production_call(
        production_reachable_local_shadow_genuine_looking,
        "Website::crawl",
        false
    ));
    let production_reachable_decoy_self_receiver = "impl crate::decoy::Website { fn helper(&mut self) { self.crawl(); } fn crawl(&mut self) {} }";
    assert!(!ast_any_production_call(
        production_reachable_decoy_self_receiver,
        "Website::crawl",
        false
    ));
    let production_reachable_decoy_nested_self_receiver = "mod decoy { pub struct Website; } impl decoy::Website { fn helper(&mut self) { self.crawl(); } fn crawl(&mut self) {} }";
    assert!(!ast_any_production_call(
        production_reachable_decoy_nested_self_receiver,
        "Website::crawl",
        false
    ));
    let production_reachable_bad_import = "use some_other_crate::Website; fn caller() { let mut w = Website::new(\"https://example.com\"); w.crawl(); }";
    assert!(!ast_any_production_call(
        production_reachable_bad_import,
        "Website::crawl",
        false
    ));
    let production_reachable_no_import_at_all =
        "fn caller() { let mut w = Website::new(\"https://example.com\"); w.crawl(); }";
    assert!(!ast_any_production_call(
        production_reachable_no_import_at_all,
        "Website::crawl",
        false
    ));
    let production_reachable_re_exported_rename = "pub use crate::decoy::Other as Website; fn caller() { let mut w = Website::new(\"https://example.com\"); w.crawl(); }";
    assert!(!ast_any_production_call(
        production_reachable_re_exported_rename,
        "Website::crawl",
        false
    ));
    let production_reachable_glob_import = "use unrelated::website::*; fn caller() { let mut w = Website::new(\"https://example.com\"); w.crawl(); }";
    assert!(!ast_any_production_call(
        production_reachable_glob_import,
        "Website::crawl",
        false
    ));
    let production_reachable_type_alias_shadow = "type Website = UnrelatedCrawler; struct UnrelatedCrawler; impl UnrelatedCrawler { fn crawl(&mut self) {} } fn caller() { let mut w = Website; w.crawl(); }";
    assert!(!ast_any_production_call(
        production_reachable_type_alias_shadow,
        "Website::crawl",
        false
    ));
    let production_reachable_external_crate_path =
        "fn caller() { let mut w = external::Website::new(\"https://example.com\"); w.crawl(); }";
    assert!(!ast_any_production_call(
        production_reachable_external_crate_path,
        "Website::crawl",
        false
    ));
    // A `crate::`-qualified path is crate-local, but this file's world
    // is the *external* artifact one — only `spider::website::Website`
    // is canonical here, so `crate::website::Website` is equally
    // unproven from this file's perspective.
    let production_reachable_crate_qualified_wrong_world = "fn caller() { let mut w = crate::website::Website::new(\"https://example.com\"); w.crawl(); }";
    assert!(!ast_any_production_call(
        production_reachable_crate_qualified_wrong_world,
        "Website::crawl",
        false
    ));

    // (D) A local `mod Website { pub fn crawl() {} }` — a *module*, not
    // the struct at all — must not prove reachability merely because
    // `Website::crawl()`'s raw text happens to match; the type prefix is
    // now independently resolved through the same canonical-identity
    // model, which cannot distinguish (nor needs to) a decoy module from
    // a decoy struct — neither is ever affirmatively proven.
    let production_reachable_module_masquerading_as_type =
        "mod Website { pub fn crawl() {} } fn shipping_consumer() { Website::crawl(); }";
    assert!(!ast_any_production_call(
        production_reachable_module_masquerading_as_type,
        "Website::crawl",
        false
    ));
    let other_qualified_call =
        "struct Other; impl Other { fn crawl() {} } fn caller() { Other::crawl(); }";
    assert!(!ast_any_production_call(
        other_qualified_call,
        "Website::crawl",
        false
    ));
    let newtype_qualified_call = "struct WebsiteWrapper(u8); impl WebsiteWrapper { fn crawl() {} } fn caller() { WebsiteWrapper::crawl(); }";
    assert!(!ast_any_production_call(
        newtype_qualified_call,
        "Website::crawl",
        false
    ));

    // (F) Positive controls: genuine canonical Website evidence remains
    // provable. `spider::website::Website` — a fully-qualified inline
    // path with no `use` import at all — is this file's one legitimate
    // spelling, since `ast_any_production_call` is only ever invoked
    // against known external artifact source trees.
    let genuine_fully_qualified_external = "fn caller() { let mut w = spider::website::Website::new(\"https://example.com\"); w.crawl(); }";
    assert!(ast_any_production_call(
        genuine_fully_qualified_external,
        "Website::crawl",
        false
    ));
    // The same, via a real `use spider::website::Website;` import —
    // exactly what `spider_cli`/`spider_mcp`/`spider_worker`'s own real
    // source does.
    let genuine_external_use_import = "use spider::website::Website; fn caller() { let mut w = Website::new(\"https://example.com\"); w.crawl(); }";
    assert!(ast_any_production_call(
        genuine_external_use_import,
        "Website::crawl",
        false
    ));
    let genuine_external_use_import_self_receiver =
        "use spider::website::Website; impl Website { fn helper(&mut self) { self.crawl(); } fn crawl(&mut self) {} }";
    assert!(ast_any_production_call(
        genuine_external_use_import_self_receiver,
        "Website::crawl",
        false
    ));
    let genuine_external_use_import_associated_call =
        "use spider::website::Website; fn shipping_consumer() { Website::crawl(); }";
    assert!(ast_any_production_call(
        genuine_external_use_import_associated_call,
        "Website::crawl",
        false
    ));
    // `is_canonical_definition_site = true` is still honored (used only
    // by fixtures simulating the definition file itself; the real
    // `artifact_source_files_calling_symbol` caller never passes `true`,
    // since `spider/src/website.rs` is never itself in an external
    // artifact's own `src/` tree).
    let canonical_site_defines_website_self_receiver = "struct Website; impl Website { fn new(_u: &str) -> Self { Website } fn crawl(&mut self) {} fn helper(&mut self) { self.crawl(); } }";
    assert!(ast_any_production_call(
        canonical_site_defines_website_self_receiver,
        "Website::crawl",
        true
    ));

    // "#[test] consumers inside src/" (item 6): a bare `#[test]`
    // function, or a conventionally-named `mod tests` with no explicit
    // `#[cfg(test)]`, must never establish shipping reachability — even
    // when the file otherwise has genuine canonical provenance.
    let bare_test_attr_consumer = "use spider::website::Website; #[test] fn t() { let mut w = Website::new(\"https://example.com\"); w.crawl(); }";
    assert!(!ast_any_production_call(
        bare_test_attr_consumer,
        "Website::crawl",
        false
    ));
    let unadorned_tests_mod_consumer = "use spider::website::Website; mod tests { fn t() { let mut w = Website::new(\"https://example.com\"); w.crawl(); } }";
    assert!(!ast_any_production_call(
        unadorned_tests_mod_consumer,
        "Website::crawl",
        false
    ));

    assert!(!manifest_enables_feature(
        "# spider/cache\n[features]\ncache = []",
        "cache"
    ));
    assert!(manifest_enables_feature(
        "[features]\ncache = [\"spider/cache\"]",
        "cache"
    ));
    assert!(!executable_test_command("echo cargo test -p spider --lib"));
    assert!(!executable_test_command(
        "if false; then cargo test -p spider --lib; fi"
    ));
    assert!(executable_test_command("cargo test -p spider --lib"));
    assert!(!executable_test_command(
        "cargo test -p spider --lib --test closure_harness --no-run"
    ));
    let dynamic = "#[tokio::test] async fn dynamic() { let h = \"real.invalid\"; Website::new(&format!(\"https://{h}\")).crawl().await; }";
    assert!(live_network_hosts_in(dynamic).is_some());
    let mixed = "#[tokio::test] async fn mixed() { TcpListener::bind(\"127.0.0.1:0\"); Website::new(\"https://choosealicense.com\").crawl().await; }";
    assert!(live_network_hosts_in(mixed).is_some());

    // Full evidence identity / cross-type definition binding (Codex
    // adversarial review, item 1/2): a qualified symbol
    // (`CapabilityType::apply`) must never be satisfied by an unrelated
    // type's same-named method.
    let cross_type = "impl Other { fn apply(&self) {} }";
    assert!(!ast_contains_production_definition(
        cross_type,
        "CapabilityType::apply",
        &BTreeSet::new(),
        true,
        false
    ));
    let same_type = "impl CapabilityType { fn apply(&self) {} }";
    assert!(ast_contains_production_definition(
        same_type,
        "CapabilityType::apply",
        &BTreeSet::new(),
        true,
        false
    ));

    // Ambiguous bare-symbol definitions across distinct owning scopes
    // (item 2/6: "fail closed rather than accepting ambiguous evidence")
    // must be rejected, not silently resolved to whichever one the AST
    // walk happens to visit first.
    let ambiguous = "fn shared_name() {} impl SomeType { fn shared_name(&self) {} }";
    assert!(!ast_contains_production_definition(
        ambiguous,
        "shared_name",
        &BTreeSet::new(),
        true,
        false
    ));
    // The same bare symbol repeated only as cfg-gated overloads of the
    // *same* free function (never a second owning scope) is not
    // ambiguous — this is the ordinary multi-overload case every real
    // WIRED chain in this codebase relies on.
    let same_owner_overloads =
        "#[cfg(feature = \"a\")] fn shared_name() {} #[cfg(feature = \"b\")] fn shared_name() {}";
    let declared_a: BTreeSet<String> = ["a".to_string()].into_iter().collect();
    assert!(ast_contains_production_definition(
        same_owner_overloads,
        "shared_name",
        &declared_a,
        true,
        false
    ));

    // Cfg-gated overload resolution (item 2: "do not combine mutually
    // exclusive cfg definitions into one fictional chain") — an overload
    // gated behind a feature that is neither compiled into this harness
    // nor declared for this capability must not count as a real
    // definition.
    let inactive_gate = "#[cfg(feature = \"never_declared_anywhere\")] fn gated_fn() {}";
    assert!(!ast_contains_production_definition(
        inactive_gate,
        "gated_fn",
        &BTreeSet::new(),
        true,
        false
    ));
    let active_gate = "#[cfg(feature = \"declared\")] fn gated_fn() {}";
    let declared_only: BTreeSet<String> = ["declared".to_string()].into_iter().collect();
    assert!(ast_contains_production_definition(
        active_gate,
        "gated_fn",
        &declared_only,
        true,
        false
    ));
    // Outside `spider/src/**` (strict = false), cfg gates are decorative:
    // a vendored crate's own, unresolvable feature namespace must not
    // silently exclude its real, compiled code (see `cfg_predicate_holds`'s
    // doc comment for the real `vendor/chromey` `_cache` case this
    // encodes).
    assert!(ast_contains_production_definition(
        inactive_gate,
        "gated_fn",
        &BTreeSet::new(),
        false,
        false
    ));

    // Module-qualified VERIFIED test identity (item 3): two different
    // modules' same-named test function must not collide.
    let module_collision =
        "mod module_a { mod tests { #[test] fn foo() {} } } mod module_b { mod tests { #[test] fn foo() {} } }";
    assert!(ast_contains_test_definition(
        module_collision,
        "module_a::tests::foo"
    ));
    assert!(ast_contains_test_definition(
        module_collision,
        "module_b::tests::foo"
    ));
    let only_a = "mod module_a { mod tests { #[test] fn foo() {} } }";
    assert!(!ast_contains_test_definition(
        only_a,
        "module_b::tests::foo"
    ));

    // Strict CI command representation (item 5): the shell-comment bypass.
    // A real shell truncates execution at an unquoted `#`; a naive text
    // scan does not. Both the ledger-facing `executable_test_command` and
    // any real workflow `run:` text must reject this outright rather than
    // silently trust text that would never actually execute as written.
    assert!(!executable_test_command(
        "cargo test -p spider --lib # --skip live_network_test"
    ));
    assert!(!shell_text_is_unambiguous(
        "cargo test -p spider --lib # --skip live_network_test"
    ));
    assert!(!executable_test_command(
        "cargo test -p spider --lib \\\n  --features chrome"
    ));
    assert!(shell_text_is_unambiguous("cargo test -p spider --lib"));

    // Cargo test selection (item 4): a non-exact positional filter that
    // does not match the cited test must be treated as excluding it —
    // `cargo test -p spider --test architecture_guardrails
    // unrelated_filter` must not prove execution of an unrelated VERIFIED
    // test in that binary.
    let unrelated_filter_selection = parse_test_selection(
        "cargo test -p spider --test architecture_guardrails unrelated_filter",
    )
    .expect("well-formed command must parse");
    assert!(selection_excludes(
        &unrelated_filter_selection,
        "no_shadow_credential_aware_cache_policy_in_cli_or_mcp"
    ));
    let matching_filter_selection = parse_test_selection(
        "cargo test -p spider --test architecture_guardrails no_shadow_credential",
    )
    .expect("well-formed command must parse");
    assert!(!selection_excludes(
        &matching_filter_selection,
        "no_shadow_credential_aware_cache_policy_in_cli_or_mcp"
    ));

    // Fail-closed execution-semantics rejection (Codex adversarial
    // review): `--no-run` compiles but never executes; `--list` prints
    // names and runs nothing; `--ignored` inverts normal selection. None
    // of these may be silently treated as "no filter, nothing excluded."
    assert!(parse_test_selection("cargo test -p spider --lib --no-run").is_none());
    assert!(parse_test_selection("cargo test -p spider --lib -- --list").is_none());
    assert!(parse_test_selection("cargo test -p spider --lib -- --ignored").is_none());
    assert!(parse_test_selection("cargo test -p spider --no-default-features --lib").is_none());
    assert!(parse_test_selection("cargo test -p spider --all-features --lib").is_none());
    assert!(parse_test_selection("cargo test -p spider --doc").is_none());
    assert!(parse_test_selection("cargo test -p spider --tests").is_none());
    assert!(parse_test_selection("cargo test -p spider --lib --features chrome").is_some());

    // Malformed recognized-option rejection (Codex adversarial review):
    // a trailing flag with no value must never be silently accepted as
    // "flag absent" — that would make `accepted TestSelection semantics
    // == actual Cargo/libtest execution semantics` false for exactly the
    // inputs where the two most obviously diverge.
    assert!(parse_test_selection("cargo test -p spider --lib -- --skip").is_none());
    assert!(parse_test_selection("cargo test -p spider --lib --features").is_none());
    assert!(parse_test_selection("cargo test -p -- --exact").is_none());
    assert!(parse_test_selection("cargo test -p spider --test -- --exact").is_none());

    // Bypass class closed (Codex adversarial review): "a recognized
    // value-taking option must reject another option token as its
    // value." A real `cargo test -p --lib` is not a request for a
    // package literally named `--lib` — Cargo itself parses `--lib` as
    // its own flag and rejects the command for `-p` missing a value.
    // This model must reach the same "not a valid selection" verdict,
    // not silently accept the option token as the package/target/
    // feature/skip-pattern value.
    assert!(parse_test_selection("cargo test -p --lib").is_none());
    assert!(parse_test_selection("cargo test --test --lib").is_none());
    assert!(parse_test_selection("cargo test -p spider --features --lib").is_none());
    assert!(parse_test_selection("cargo test -p spider -F --lib").is_none());
    assert!(parse_test_selection("cargo test -p spider --lib -- --skip --exact").is_none());

    // Real Cargo feature semantics (Codex adversarial review): repeated
    // `--features` merge; comma-separated features split into individual
    // tokens; `-F` is cargo's real short alias and must be modeled
    // exactly, not silently reinterpreted as a positional test filter.
    let repeated_features =
        parse_test_selection("cargo test -p spider --lib --features chrome --features cache")
            .expect("repeated --features must parse");
    assert_eq!(
        repeated_features.features,
        ["cache".to_string(), "chrome".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    let comma_features = parse_test_selection("cargo test -p spider --lib --features chrome,cache")
        .expect("comma-separated --features must parse");
    assert_eq!(
        comma_features.features,
        ["cache".to_string(), "chrome".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    let short_form_features = parse_test_selection("cargo test -p spider --lib -F chrome")
        .expect("-F must be modeled as an alias for --features");
    assert_eq!(
        short_form_features.features,
        ["chrome".to_string()].into_iter().collect::<BTreeSet<_>>()
    );

    // Every unknown short option must fail closed, not be silently
    // reinterpreted as a positional test-name filter.
    assert!(parse_test_selection("cargo test -p spider --lib -x").is_none());
    assert!(parse_test_selection("cargo test -p spider --lib -Zunstable").is_none());
}
