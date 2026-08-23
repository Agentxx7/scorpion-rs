# Canonical CAPTCHA Image Grid Input SDD

Frontier: `SCORPION_CANONICAL_CAPTCHA_IMAGE_GRID_INPUT_001`

Baseline: `abcaf54b3bc021789a3f67cd624b9b8a495f1211`

## Model

`CaptchaVisualInput::MaterializedFullGrid` is a distinct canonical form. It
contains one validated `CaptchaImageGridInput`, which in turn owns one ordinary
already-materialized visual plus original dimensions, rows, columns, explicit
cell records and empty-selection semantics. Existing vectors of ordinary
`Materialized` visuals retain their existing multi-visual meaning.

Each `CaptchaImageGridCell` binds a stable caller-assigned choice ID to one
explicit row, column and rectangle in original full-grid image coordinates.
Construction canonicalizes records into row-major order using the explicit
row/column fields; it never derives identity from vector position, filenames,
image count, prompt text or provider behavior.

## Validation

The constructor fails closed for zero dimensions/layout, non-materialized
visuals, overflow, missing/extra cells, empty/duplicate IDs, duplicate or
out-of-range positions, zero-area/out-of-bounds rectangles and any positive
area overlap. Touching rectangle boundaries are non-overlapping.

The validated type has private fields and read-only accessors, so the public
enum variant cannot carry an unvalidated grid. Canonical dispatch permits the
full-grid form only as the sole visual of `ImageGridSelection`.

## Compatibility and serialization

No existing `CaptchaChallenge` field or ordinary `CaptchaVisualInput` variant
changes meaning, so existing public struct construction remains source
compatible. CAPTCHA vocabulary currently has no serde contract; consequently
there is no pre-existing serialization identity to migrate or alias.

Provider-local compositing, resizing, inferred layout, invented geometry and
ID reordering remain forbidden. Provider/runtime work, empirical
qualification, corpus work and routing are outside this frontier.
