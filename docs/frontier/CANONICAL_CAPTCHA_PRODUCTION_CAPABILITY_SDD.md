# Canonical CAPTCHA production capability

Frontier: `SCORPION_CANONICAL_CAPTCHA_MACHINE_READABLE_CAPABILITY_COVERAGE_001`

## 1. Purpose

This SDD names the design this frontier's ledger entry
(`docs/frontier/ledger/SCORPION_CANONICAL_CAPTCHA_MACHINE_READABLE_CAPABILITY_COVERAGE_001.toml`)
describes machine-readably. It does not introduce new production behavior —
every capability named here already shipped across six prior frontiers
(`SCORPION_CANONICAL_CAPTCHA_CHALLENGE_DETECTION_BINDING_001` through
`SCORPION_CANONICAL_CHROME_OOPIF_TARGET_VISIBILITY_IN_STREAMING_PIPELINE_001`,
closed at commit `d85cf3d0`). This document, and the ledger entry it backs,
is the first machine-readable, harness-checked description of that already-
shipped chain — closing the coverage gap those frontiers left behind, per
`spider/tests/closure_harness.rs` and
`docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md`.

## 2. The chain

```
PUBLIC_PROVIDER_SELECTION (CLI/MCP -> Configuration::captcha_provider)
  -> CHROME_EXECUTION (Website::crawl -> ... -> fetch_page_html_chrome_base_inner)
  -> CHALLENGE_DETECTION (detect_browser_challenge, probe_oopif_challenges)
  -> SNAPSHOT_MATERIALIZATION (BrowserChallengeSnapshot::capture / capture_in_frame)
  -> PROVIDER_ROUTING (DetectedBrowserChallenge::route -> route_detected_browser_challenge /
     route_detected_framed_browser_challenge)
  -> PALIGEMMA_LOCAL_RUNTIME (resolve_paligemma_provider, PaligemmaLocalCaptchaProvider)
  -> REAL_INFERENCE (PaligemmaLocalCaptchaProvider::solve)
  -> CANONICAL_SOLUTION (CaptchaSolution)
  -> BROWSER_ACTION (execute_browser_captcha_attempt / execute_browser_captcha_attempt_in_frame)
  -> POST_ACTION_OBSERVATION (re-detection confirming solved/unsolved)
```

Execution contexts — `TopLevel`, `SameSessionChild` (via `FrameContext::
resolve_same_session_child`), `Oopif` (via `FrameContext::resolve_child`) —
are classification metadata on the same chain, not three separate graphs.
Detection/materialization/action are the same functions for all three;
`FrameContext::classification` (`spider/src/features/frame_context.rs`)
is what distinguishes them at runtime.

## 3. What this frontier's ledger claims, and why not further

- `IMPLEMENTED`: every node above names a real, `syn`-verified definition.
- `VERIFIED`: real-Chrome integration tests across
  `browser_challenge_detection_real.rs`, `captcha_browser_binding_real.rs`,
  `captcha_browser_frame_action_real.rs`,
  `captcha_browser_production_binding_real.rs`,
  `captcha_browser_oopif_action_real.rs`,
  `captcha_browser_oopif_streaming_shipping_real.rs`,
  `captcha_browser_paligemma_real.rs`, and the provider-registry acceptance
  suites, each resolved to a real `#[tokio::test]` definition.
- `WIRED`: one AST-provable, non-macro chain from `Website::crawl` through
  to `detect_browser_challenge`. The real production behavior continues
  past that point into `.route()` and the provider registry — true, and
  documented — but the actual call site is `detected.route(...)`, a method
  call on a local variable named `detected`, not `self` and not a name
  matching this harness's `Type::method` receiver-binding heuristic (see
  `closure_harness.rs`'s `ast_function_calls`, `visit_expr_method_call`).
  This harness has no general local-variable type-inference (`syn` is not a
  type checker); proving that specific hop mechanically would require a
  capability this harness does not have today, exactly the same class of
  gap `SCORPION_CANONICAL_CREDENTIAL_CACHE_ISOLATION_001` hit at a
  `tokio::join!` macro boundary. `route_detected_browser_challenge`,
  `route_detected_framed_browser_challenge`, and the provider-construction
  functions remain truthfully claimed under `IMPLEMENTED` — real,
  `syn`-verified definitions — never re-claimed as `WIRED` through a hop
  this harness cannot independently prove.
- `PRODUCTION_REACHABLE`: `Website::crawl` is one of this harness's own
  `RECOGNIZED_PRODUCTION_ROOTS`, and both `spider_cli` (default features)
  and `spider_mcp` (default features) call it directly with `chrome`
  compiled in — `spider_mcp/src/tools/mod.rs:spawn_crawl_task`,
  `spider_cli/src/main.rs:crawl_with_mode`.
- Real CUDA/F16 PaliGemma inference through the shipping CLI (twice) and
  the shipping MCP server (once), with real OOPIF detection, real
  materialization, a real produced solution, a real browser action, and a
  real observed solved transition — genuinely observed in this same
  investigation, bound to commit `d85cf3d0` — is recorded as
  `OPERATOR_OBSERVED` + `LIVE_ENVIRONMENT_DEPENDENT` evidence, not `WIRED`.
  This is not a downgrade: per
  `CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md` section 2,
  `WIRED`/`PRODUCTION_REACHABLE` are static-analysis maturity classes,
  independent of real hardware observation, and a shipping runtime
  capability that depends on real GPU inference is exactly the case that
  proof class exists for.
- No `CI_PROVEN` record exists — no GitHub Actions run of this capability's
  evidence has been observed. `UNPROVEN` names this honestly. `ADVERSARIALLY_
  VERIFIED`, `CI_ENFORCED`, and `CLOSED` are not claimed.

## 4. Architecture decision gate

`PALIGEMMA_LOCAL_RUNTIME`'s only real definition is
`spider/src/features/paligemma_captcha.rs:PaligemmaLocalCaptchaProvider`,
gated `#[cfg(all(feature = "chrome", feature = "local_paligemma"))]`. No
`Qwen`/`Qwen3`/`Qwen3-VL` symbol exists anywhere in `spider/src/features/`
(grep-verified against this same commit) for this ledger to reference,
reject, or guard against reintroducing.
