# Canonical closure and production-reality harness

Frontier: `SCORPION_CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_001`
Status: `IMPLEMENTED / LOCAL-VERIFIED / CI-UNPROVEN`

## 1. Purpose and authority

This frontier checks capability maturity against the current repository. The
canonical verifier is `spider/tests/closure_harness.rs`; claims live in
`docs/frontier/ledger/*.toml`. The implementation is authoritative. This SDD
describes the final design, not the superseded lexical/grep implementation.

## 2. Lifecycle semantics

The strictly ordered stages are:

1. `DESIGNED`: the ledger names an existing SDD.
2. `IMPLEMENTED`: every `path:symbol` identifies a production Rust definition.
3. `VERIFIED`: every item names a real, structurally-resolved test
   definition, and the recorded command structurally selects it. `VERIFIED`
   means exactly this — "the cited test exists and the declared command
   would execute it" — and nothing more. It is not independently observed
   PASS evidence: the harness never runs the cited tests itself and never
   replays or trusts a prior run's result (see below).
4. `WIRED`: a production-rooted call chain reaches this capability's own
   `IMPLEMENTED` evidence, with every adjacency proven.
5. `PRODUCTION_REACHABLE`: a known shipping artifact both enables a required
   feature and calls the same entry point already proven as a `WIRED` root.
6. `ADVERSARIALLY_VERIFIED`: a bypass review is capability-bound and bound to
   a real commit reachable from `HEAD`.
7. `CI_ENFORCED`: a required push/PR workflow is configured to execute the
   exact feature command and select all cited `VERIFIED` evidence. This is a
   configuration fact, not evidence that GitHub executed the workflow.
8. `CLOSED`: all prior stages hold, closure evidence is commit-bound, and
   every capability-specific required proof class is independently present.

Table presence alone never advances maturity. When
`PRODUCTION_REACHABLE.verdict != "MET"`, `ADVERSARIALLY_VERIFIED`,
`CI_ENFORCED`, and `CLOSED` must be absent. The top-level claim may not outrun
the stage independently computed by the harness.

### Maturity and proof are independent

Every capability declares `required_proof_classes`. The canonical classes are:

- `CODE_PROVEN`: locally reproducible source, architecture, and deterministic
  test evidence bound to a repository commit.
- `CI_PROVEN`: a successful real GitHub Actions run bound to the exact commit,
  workflow, run, job, step, and required-command identity. Workflow YAML never
  creates this proof by itself.
- `OPERATOR_OBSERVED`: a concrete product command and result observed by an
  operator, bound to the exact observed commit. Fixture tests cannot supply it.
- `LIVE_ENVIRONMENT_DEPENDENT`: classification of required real network,
  provider, model, browser, or external infrastructure. Classification says
  `observed = false` and never represents an observation. A capability that
  requires this class must also require `OPERATOR_OBSERVED`.
- `UNPROVEN`: an explicit absence record naming missing proof. It cannot be a
  required class and cannot satisfy another class.

Green local tests therefore do not imply CLOSED, `CI_ENFORCED != CI_PROVEN`,
successful CI does not imply operator acceptance, and declaring a live
dependency does not imply that live behavior ran. Pure structural/library
capabilities may require CODE_PROVEN and CI_PROVEN only. Shipping runtime
capabilities may additionally require OPERATOR_OBSERVED and
LIVE_ENVIRONMENT_DEPENDENT.

## 3. Rust, Cargo, and reachability evidence

Ledger files are parsed with `toml`; each file stem must equal its top-level
`id`. `TEMPLATE.toml` and `LIVE_NETWORK_TESTS.toml` are not capability claims.

`IMPLEMENTED` evidence is parsed with `syn::parse_file`. Real functions,
methods, structs, enums, traits, types, constants, and statics outside test-only
modules/items qualify. Comments, strings, call sites, and test-only definitions
do not.

`VERIFIED` requires `test_only = true`. Evidence resolves to a real `#[test]`
or framework test function reached by walking its *entire* declared module
path as an exact, contiguous, in-order chain from the file's top level — an
evidence entry naming `module_a::tests::foo` can no longer be satisfied by an
unrelated `foo` defined under `module_b::tests` in the same file.
`last_verified_command` must select `--lib` for tests under `spider/src` and
the named integration target for tests under `spider/tests`; `last_verified_result`
is read by nothing in this file and can never advance maturity on its own —
only structural re-derivation of the evidence itself does.

