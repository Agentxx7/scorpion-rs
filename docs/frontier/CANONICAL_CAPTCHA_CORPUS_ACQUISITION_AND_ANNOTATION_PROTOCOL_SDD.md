# Canonical CAPTCHA Corpus Acquisition and Annotation Protocol

Frontier: `SCORPION_CANONICAL_CAPTCHA_CORPUS_ACQUISITION_AND_ANNOTATION_PROTOCOL_001`

Baseline: `fecdfa43d42494533f2430617d847a7058122bb0`

## Purpose

This protocol separates lawful data governance from provider evaluation. It
does not acquire CAPTCHA material, appoint annotators, run models, or declare a
corpus representative. It defines the only record shape that may later become
qualification evidence.

Each challenge family freezes independently. A corpus is not ready merely
because another family has reached 200 cases.

## Acquisition

Before admission, an operator retains a pinned source revision, acquisition
authorization, rights record and SHA-256 provenance record. Raw assets are
preserved with original dimensions, byte size, relative path and SHA-256.
Mutable URLs are discovery metadata outside corpus identity and cannot replace
preserved bytes. Model-generated labels and model-selected samples are not
accepted as independent ground truth.

## Annotation

Every case has at least two distinct pseudonymous annotators. Their immutable
raw records are hashed separately and completed without access to one another.
A third, distinct adjudicator records agreement or disagreement, method, final
truth and adjudication digest. Disagreement cannot be erased by presenting only
the final label.

Image grids bind instruction, row-major stable choice IDs, layout, original
dimensions, selected IDs and whether empty selection is valid. Horizontal
offset binds initial piece position when applicable, displacement and tolerance
in original pixels plus method. Point selection binds instruction, original
coordinates, a point and optional accepted rectangle, tolerance and method.

## Splits and sealing

Every case belongs to exactly one non-empty development or qualification split.
Development material may inform prompt/output grammar. The qualification split
remains sealed until evaluation configuration, grammar and the already locked
threshold policy are frozen. Split changes produce a different corpus digest.

## Freeze and identity

`CaptchaCorpusDraft` is preparation-only. `freeze` consumes it after validating:

- at least 200 challenge-level cases;
- complete, unique and referenced assets;
- kind-correct original-coordinate labels;
- two or more independent annotators and distinct adjudicator;
- truthful disagreement record;
- total, disjoint and non-empty splits;
- source rights, annotation completion, test sealing and threshold attestations.

Only `FrozenCaptchaCorpus` may enter empirical qualification. Its SHA-256 binds
corpus ID/version/kind, source provenance, all assets, all raw annotations,
adjudications, split assignments and freeze attestations. Frozen fields are
private and exposed read-only.

## Synthetic data

Synthetic cases may test parsers, malformed outputs, bounds and coordinate
transforms. They do not establish representative provider capability unless a
separate independently reviewed corpus decision explicitly qualifies them.

## Locked thresholds

The threshold identity refers to the policy already fixed by
`SCORPION_QWEN3_VL_CAPTCHA_EMPIRICAL_QUALIFICATION_001`. This frontier provides
no API for changing it and observes no model output.

## Readiness

The acquisition/annotation protocol is ready for ImageGridSelection,
HorizontalOffset and PointSelection. This does not mean a corpus exists. A
family resumes empirical qualification only after an authorized operator has
produced and frozen at least 200 valid cases through this contract.
