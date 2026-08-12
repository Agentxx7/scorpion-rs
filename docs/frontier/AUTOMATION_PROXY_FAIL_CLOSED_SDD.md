# Automation Proxy Fail-Closed SDD

Frontier: `SCORPION_SPIDER_AGENT_AUTOMATION_PROXY_FAIL_CLOSED_001`

Baseline: `11d7a09002959814fc87d341fcdccc1798876196`

## Execution states

- No proxy requested (`None` or an empty list): retain the canonical shared
  direct automation client.
- One or more valid proxies requested: parse every URL, attach every proxy to
  the existing 120-second client builder, successfully build the client, and
  store that resolved client before execution.
- Invalid proxy URL: return `EngineError::Http` with the original reqwest error;
  do not execute.
- Proxy client build failure: return `EngineError::Http` with the original
  reqwest error; do not execute.

Multiple proxies preserve the existing policy: each proxy is added in caller
order using `reqwest::Proxy::all`.

## Ownership and propagation

`RemoteMultimodalEngine::with_proxies` is the single resolver. It is fallible
and uses the existing canonical `EngineError`/`EngineResult` vocabulary. The
`run_remote_multimodal_with_page` construction boundary propagates resolution
failure before any request path can call `http_client()`.

## Guardrail

Acceptance rejects ignored proxy parse/build results and proves synthetically
that a swallowed resolution result is detected. Direct-client behavior remains
legal only for `None` or an empty proxy list.

## Scope

No model, Chrome, screenshot, retry, scripting, provider/tool, generic proxy,
worker, cache, or WATCH/MONITOR policy changes are permitted.

## Two-branch applicability

`TWO_BRANCH_NOT_APPLICABLE`: any optional or side-channel resolution state
allows `None` to retain its ambiguous meaning. A fallible resolver propagated at
the construction boundary is the only design that makes failure structurally
incapable of authorizing direct execution.

## Done

Focused acceptance, automation tests, architecture guardrails, relevant feature
checks, reverse consumers, clippy, rustfmt, and diff checks pass. Stop before
commit or push.