**What `VERIFIED` does not mean** (Codex adversarial review: "do not imply
independently observed PASS evidence unless the harness actually
binds/replays such evidence"): this harness never executes the cited tests
and never replays, parses, or trusts any prior test-run output.
`last_verified_command`/`last_verified_result` are free-text fields a human
or a separate CI run fills in; they are informational context for a
reviewer, not inputs to any assertion here. A `VERIFIED` claim is
structural — the named test exists and would be selected — never an
independently confirmed pass. `CI_ENFORCED` proves only that a required,
non-gated workflow step is correctly configured. Genuine GitHub execution
evidence comes from a separate `CI_PROVEN` record after that exact command
succeeds in a real Actions run.

### Canonical `Website` identity (round 5 model)

Every prior round of this hardening effort (rounds 2-4) — despite closing a
real, demonstrated bypass each time — still ultimately answered the question
"is this `Website` the real one?" by asking "does this *look* right": is
there a locally-shadowed struct, a badly-qualified import path, an
unrecognized macro name. That is *name/shape inference*, and Codex's round-5
review named the underlying pattern precisely: whatever specific
disqualifying shape the harness didn't yet check for trivially walked
through, because the model's default answer to an unrecognized shape was
"trust it." A bare, same-file `struct Website; impl Website { ... }` with no
`use` import at all was never disqualified by any prior round's shadow/import
checks (there was no import to flag, and the *local* struct was assumed
non-adversarial); a `mod Website { pub fn crawl() {} }` followed by
`Website::crawl()` matched on raw path text, since `syn` cannot tell a module
path from a type path at a *reference* site at all.

This round replaces that posture entirely. Canonical `Website` identity is no
longer inferred from a shape; it is **proven affirmatively from a small,
fixed, exhaustively-enumerated set of known-good forms, or it is
`NOT_PROVEN`** — there is no "innocent until proven guilty" default anywhere
in this model any more. The one fact this harness treats as ground truth
(not derived) is that the real `Website` type is defined at
`spider/src/website.rs` (`CANONICAL_WEBSITE_DEFINITION_FILE`). Every other
reference to it — a bare name used elsewhere, a qualified path, an `impl`
block's self_ty — must trace back to that fact through exactly one of two
provable channels (`file_proves_bare_website`):
  1. the code lives *in* `spider/src/website.rs` itself — the definition
     site, trivially "the real one" since it is *where* `Website` is
     defined, not a reference to it;
  2. the code lives elsewhere and has a `use` import (or writes an inline
     path) whose *complete*, fully-qualified text is *exactly* one of the
     finite legitimate spellings for that file's own "world"
     (`canonical_website_paths`) — `crate::website::Website` from inside the
     `spider` crate itself, `spider::website::Website` from a known external
     shipping-artifact crate (`spider_cli`/`spider_mcp`/`spider_worker`,
     which depend on the published `spider` crate).

Every other spelling — `crate::decoy::Website`, `self::decoy::Website`,
`super::decoy::Website`, `external::Website`, a locally-defined
`struct Website`/`enum Website`/`type Website = Other`/`mod Website` with no
matching import at all, a rename whose *source* path is unrelated
(`pub use crate::decoy::Other as Website;`) — is not on that list and proves
nothing, regardless of how closely it resembles a trusted shape.
**Crate-local is not the same thing as canonical**: `use crate::decoy::Website;`
has a provably crate-local first segment, exactly like the real import, and
is still `NOT_PROVEN` — only the *complete* path, not merely its first
segment, is checked. This deliberately does not attempt `self::`/
`super::`-relative resolution (verifying either would require knowing a
referencing file's own module depth relative to crate root — genuine name
resolution, which a single isolated file's `syn::File` cannot provide; "do
not build a rust compiler"). No real code in this repository uses either
form to reach `Website` (grep-confirmed), so this costs no real coverage.

`canonical_type_owner_name` is the single shared resolution primitive
consumed identically by every maturity-bearing path in this harness: WIRED
definition lookup (`ast_contains_production_definition`'s owner tracking),
WIRED adjacency (`ast_function_calls`'s strict and vendor-permissive
receiver/associated-call resolution), and `PRODUCTION_REACHABLE`
(`ast_any_production_call`'s self-receiver, named-variable, and
associated-call binding). For any type name other than `Website`, only a
bare, single-segment path resolves at all — this harness has no
canonical-site concept for any other type, and ledger evidence never names
one through a qualified `impl` self_ty either, so a qualified form is exactly
as unsupported as it is for `Website`. There is deliberately no second,
weaker ownership model for any of these consumers — the same primitive,
the same rule, everywhere.

An associated/qualified call (`Website::crawl()`) is resolved the same way:
the callee path's *type prefix* — every segment before the trailing method
name — is independently checked through `canonical_type_owner_name`, never
by raw string equality against the declared symbol text. This closes the
exact reproducer named by Codex: `mod Website { pub fn crawl() {} }`
followed by `Website::crawl()` from elsewhere — `syn` cannot distinguish a
module path from a type path at a reference site (only at the definition
can the two be told apart), so a decoy module sharing only the bare name
`Website` is, correctly, exactly as unproven as a decoy struct would be;
neither needs to be specifically detected, since neither is ever
affirmatively proven canonical.

### WIRED

Every `WIRED.callers` chain has at least two `path:symbol` hops. Its root is a
real definition in production `src/` code. For every adjacent pair the harness
parses the caller file with `syn`, locates that caller's body, and proves the
declared callee call. Receiver/impl-type checks reject unrelated same-name
methods; qualified calls bind to their expected type via the canonical
identity model above. **Every hop, including the terminal, is independently
resolved to exactly one specific definition** — the terminal was previously
checked only as a callee (via adjacency and the IMPLEMENTED-evidence string
match), never independently for its own existence/ambiguity; it now goes
through the same existence gate as every intermediate hop.

The terminal is capability-bound: its complete `path:symbol` text (file and
fully qualified symbol, not a bare trailing method name) must equal one of
this ledger entry's own `IMPLEMENTED` evidence items verbatim. Real but
unrelated symbols in a different file or type, or names coexisting without
call adjacency, do not establish `WIRED`.

Every definition owner (a module-level item, or an `impl <Type>` block) is
tracked together with its complete enclosing module path — `""` for a file's
top level, `"real"` for `mod real { ... }`, and so on — not merely "free
function" vs. "impl method." A bare symbol resolves only when exactly one
module's definition matches; a symbol independently defined under two
different modules of the same file (`real::hop` vs. an unrelated
`unrelated::hop`, both spelled bare `hop` in the ledger) is irreducible,
unresolved ambiguity and fails closed rather than silently picking whichever
one the AST walk visits first. The same applies to qualified (`Type::method`)
lookups: if the same type name is independently `impl`-ed in two different
modules, that also fails closed. For an `impl` block naming the canonical
`Website` specifically, this module-collision gate composes with the
canonical-identity model above: two sibling modules each defining their own
bare `impl Website { fn crawl(...) }` remain irreducibly ambiguous even when
both would otherwise independently qualify.

Every hop's definition lookup and every caller's own call site are resolved
against this capability's *declared* feature set (this harness binary's own
live-compiled features, `PRODUCTION_REACHABLE.feature_requirements`, and an
optional `WIRED.additional_cfg_features` list kept deliberately separate from
`feature_requirements` so it cannot perturb the reachability verdict). A
`#[cfg(...)]`-gated overload only counts as the real definition if its
predicate evaluates `true` under that declared set; mutually exclusive
overloads of the same symbol can no longer be silently combined into one
fictional chain. This cfg evaluation is applied strictly only to
`spider/src/**` — the one crate whose Cargo feature names this process's own
`cfg!` reads reflect. Evidence under `vendor/**` (a separate crate with its
own, unresolved feature namespace) cannot have its cfg predicates evaluated
true/false at all, but this no longer means "accept anything": a candidate
definition with no `#[cfg(...)]` at all is unconditionally trusted (nothing
exclusive about it), while candidates that *are* cfg-gated are only trusted
when every candidate for the same caller agrees on whether it calls the
target. Two mutually exclusive vendor overloads that disagree (one calls the
target, a `#[cfg]`-sibling does not) are refused entirely — neither can
supply adjacency proof. A real, latent bug found and fixed while hardening
this: the vendor-mode agreement check used `Iterator::all` on the candidate
list, which is vacuously `true` on an *empty* list — a caller name matching
zero candidates at some scanned scope previously made the scanner falsely
report a hit and stop, without ever reaching the sibling/child scope that
held the real candidate; this is now an explicit empty-list check returning
`false`. The module-path change above applies uniformly regardless of
`strict`, so a bare name ambiguous across two unrelated modules is caught by
the same existence gate whether the evidence is under `spider/src/**` or
`vendor/**`.

**No macro invocation, under any macro name, ever establishes call
adjacency.** This is a deliberate, complete removal, not a narrower
allowlist: an earlier version of this harness credited a closed allowlist of
macro *names* known to evaluate all their arguments (`tokio::join!`/bare
`join!`) — but a macro name is a syntactic identifier, not a canonical
symbol. A locally defined `macro_rules! join { ($($tokens:tt)*) => {}; }`
shadows the real `tokio::join!` under the exact same bare name, and this
harness has no way to tell, from a single file's `syn::File`, which `join!`
a given `join!(...)` invocation actually resolves to — that is real
macro/name resolution, which this harness does not attempt ("do not build a
rust compiler"). `Calls` (the adjacency visitor shared by
`ast_contains_production_definition`'s caller-body scan and
`ast_function_calls`) has no `visit_macro`/`visit_expr_macro` override at
all; the default `syn::visit` behavior for a macro invocation does not parse
or recurse into its token stream (`TokenStream` is not a typed AST node), so
a call expression sitting inside *any* macro invocation's arguments —
allowlisted name or not — is structurally invisible to this scan and can
never establish a hit. A production chain that depends on macro expansion to
establish real adjacency is `NOT_PROVEN` by this harness — see section 8 for
the real consequence of this to the credential-cache proof case's own WIRED
evidence.

### PRODUCTION_REACHABLE

`entry_point_symbols` must be a subset of this same capability's proven
`WIRED` roots. Generic or independently declared entry points are rejected.

Known shipping artifacts are `spider_cli`, `spider_mcp`, and `spider_worker`.
Their manifests are parsed as TOML. Feature proof comes from actual default
features and dependency feature forwarding, never substring matching. Artifact
sources are parsed with `syn`; production calls must bind to the declared
root through the canonical identity model above — the same primitive, not a
separate one, for self-receiver method calls, named-variable receivers, and
associated/qualified calls alike. `ast_any_production_call` is only ever
invoked against known external shipping-artifact source trees (never
`spider`'s own crate source), so the canonical spelling its "world" accepts
is always `spider::website::Website` — exactly what
`spider_cli`/`spider_mcp`/`spider_worker`'s own real source does via
`use spider::website::Website;`. Test-only calls (`#[test]` functions,
`#[cfg(test)]`/conventionally-named `mod tests`) are excluded regardless of
how genuine their canonical provenance is — `cargo test`-only code is never
part of the shipped binary.

This does not catch every theoretically possible cross-crate shadowing
scheme (e.g. a re-exported alias several `use` hops removed through
intermediate crate-local re-exports, or a variable reassigned through an
intermediate helper function that itself returns `Website` without a visible
`Website::new` call or type annotation at the use site), but it does close
every bypass form demonstrated during review, across five rounds — including
the `mod Website { pub fn crawl() {} }` module-masquerading-as-type
reproducer, which the raw-text associated-call match of every prior round
never independently verified at all.

A `MET` verdict requires at least one declared artifact to both enable one of
`feature_requirements` and call a WIRED-bound entry point. A `NOT_MET` verdict
is checked for staleness: no known artifact may currently satisfy both.
`siblings_enumerated = true`, a `siblings` array, and `siblings_note` are
mandatory. Exhaustiveness of that inventory is explicitly human/process
evidence, not a mechanically proven whole-program fact.

## 4. CI_ENFORCED proof

`CI_ENFORCED` is the workflow-configuration half of CI truth. A separately
supplied `[proof.CI_PROVEN]` record is the execution half. That record names
the exact executed commit, workflow, numeric Actions run ID and URL, job,
step, successful conclusion, capability ID, and stable command identity.
Deterministic verification never queries GitHub and therefore cannot create
CI_PROVEN during implementation. CI proof is supplied only after the commit
has actually run.

Workflows are parsed with `serde_yaml_ng`. Only `run:` steps in workflows
triggered by `push` or `pull_request` apply. Any explicit job- or step-level
`if:` makes the step gated and ineligible. YAML comments and schedule-only
workflows cannot satisfy evidence.

`CI_ENFORCED` carries no free-text shell command at all. The real `cargo
test` invocation it claims is declared as discrete, structurally typed TOML
fields — `package`, `lib`, `test_targets`, `feature_set`,
`positional_filters`, `exact`, `skip` — parsed with no shell semantics
whatsoever. The real workflow side is unavoidably GitHub Actions shell text,
but it is only ever considered after a strict allowlist grammar confirms the
whole `run:` value is an unambiguous direct `cargo test` invocation: after
whitespace normalization the first tokens must be exactly `cargo test`, none
of these shell metacharacters may appear (pipe, ampersand, semicolon,
dollar, backtick, angle brackets, parentheses), and the text must be
shell-unambiguous — no unquoted `#` and no backslash anywhere (a real shell
truncates execution at an unquoted `#`; a text scan that doesn't reject it
could still "see" a flag placed after one that never actually runs). This
filter is applied once, uniformly, at workflow load time, so every consumer
of a parsed `run:` value (self-selection, live-test exclusion, `CI_ENFORCED`
matching) is protected. A qualifying `run:` value is then reduced to the
exact same structural representation the ledger declares (package,
`lib`/`test_targets`, feature token set, positional filters, `--skip`,
`--exact`) and compared field-by-field — order-independent, set-based —
never by string equality. Two commands with identical real cargo-test
meaning but superficially different text now match; a shell trick that
changes real execution while preserving old string-equality (or the
reverse) is categorically impossible, because there is no string comparison
left to fool.

`CI_ENFORCED` is also exactly bound to `VERIFIED`: `lib = true` is required
for every cited library test and the exact binary name must appear in
`test_targets` for every cited integration test. `skip`, non-exact
`positional_filters`, and exact `positional_filters` are each independently
evaluated against every cited test's bare name, so naming the right binary
while excluding the exact evidence — via any of the three mechanisms,
including a plain positional filter that simply doesn't match — fails.

This is not full Cargo/libtest semantic coverage, and is not claimed to be.
The parser recognizes a closed, explicit set of flags (`--lib`, `-p`,
`--test`, `--features`/`-F`, `--skip`, `--exact`) and *rejects the whole
command* — returns `None`, never silently ignores or reinterprets — the
moment it sees anything else. This includes four distinct failure classes:
  - a denylist of flags known to change execution semantics without fitting
    this model at all: `--no-run` (compiles, never executes), `--list`
    (prints names, never executes), `--ignored` (inverts selection to
    `#[ignore]`-only tests), `--no-default-features`/`--all-features`
    (change the active feature set independent of any `--features` flag),
    `--doc` (doctests only), and
    `--tests`/`--bins`/`--benches`/`--examples`/`--bin`/`--example`/`--bench`
    (select a whole target class this model has no way to name);
  - any *recognized* flag with a missing or malformed value — a trailing
    `-p`, `--test`, `--features`/`-F`, or `--skip` with nothing after it is
    rejected rather than silently treated as "flag absent, nothing changes";
  - a *recognized* flag whose consumed "value" token is itself syntactically
    another option (starts with `-`) — `cargo test -p --lib`,
    `--test --lib`, `--features --lib`, `-F --lib`, `-- --skip --exact` are
    all rejected rather than treating the following flag's own token text as
    an ordinary package/target/feature/skip-pattern value. Real Cargo parses
    `--lib` as its own flag in every one of these cases and rejects the
    command for the value-taking option having no value at all; this model
    must reach the same "not a valid selection" verdict, never silently
    construct a `TestSelection` from a command Cargo/libtest itself would
    refuse to run;
  - any unrecognized flag at all, long (`--`) or **short** (`-`) — a single-
    dash token that isn't `-p` or `-F` (both explicitly modeled) is an
    unrecognized short option and is rejected, never silently reinterpreted
    as a positional test-name filter, which an earlier version of the
    tokenizer's permissive catch-all did.

`--features`/`-F` model real Cargo feature semantics, not a single flat
string: repeated occurrences merge into one combined feature set, and each
occurrence's value is split on both commas and whitespace into individual
feature tokens (`--features chrome,cache` and `--features chrome --features
cache` both yield the same two-element set). A rejected command can never
match any declared `CI_ENFORCED` fields — the same fail-closed posture an
unresolvable `cfg` predicate gets.

`[stages.CI_ENFORCED]` also carries a required `ci_workflow_file` field — the
exact repo-relative workflow path (`.github/workflows/rust.yml`) the
qualifying command must be found in. Every parsed workflow step now retains
which file it came from; a structurally matching, non-gated, applicable,
executable command found in *any other* workflow file does not satisfy
`CI_ENFORCED`, only one found in the declared `ci_workflow_file` does. This
closes a real gap: an earlier version scanned every `.yml` file under
`.github/workflows/` and flattened every step into one undifferentiated
list with no file provenance at all, so a matching command sitting in a
workflow this repository's real CI configuration never actually runs would
have satisfied evidence identically to one in the right file.
`ci_workflow_file` is required whenever a `[stages.CI_ENFORCED]` table
exists at all — there is no fallback to a hardcoded path.

The harness itself must be an explicit `--test closure_harness` target in an
applicable, required, *directly executable* CI command (the same grammar
`CI_ENFORCED` itself requires — an `echo`-wrapped or otherwise non-executing
command cannot satisfy self-enforcement). The same requirement additionally
applies to `--test closure_harness_behavioral_contract`,
`--test closure_harness_integrity`, and `--test architecture_guardrails` —
the independent behavioral suite and the integrity sentinel must themselves
be required, non-gated CI, not merely local development checks.

### CLOSED and ADVERSARIALLY_VERIFIED revision binding

`CLOSED.closed_commit` must be a real commit object (never a blob/tree)
reachable from `HEAD`. Reachability alone is insufficient: the claimed
revision must contain byte-identical copies, at that exact commit, of every
file `closure_relevant_files` names for the entry — a fixed, enumerated set,
not a claim of true exhaustiveness over every file that could conceivably
matter. That set is: the ledger entry itself, `LIVE_NETWORK_TESTS.toml`,
`closure_harness.rs`, `closure_harness_behavioral_contract.rs`,
`closure_harness_integrity.rs`, `spider/Cargo.toml`, every known shipping
artifact's `Cargo.toml`, every file named in this entry's own
`IMPLEMENTED`/`VERIFIED` evidence, every file named by any hop (not only the
terminal) in every `WIRED.callers` chain, and — only when
`PRODUCTION_REACHABLE.verdict = "MET"` — the exact shipping-artifact source
file(s) this same harness independently re-derives as calling the
WIRED-bound entry point. A historical ancestor whose ledger and harness
bytes happen to match today's but whose implementation, call-chain, or
shipping-consumer source has since drifted fails this check.

The CI workflow file is bound *dynamically*, not as a fixed path: when
`[stages.CI_ENFORCED]` exists, the set includes exactly the file named by
that table's own `ci_workflow_file` field — the same file
`ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match`
required the qualifying command to live in. An earlier version hardcoded
`.github/workflows/rust.yml` here regardless of what (if anything)
`CI_ENFORCED` actually named, which both bound the wrong file for a
capability whose evidence lived elsewhere and bound *a* file even for
entries with no `CI_ENFORCED` table at all. When no `[stages.CI_ENFORCED]`
table exists — true for any `ADVERSARIALLY_VERIFIED`-only entry, since
`ADVERSARIALLY_VERIFIED` precedes `CI_ENFORCED` in the stage order — no
workflow file is CI-derived evidence for this capability yet, so none is
bound; inventing one this entry's evidence never actually rested on would
itself be a false provenance claim.

`ADVERSARIALLY_VERIFIED.reviewed_commit` is checked identically — object
kind, reachability, capability-ID binding, and the same relevant-file set
as `CLOSED` — via the same shared function, so a stale-but-real ancestor
cannot advance ADVERSARIALLY_VERIFIED maturity either.

**`DESIGNED.sdd` is existence evidence only, deliberately not
revision-bound.** `designed_stage_references_a_real_sdd_file` checks only
that the declared path resolves to a real, committed file
(`workspace_root().join(sdd).is_file()`) — it does not hash or otherwise
byte-bind the document, and `closure_relevant_files` never includes it. This
is a considered decision, not an oversight: this SDD is one shared document
describing the whole harness, iteratively reconciled after nearly every
implementation change across many capabilities and many review rounds
(exactly as this file is being edited in the same commit as the code changes
it describes right now) — it is institutional documentation that is expected
to keep evolving, not a frozen, capability-specific design artifact whose
exact historical text was "the design that was reviewed." Byte-binding it
would mean any later, unrelated SDD clarification (documenting a *different*
capability's fix) silently invalidates every previously-CLOSED entry's
revision binding unless the SDD were re-committed in lockstep with every
future change forever — a burden this model does not claim to carry, and
does not need to: `DESIGNED`'s actual claim is narrow ("a real, committed
design document exists and is named"), and every stage from `IMPLEMENTED`
onward is independently, mechanically re-verified against real code and
evidence regardless of what the SDD's current text says. If a future
capability's ledger entry ever needs to claim "CLOSED reflects the exact
reviewed design revision, not merely that some design document exists," that
would require a different, additional mechanism (e.g. a per-capability
design-document hash or excerpt bound at `DESIGNED` time) — no such
mechanism exists today, and no ledger entry should be read as implicitly
claiming one.

## 5. Independent behavioral contract and mutation proof

`closure_harness_behavioral_contract.rs` is a separate test binary. It creates
isolated temporary ledger/workflow directories and invokes the real verifier
in a subprocess. It does not copy verifier predicates.

38 of its 39 `#[test]` functions call `run_single_verifier_check_strict`,
which appends `--exact <inner-test-name>` to the subprocess invocation,
asserts the subprocess output confirms exactly one inner test ran (guarding
against a typo'd name silently matching zero tests, which would trivially
"pass"), and reports that one test's own pass/fail status. `--exact`
filtering structurally excludes every other `#[test]` in `closure_harness`
— including `structural_parser_rejects_known_adversarial_fixtures`, an
internal self-test sharing several of the same AST/TOML helpers — from
running at all, so a failure can only mean the one named rule specifically
accepted or rejected the fixture; no unrelated predicate can mask the
result. This is a load-bearing claim only because it is what the shipped
`#[test]` functions actually invoke — an earlier version of several of
these fixtures called `run_real_verifier_against` (the whole `closure_harness`
binary) instead, which still correctly rejected each fixture but did not,
by itself, prove which rule was responsible.

The 39th, `behavioral_verifier_accepts_a_genuinely_valid_fixture`, is
deliberately whole-verifier: a positive control asserting every rule
simultaneously accepts a genuinely valid case has no single rule to isolate
to, and is documented as such rather than called isolated mutation proof.

Each rejection fixture was constructed against a specific, isolated verifier
rule and confirmed, by hand, to flip from reject to accept when that rule
alone was mutated, then reject again once reverted:

| Rule isolated | Mutation | Result |
|---|---|---|
| Git object kind | Cargo.toml blob SHA as closure commit | reject |
| Root binding | generic, WIRED-unbound entry point | reject |
| Call adjacency | real symbols without a caller-to-callee call | reject |
| Positive control | genuinely valid WIRED-only fixture (whole-verifier, not rule-isolated) | accept |
| Stage prerequisites | CLOSED without predecessor tables | reject |
| Production definition | IMPLEMENTED points to test-only evidence | reject |
| Parsed feature proof | artifact claims a fabricated feature | reject |
| Direct grammar | CI command wrapped with `echo` | reject |
| Required execution | matching step behind `if:` | reject |
| Applicable trigger | matching step in schedule-only workflow | reject |
| Reviewed history | unrelated historical ancestor as CLOSED proof | reject |
| VERIFIED selection | correct target but `--skip` cited test | reject |
| VERIFIED selection (non-exact positional) | correct target but a non-matching positional filter | reject |
| Reviewed history (ADVERSARIALLY_VERIFIED) | unrelated historical ancestor as review proof | reject |
| WIRED terminal identity | terminal binds to IMPLEMENTED evidence in an unrelated file via bare-name collision | reject |
| Type::method impl ownership | WIRED root satisfied by an unrelated type's same-named method | reject |
| Cfg-active adjacency | inactive cfg-gated overload's body used as call evidence | reject |
| Test-only intermediate caller | `#[cfg(test)]`-only caller used as WIRED adjacency proof | reject |
| VERIFIED module identity | evidence resolved via bare-test-name collision across modules | reject |
| WIRED module identity (terminal) | WIRED terminal's bare name ambiguous across two unrelated modules | reject |
| Canonical Website ownership (local) | unrelated decoy `Website` struct in a real artifact's `src/` credited as reachability | reject |
| Canonical Website ownership (imported) | `use`-imported unrelated `Website` in a real artifact's `src/` credited as reachability | reject |
| Test-only production consumer | `#[test]`-only consumer in a real artifact's `src/` credited as reachability | reject |
| No-run CI evidence | `--no-run` (compile-only) step credited as executing evidence | reject |
| List-only CI evidence | `-- --list` step credited as executing evidence | reject |
| Ignored-only CI evidence | `-- --ignored` step credited as executing evidence | reject |
| Unmodeled feature-mode CI evidence | `--all-features` step credited despite unrepresented feature-set effect | reject |
| CI workflow provenance | matching command exists only in a workflow file other than the declared `ci_workflow_file` | reject |
| CI workflow provenance (positive control) | matching command genuinely in the declared `ci_workflow_file`, decoy in another file | accept |
| Canonical Website provenance (nested + qualified) | `Website` nested in an unrelated module, referenced only via an inline qualified path with no `use` import | reject |
| Macro adjacency allowlist | `stringify!(target())` (never evaluates its argument) credited as WIRED call adjacency | reject |
| Value-taking option validation | `--test --lib` (`--test` consuming `--lib` as its value) credited as a valid CI selection | reject |
| Shared impl-ownership identity (WIRED) | `impl decoy::Website { fn crawl(&mut self) { self.fake_next(); } }` (decoy nested in a crate-local module, qualified self_ty) credited as a genuine WIRED root/adjacency | reject |
| Shared impl-ownership identity (PRODUCTION_REACHABLE) | `impl crate::decoy::Website { fn helper(&mut self) { self.crawl(); } }` (no local shadow, no `use` import) credited as production reachability | reject |
| Canonical identity (WIRED, bare local shadow) | same-file `struct Website; impl Website { ... }` with no canonical import credited as a WIRED root | reject |
| Canonical identity (WIRED, crate-local decoy import) | `use crate::decoy::Website;` (crate-local, not canonical) credited as a WIRED root | reject |
| Canonical identity (PRODUCTION_REACHABLE, module masquerading as type) | `mod Website { pub fn crawl() {} }` + `Website::crawl()` credited as production reachability | reject |
| Macro adjacency (locally-shadowed name) | a local `macro_rules! join` shadowing the real `tokio::join!` credited as WIRED call adjacency | reject |
| Canonical identity positive control (external world) | genuine `use spider::website::Website;` in a real artifact accepted as production reachability | accept |

Four of these — `Type::method` impl ownership, cfg-active adjacency,
test-only intermediate caller exclusion (the WIRED-chain rules), each via
`wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source`
— required mutating *two* independent functions simultaneously
(`ast_contains_production_definition`'s existence check and
`ast_function_calls`'s adjacency check each separately enforce the same
guarantee, a defense-in-depth choice made earlier in this frontier);
mutating either one alone left the other still rejecting the fixture, so a
single-function mutation could not demonstrate the flip. The canonical-
Website-ownership and test-only-production-consumer fixtures use a real
shipping artifact (`spider_worker`) with a throwaway, `Drop`-cleaned
scratch source file, rather than a synthetic in-memory string, since
`PRODUCTION_REACHABLE` evidence is scanned from real artifact `src/` trees
on disk.

The verifier additionally carries structural adversarial fixtures for AST
receiver binding, unrelated types/modules/newtypes, comment/string evidence,
feature forwarding, direct-command grammar, cfg-gate resolution
(spider/src-strict and vendor-permissive-with-ambiguity-check), module-path
test identity, canonical-Website file-trust, and test selection. Those run
in-process inside `closure_harness.rs` itself (not subprocess-isolated) and
are defense in depth; the subprocess suite in
`closure_harness_behavioral_contract.rs` is the independent behavioral
contract.

## 6. Integrity sentinel

`closure_harness_integrity.rs` checks a reviewed SHA-256 digest and required
semantic markers in `closure_harness.rs`. It detects accidental drift and
forces an explicit review/update when the verifier changes. It is not tamper
resistance, a security boundary, an independent oracle, or protection against
a coordinated verifier-and-digest edit.

## 7. LIVE_NETWORK_TESTS and deterministic CI

The harness statically re-derives live-network unit tests and bidirectionally
compares them with `LIVE_NETWORK_TESTS.toml`. Detection recognizes test
functions, network-producing calls/URLs, externally routable and
dynamic/unresolved hosts, and local listener fixtures. Local, loopback, and
reserved hosts are not live external network. New unclassified detections and
stale registry entries both fail.

Every required, non-gated `cargo test -p spider --lib` execution must exclude
every registered live test it could select, using the same parsed positional,
`--skip`, and `--exact` model. `--no-run` steps execute nothing and are not
checked as execution steps. Live coverage remains in the separately classified
and `RUN_LIVE_TESTS`-gated `spider_core_live_network` job. When disabled, the
workflow prints `LIVE TESTS = NOT RUN`; that state creates no CI, operator, or
live observation. When enabled, each of the 13 registered tests is invoked by
a separate exact Cargo/libtest command so one malformed multi-filter command
cannot silently replace the suite. Infrastructure failures remain fatal.

The required `spider_core` job prebuilds while the network is open. Before test
execution it flushes IPv4 and IPv6 OUTPUT chains, permits only `lo`,
`127.0.0.0/8`, and `::1/128`, sets both OUTPUT policies to DROP, permits no
DNS/external egress, and clears proxy variables. It then proves IPv4 and IPv6
loopback with local HTTP servers and proves direct IPv4 and IPv6 external
probes fail. Every firewall command/probe is fail-closed.

The deterministic phase runs default-feature unit tests, the
`chrome cache cache_request` unit suite, the closure/architecture targets, and
one exact `chrome_remote_cache` credential-cache regression.

The separate required `current_product` job covers deterministic
`spider_agent` OpenAI-compatible/SearXNG behavior, `spider_agent_html`, durable
acquisition, ResearchSession/DurableResearchResult, canonical identity,
no-default and research-only CLI builds, default shipping CLI tests, and the
shipping-binary RUN/SHOW fixture. These are CODE/CI tests only. They do not
claim real SearXNG, network, model, or synthesis acceptance.

The firewall rules and probes are CODE_PROVEN configuration until a real
GitHub-hosted run succeeds. Only that run can supply CI_PROVEN for isolation.

## 8. Credential-cache proof case

`SCORPION_CANONICAL_CREDENTIAL_CACHE_ISOLATION_001` truthfully has:

- `DESIGNED`, `IMPLEMENTED`, `VERIFIED`, `WIRED`: `MET`
- `PRODUCTION_REACHABLE`: `NOT_MET`
- later stages: withheld
- top-level stage: `WIRED`

Only the canonical cache_request.rs chain (`Website::crawl ->
sitemap_crawl_chain_raw -> sitemap_crawl_raw -> sitemap_parse_crawl ->
new_page_for_mode -> fetch_page_html_with_cache_executor`) is declared as a
`WIRED.callers` entry now — AST-proven from `Website::crawl` through real
adjacency to this capability's `IMPLEMENTED` terminal, with no macro-based
hop anywhere in it. The legacy/vendor-chromey chain, declared in earlier
rounds, was **removed as a WIRED claim in round 5**: its one adjacency hop
(`set_document_content_if_requested_cached` calling
`vendor/chromey/src/page.rs:spawn_cache_listener`) only exists inside a real
`tokio::join!(page.spawn_cache_listener(...), ...)` macro invocation's
arguments in `spider/src/utils/mod.rs`, and this harness no longer credits
any macro invocation as call adjacency at all (section 3). No product wiring
was touched to make this change — the real implementation code is unchanged;
only the ledger's own claim about what this harness can *mechanically prove*
was corrected. The underlying fix (vendor/chromey's
`create_cache_key_raw`/`contains_disqualifying_secret_header`) remains
truthfully claimed under `[stages.IMPLEMENTED]`, which only claims the fix
genuinely exists in the codebase — never that this harness can prove its
reachability through a macro-expanded hop. Re-establishing that reachability
claim would require either a non-macro call path (if one exists) or a
future, narrowly-scoped, explicitly-verified macro-resolution mechanism this
harness does not have today — not a reintroduction of the bare-macro-name
allowlist this round closed.

No known artifact satisfies both `PRODUCTION_REACHABLE` conditions:
`spider_cli` and `spider_mcp` call the root but do not enable the required
cache features; `spider_worker` can enable a cache feature but its handlers do
not call the WIRED-bound root. Compilation alone is insufficient.

## 9. External and unverified assumptions

- Sibling enumeration is reviewer evidence; global exhaustiveness is not
  mechanically proven.
- Static Rust analysis is repository-specific, not a compiler whole-program
  call graph. Unsupported forms fail closed and require harness evolution.
- Live-network classification is conservative; dynamic/unresolved hosts are
  external, while novel indirection may require detector evolution.
- The dual-stack firewall could not run in this development sandbox because it
  lacks root/CAP_NET_ADMIN. Its first GitHub-hosted run is the external proof
  of passwordless sudo, iptables/ip6tables, IPv6 loopback, and runner behavior;
  the probes make mismatch fail closed. The IPv6 external-egress probe's URL
  was corrected from an unbracketed `https://2606:4700:4700::1111/` (which
  `curl` cannot even parse as a valid URL — colons after the scheme are
  ambiguous with a port separator — meaning the probe would "fail," and the
  `must be denied` check would trivially "pass," for a URL-parsing reason
  having nothing to do with the firewall) to the correctly bracketed
  `https://[2606:4700:4700::1111]/`. This was never observed running for
  real either before or after the fix; it remains unverified until a real
  GitHub Actions run is watched.
- Proxy-variable clearing is now applied at the `spider_core` job level (an
  `env:` block on the job itself, inherited by every step's process), not
  only inside the firewall/probe step's own shell via `unset`. Each GitHub
  Actions `run:` step is a separate process; a shell `unset` performed in one
  step does not persist into any later step, so the firewall lockdown alone
  never actually guaranteed that the deterministic/assurance test steps that
  ran *after* it were free of an inherited proxy variable capable of routing
  an "external" request back through an allowed loopback address. The
  firewall step's own `unset` is kept in addition, as a second guarantee for
  that step's own probes specifically. This closes the gap on paper; like the
  firewall lockdown itself, it has not been observed running for real and
  remains unverified until a real GitHub Actions run is watched.
- The credential-cache case cannot claim `CI_ENFORCED` while
  `PRODUCTION_REACHABLE` remains `NOT_MET`, regardless of local test results.
- `CI_ENFORCED.ci_command` has been replaced with a structurally typed
  ledger representation (`package`, `lib`, `test_targets`, `feature_set`,
  `positional_filters`, `exact`, `skip`) that carries no shell semantics —
  the ledger side of this bypass class is closed. The real workflow `run:`
  side remains, unavoidably, GitHub Actions shell text (this is not
  something the harness controls), but it is only ever accepted after the
  same strict allowlist grammar (`executable_test_command` /
  `shell_text_is_unambiguous`) confirms it, then reduced to the identical
  structural representation and compared field-by-field, never by string
  equality.
- A per-rule independent-subprocess mutation-proof, each backed by a
  permanent regression test using the actual `--exact` single-rule
  isolation mechanism (`run_single_verifier_check_strict`, not the
  whole-verifier `run_real_verifier_against`), covers 38 maturity-critical
  rules — see section 5's table for the complete list. The 39th shipped
  behavioral test is a deliberate whole-verifier positive control,
  documented as such rather than called isolated mutation proof.
- **Canonical `Website` identity was rebuilt in round 5 from an
  affirmative-provenance model, replacing four rounds of accreted
  shape/name-inference heuristics that each closed one demonstrated bypass
  while leaving the same underlying weakness — "does this look right" —
  in place for the next one.** Rounds 2-4 progressively closed: a
  locally-shadowed `struct Website`/`enum Website`/`type Website = ...` at
  any nesting depth; an unrelated `use`-imported/re-exported `Website` not
  provably crate-local at its first segment; any glob import; an inline
  qualified path in a type annotation/constructor call; a qualified `impl`
  block self_ty. Each fix was real and each closed the reproducer
  demonstrated against it — but each was still fundamentally "reject a
  specific bad shape," so round 5 demonstrated three further reproducers
  no prior shape-rejection heuristic covered: (1) a *same-file* bare
  `struct Website; impl Website { ... }` with no `use` import at all
  (nothing to flag as "bad," since there was no import, and a same-file
  local definition was never itself treated as suspicious); (2)
  `use crate::decoy::Website;` — provably crate-local at its first
  segment, exactly like the real import, so the "crate-local first
  segment" rule from round 3 could not distinguish it from
  `crate::website::Website`; (3) `mod Website { pub fn crawl() {} }`
  followed by `Website::crawl()` — a *module*, not a struct, matched on
  raw path-text equality alone, since `syn` cannot tell a module path from
  a type path at a reference site. Round 5 replaces the entire model:
  canonical identity is proven affirmatively from a closed, finite set of
  known-good forms (`file_proves_bare_website`,
  `canonical_type_owner_name`) — the real definition site, or a `use`
  import/inline path whose *complete* text exactly equals
  `crate::website::Website` (inside the `spider` crate) or
  `spider::website::Website` (from a known external artifact crate) — or
  it is `NOT_PROVEN`. This is a single shared primitive consumed
  identically by `ast_contains_production_definition`'s owner tracking,
  `ast_function_calls`'s strict and vendor-permissive receiver/
  associated-call resolution, and `ast_any_production_call`'s
  self-receiver/named-variable/associated-call binding — there is no
  longer a separate "file-trust" pre-check plus a separate "binding
  provenance" check plus a separate "impl-ownership" check with three
  different strengths; there is one affirmative-proof primitive, consumed
  everywhere. This is still not full cross-crate type resolution (`syn`
  has no type checker, and this harness does not attempt `self::`/
  `super::`-relative resolution at all — no real code in this repository
  uses either to reach `Website`, grep-confirmed): a re-exported alias
  several `use` hops removed through intermediate crate-local re-exports,
  or a variable reassigned through an intermediate helper function that
  itself returns `Website` without a visible `Website::new` call or type
  annotation at the use site, would not be caught, and would correctly
  return `NOT_PROVEN` rather than being guessed canonical.
- `Cargo`/libtest argument parsing (`parse_test_selection`) now fails
  closed on malformed values for every recognized flag (a trailing
  `-p`/`--test`/`--features`/`-F`/`--skip` with nothing after it), on a
  recognized flag's value token itself syntactically being another option
  (`-p --lib`, `--test --lib`, `--features --lib`, `-F --lib`,
  `-- --skip --exact` are all rejected rather than treating the following
  flag's own text as an ordinary value — `looks_like_an_option`), models
  real Cargo feature semantics (repeated `--features` merge; comma- and
  whitespace-separated values both split into individual tokens; `-F` is
  modeled as an exact alias, not left to fall through to the
  positional-filter catch-all), and rejects every unrecognized short
  option (`-x`, `-Z...`) rather than silently reinterpreting it as a
  test-name filter — the same fail-closed posture already applied to
  unrecognized long options.
- **No macro invocation, under any macro name, is credited as call
  adjacency any more — this is a complete removal, not a narrower
  allowlist.** An earlier round's fix replaced raw-token-substring
  matching with a closed allowlist of macro *names* known to evaluate all
  their arguments (bare `join!`/`tokio::join!`, the one real production
  case, `tokio::join!(page.spawn_cache_listener(...), ...)`). Round 5
  found this still name/shape inference: a locally defined
  `macro_rules! join { ($($tokens:tt)*) => {}; }` shadows the real
  `tokio::join!` under the identical bare name, and this harness has no
  way to tell, from a single file's `syn::File`, which `join!` a given
  invocation actually resolves to. The `Calls` visitor (shared by
  `ast_contains_production_definition`'s caller-body scan and
  `ast_function_calls`) now has no `visit_macro`/`visit_expr_macro`
  override at all; the default `syn::visit` behavior does not parse a
  macro's token stream (`TokenStream` is not a typed AST node), so a call
  sitting inside *any* macro's arguments — allowlisted name or not — is
  structurally invisible and never establishes a hit. The real
  consequence: `SCORPION_CANONICAL_CREDENTIAL_CACHE_ISOLATION_001`'s
  legacy/vendor-chromey WIRED chain, whose one real adjacency hop only
  exists inside a `tokio::join!(...)` invocation, was removed from that
  ledger entry's `WIRED.callers` in this same round (no product wiring
  changed — only the ledger's claim about what this harness can
  mechanically prove); the canonical cache_request.rs chain, which never
  routes through a macro, remains and alone keeps `WIRED` truthfully
  `MET`. See section 8.
- CI workflow provenance is now tracked per parsed step
  (`WorkflowStep::file`, `.github/workflows/<filename>`) and required to
  exactly equal `[stages.CI_ENFORCED].ci_workflow_file` before a matching
  command counts as evidence, and the same declared file (not a hardcoded
  path) is what `CLOSED`/`ADVERSARIALLY_VERIFIED` revision-binding
  includes. This closes a real gap where every workflow file under
  `.github/workflows/` was scanned and flattened together with no file
  provenance retained at all — a matching command in the *wrong* workflow
  file previously satisfied `CI_ENFORCED` identically to one in the file
  this repository's real CI configuration actually runs.
- Definition-owner tracking (`DefinitionOwner`) now carries the complete
  enclosing module path, not merely "free function" vs. "impl method" — a
  bare symbol independently defined in two different modules of the same
  file (`real::hop` vs. an unrelated `unrelated::hop`) is now detected as
  ambiguous and fails closed, where it previously collapsed into a single
  untracked "free function" key and silently resolved to whichever
  definition the AST walk visited first. The WIRED chain's terminal hop —
  previously checked only as an adjacency callee, never independently for
  its own existence — now goes through the same module-aware existence
  gate as every intermediate hop. A related, more severe latent bug found
  and fixed in the same pass: the vendor-mode cfg-agreement check used
  `Iterator::all` on an empty candidate list, which is vacuously `true` in
  Rust — a caller name matching zero candidates at some scanned scope
  previously made the scanner falsely report a hit and return immediately,
  without ever reaching the sibling/child scope holding the real
  candidate.
- Cfg-gated overload resolution for `WIRED`/`IMPLEMENTED` evidence is
  strict (fail-closed, must-prove-active) only for `spider/src/**`, whose
  Cargo feature names this harness binary's own `cfg!` reads genuinely
  reflect. Vendored/third-party source (`vendor/**`) has its own,
  unresolved feature namespace this single-crate mechanism cannot
  evaluate true/false for individual predicates (a real case found while
  hardening this: `vendor/chromey`'s internal `_cache` feature) — but
  cfg gates there are no longer decorative. A vendor candidate with no
  `#[cfg(...)]` at all is trusted unconditionally; when every candidate
  for a caller is cfg-gated, the call-adjacency answer is only trusted
  when every candidate agrees, and genuine disagreement (mutually
  exclusive overloads that answer differently) is refused rather than
  OR'd into a fictional chain.
- This same cfg-aware adjacency check, applied for the first time this
  round, found and corrected three real, previously-undetected stale hops
  in the credential-cache proof case's own `WIRED` chains — a missing
  `fetch_page_html_base` hop, and a mutually-exclusive-cfg-overload
  mismatch that had the ledger citing `sitemap_crawl_chain` (whose
  chrome-active overload actually calls `sitemap_crawl_chrome`, not
  `sitemap_crawl_raw`) instead of the real, always-compiled
  `sitemap_crawl_chain_raw`. The chains as corrected are re-verified
  end-to-end by `closure_harness.rs` itself; see the ledger file's own
  comments for the exact line numbers and cfg predicates involved.

## 10. Changed files

- `.github/workflows/rust.yml`
- `Cargo.lock`
- `spider/Cargo.toml`
- `docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md`
- `docs/frontier/ledger/TEMPLATE.toml`
- `docs/frontier/ledger/LIVE_NETWORK_TESTS.toml`
- `docs/frontier/ledger/SCORPION_CANONICAL_CREDENTIAL_CACHE_ISOLATION_001.toml`
- `spider/tests/closure_harness.rs`
- `spider/tests/closure_harness_behavioral_contract.rs`
- `spider/tests/closure_harness_integrity.rs`

## 11. Verification record

Exact final local commands/counts are reported in the independent-review
package. The IPv4/IPv6 lockdown remains externally unverified until its
required GitHub Actions job runs.
