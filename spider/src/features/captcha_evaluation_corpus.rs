//! Governance and immutable identity for CAPTCHA evaluation corpora.
//!
//! This module owns no acquisition, annotation service, provider execution or
//! model evaluation. It validates records produced by authorized callers and
//! permits qualification only through a frozen [`FrozenCaptchaCorpus`].

use crate::features::captcha::CaptchaChallengeKind;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

const MINIMUM_CASES: usize = 200;

/// Stable, provider-independent corpus identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CorpusId(pub String);

/// One immutable corpus version. Mutable labels such as `latest` are invalid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorpusVersion(pub String);

/// Pinned provenance and rights for the raw source material.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorpusSourceProvenance {
    /// Organization or person authorized to supply the material.
    pub source_authority: String,
    /// Immutable source revision, release, or archival identity.
    pub source_revision: String,
    /// Human-reviewable acquisition authorization record identity.
    pub acquisition_authorization: String,
    /// License or permission record allowing retention and evaluation.
    pub rights_record: String,
    /// SHA-256 of the retained source-provenance record.
    pub provenance_sha256: String,
}

/// One preserved raw asset. A URL is deliberately not corpus identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorpusAsset {
    /// Corpus-relative immutable path.
    pub path: String,
    /// Exact byte size.
    pub size_bytes: u64,
    /// SHA-256 of the preserved bytes.
    pub sha256: String,
    /// Original width before any evaluation preprocessing.
    pub original_width: u32,
    /// Original height before any evaluation preprocessing.
    pub original_height: u32,
}

/// Stable pseudonymous annotator identity. It must not identify a provider or
/// model and must differ across the two independent annotations for a case.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AnnotatorId(pub String);

/// Ground-truth value, separated by challenge kind.
#[derive(Clone, Debug, PartialEq)]
pub enum CaptchaGroundTruth {
    /// Stable choice IDs selected by an annotator. Empty is a valid answer
    /// only when the case explicitly permits it.
    ImageGrid {
        /// Selected IDs, in canonical choice order.
        selected_choice_ids: Vec<String>,
    },
    /// Required drag displacement in original-image pixels.
    HorizontalOffset {
        /// Initial piece position when the challenge exposes one.
        initial_piece_x: Option<f64>,
        /// Correct horizontal displacement in original coordinates.
        displacement_px: f64,
        /// Accepted absolute error in pixels.
        tolerance_px: f64,
        /// Human-reviewable annotation method identity.
        annotation_method: String,
    },
    /// Point or rectangular accepted region in original coordinates.
    PointSelection {
        /// Adjudicated target point.
        x: f64,
        /// Adjudicated target point.
        y: f64,
        /// Optional inclusive accepted rectangle `[x0,y0,x1,y1]`.
        accepted_region: Option<[f64; 4]>,
        /// Accepted radial error when no rectangle is supplied.
        tolerance_px: f64,
        /// Human-reviewable annotation method identity.
        annotation_method: String,
    },
}

/// One annotation completed without access to the other annotator's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct IndependentAnnotation {
    /// Independent annotator.
    pub annotator: AnnotatorId,
    /// Ground truth proposed before adjudication.
    pub ground_truth: CaptchaGroundTruth,
    /// SHA-256 of the immutable raw annotation record.
    pub annotation_sha256: String,
}

/// Recorded adjudication after both independent annotations were frozen.
#[derive(Clone, Debug, PartialEq)]
pub struct CorpusAdjudication {
    /// Whether the independent labels disagreed. Agreement may still be
    /// reviewed, but disagreement must never be silently erased.
    pub disagreement_recorded: bool,
    /// Final frozen ground truth.
    pub final_ground_truth: CaptchaGroundTruth,
    /// Human-reviewable adjudicator identity, distinct from both annotators.
    pub adjudicator: AnnotatorId,
    /// Immutable method or policy used for adjudication.
    pub method: String,
    /// SHA-256 of the adjudication record.
    pub adjudication_sha256: String,
}

/// Challenge layout for stable image-grid choice identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageGridLayout {
    /// Number of rows.
    pub rows: u16,
    /// Number of columns.
    pub columns: u16,
    /// Stable IDs in row-major order.
    pub choice_ids: Vec<String>,
    /// Whether an empty selection is a valid possible answer.
    pub empty_selection_valid: bool,
}

/// One challenge-level corpus case.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptchaCorpusCase {
    /// Stable case identity within the corpus.
    pub case_id: String,
    /// Corpus-relative path to the preserved challenge image.
    pub asset_path: String,
    /// Semantic instruction shown for this challenge.
    pub instruction: String,
    /// Required only for image-grid challenges.
    pub image_grid: Option<ImageGridLayout>,
    /// Exactly two or more independently produced annotations.
    pub independent_annotations: Vec<IndependentAnnotation>,
    /// Frozen adjudication of the independent records.
    pub adjudication: CorpusAdjudication,
}

/// Immutable split assignment. Qualification cases are kept sealed until the
/// evaluation configuration and prompt/output grammar identities are frozen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorpusSplit {
    /// May be used for prompt and grammar development.
    Development,
    /// Unseen qualification/test material.
    Qualification,
}

/// Assignment of one case to exactly one split.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorpusSplitAssignment {
    /// Existing case identity.
    pub case_id: String,
    /// Frozen split.
    pub split: CorpusSplit,
}

/// Governance attestations required before freezing. These values refer to
/// separately retained review records; they are not inferred from URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorpusFreezeAttestation {
    /// Rights and authorization were reviewed before assets were admitted.
    pub rights_review_record: String,
    /// Independent annotation and adjudication completed before evaluation.
    pub annotation_completion_record: String,
    /// Qualification split remained unseen while prompt/grammar were built.
    pub qualification_seal_record: String,
    /// Previously locked thresholds cannot be changed by this corpus.
    pub threshold_policy_id: String,
    /// SHA-256 of the complete attestation record.
    pub attestation_sha256: String,
}

/// Mutable preparation record. It cannot be consumed by provider evaluation.
#[derive(Clone, Debug)]
pub struct CaptchaCorpusDraft {
    /// Stable identity.
    pub corpus_id: CorpusId,
    /// Immutable version selected before qualification.
    pub version: CorpusVersion,
    /// Exactly one independently qualified challenge family.
    pub challenge_kind: CaptchaChallengeKind,
    /// Source authorization and rights.
    pub source: CorpusSourceProvenance,
    /// Complete raw asset manifest.
    pub assets: Vec<CorpusAsset>,
    /// Complete challenge and annotation manifest.
    pub cases: Vec<CaptchaCorpusCase>,
    /// Complete split manifest.
    pub splits: Vec<CorpusSplitAssignment>,
}

/// Fully validated, immutable corpus identity suitable for provider
/// qualification. Fields are private so callers cannot relabel after freeze.
#[derive(Clone, Debug)]
pub struct FrozenCaptchaCorpus {
    draft: CaptchaCorpusDraft,
    attestation: CorpusFreezeAttestation,
    corpus_sha256: String,
}

impl FrozenCaptchaCorpus {
    /// Stable corpus identity.
    pub fn corpus_id(&self) -> &CorpusId {
        &self.draft.corpus_id
    }

    /// Exact frozen version.
    pub fn version(&self) -> &CorpusVersion {
        &self.draft.version
    }

    /// Independently qualified challenge family.
    pub fn challenge_kind(&self) -> CaptchaChallengeKind {
        self.draft.challenge_kind
    }

    /// Complete SHA-256 identity over source, assets, raw annotations,
    /// adjudications, splits and freeze attestations.
    pub fn corpus_sha256(&self) -> &str {
        &self.corpus_sha256
    }

    /// Read-only cases. Provider code cannot mutate labels or split placement.
    pub fn cases(&self) -> &[CaptchaCorpusCase] {
        &self.draft.cases
    }

    /// Read-only split assignments.
    pub fn splits(&self) -> &[CorpusSplitAssignment] {
        &self.draft.splits
    }

    /// Review record proving the qualification split remained sealed.
    pub fn qualification_seal_record(&self) -> &str {
        &self.attestation.qualification_seal_record
    }
}

/// Why a draft cannot become qualification evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorpusFreezeFailure {
    /// Identity, rights, or attestation metadata is missing or mutable.
    InvalidGovernance,
    /// Fewer than 200 challenge-level cases were supplied.
    InsufficientCases,
    /// Asset manifest is missing, duplicated, incomplete, or malformed.
    InvalidAssetManifest,
    /// A case does not match its challenge family or asset.
    InvalidCase,
    /// Independent annotation or adjudication requirements were not met.
    InvalidAnnotation,
    /// Development and qualification splits are incomplete or invalid.
    InvalidSplits,
}

impl CaptchaCorpusDraft {
    /// Validate and consume a complete draft, returning the only corpus shape
    /// accepted for empirical provider qualification.
    pub fn freeze(
        self,
        attestation: CorpusFreezeAttestation,
    ) -> Result<FrozenCaptchaCorpus, CorpusFreezeFailure> {
        validate_governance(&self, &attestation)?;
        if self.cases.len() < MINIMUM_CASES {
            return Err(CorpusFreezeFailure::InsufficientCases);
        }
        validate_assets(&self)?;
        validate_cases(&self)?;
        validate_splits(&self)?;
        let corpus_sha256 = corpus_digest(&self, &attestation);
        Ok(FrozenCaptchaCorpus {
            draft: self,
            attestation,
            corpus_sha256,
        })
    }
}

fn validate_governance(
    draft: &CaptchaCorpusDraft,
    attestation: &CorpusFreezeAttestation,
) -> Result<(), CorpusFreezeFailure> {
    let values = [
        draft.corpus_id.0.as_str(),
        draft.version.0.as_str(),
        draft.source.source_authority.as_str(),
        draft.source.source_revision.as_str(),
        draft.source.acquisition_authorization.as_str(),
        draft.source.rights_record.as_str(),
        attestation.rights_review_record.as_str(),
        attestation.annotation_completion_record.as_str(),
        attestation.qualification_seal_record.as_str(),
        attestation.threshold_policy_id.as_str(),
    ];
    if values.iter().any(|value| invalid_identity(value))
        || !valid_sha256(&draft.source.provenance_sha256)
        || !valid_sha256(&attestation.attestation_sha256)
        || draft.source.source_revision.eq_ignore_ascii_case("latest")
        || draft.version.0.eq_ignore_ascii_case("latest")
    {
        return Err(CorpusFreezeFailure::InvalidGovernance);
    }
    Ok(())
}

fn validate_assets(draft: &CaptchaCorpusDraft) -> Result<(), CorpusFreezeFailure> {
    let mut paths = HashSet::new();
    if draft.assets.is_empty()
        || draft.assets.iter().any(|asset| {
            invalid_relative_path(&asset.path)
                || !paths.insert(asset.path.as_str())
                || asset.size_bytes == 0
                || asset.original_width == 0
                || asset.original_height == 0
                || !valid_sha256(&asset.sha256)
        })
    {
        return Err(CorpusFreezeFailure::InvalidAssetManifest);
    }
    Ok(())
}

fn validate_cases(draft: &CaptchaCorpusDraft) -> Result<(), CorpusFreezeFailure> {
    let assets: HashMap<_, _> = draft.assets.iter().map(|a| (a.path.as_str(), a)).collect();
    let mut ids = HashSet::new();
    let mut used_assets = HashSet::new();
    for case in &draft.cases {
        let Some(asset) = assets.get(case.asset_path.as_str()) else {
            return Err(CorpusFreezeFailure::InvalidCase);
        };
        if invalid_identity(&case.case_id)
            || !ids.insert(case.case_id.as_str())
            || !used_assets.insert(case.asset_path.as_str())
            || case.instruction.trim().is_empty()
            || !ground_truth_valid(draft.challenge_kind, asset, case)
        {
            return Err(CorpusFreezeFailure::InvalidCase);
        }
        validate_annotations(draft.challenge_kind, asset, case)?;
    }
    if used_assets.len() != assets.len() {
        return Err(CorpusFreezeFailure::InvalidAssetManifest);
    }
    Ok(())
}

fn validate_annotations(
    kind: CaptchaChallengeKind,
    asset: &CorpusAsset,
    case: &CaptchaCorpusCase,
) -> Result<(), CorpusFreezeFailure> {
    let mut annotators = HashSet::new();
    if case.independent_annotations.len() < 2
        || case.independent_annotations.iter().any(|annotation| {
            invalid_identity(&annotation.annotator.0)
                || !annotators.insert(annotation.annotator.0.as_str())
                || !valid_sha256(&annotation.annotation_sha256)
                || !truth_value_valid(kind, asset, case, &annotation.ground_truth)
        })
        || annotators.contains(case.adjudication.adjudicator.0.as_str())
        || invalid_identity(&case.adjudication.adjudicator.0)
        || invalid_identity(&case.adjudication.method)
        || !valid_sha256(&case.adjudication.adjudication_sha256)
        || !truth_value_valid(kind, asset, case, &case.adjudication.final_ground_truth)
    {
        return Err(CorpusFreezeFailure::InvalidAnnotation);
    }
    let first = &case.independent_annotations[0].ground_truth;
    let disagreement = case
        .independent_annotations
        .iter()
        .skip(1)
        .any(|annotation| annotation.ground_truth != *first);
    if disagreement != case.adjudication.disagreement_recorded {
        return Err(CorpusFreezeFailure::InvalidAnnotation);
    }
    if !disagreement && case.adjudication.final_ground_truth != *first {
        return Err(CorpusFreezeFailure::InvalidAnnotation);
    }
    Ok(())
}

fn ground_truth_valid(
    kind: CaptchaChallengeKind,
    asset: &CorpusAsset,
    case: &CaptchaCorpusCase,
) -> bool {
    match kind {
        CaptchaChallengeKind::ImageGridSelection => case.image_grid.as_ref().is_some_and(|grid| {
            grid.rows > 0
                && grid.columns > 0
                && usize::from(grid.rows) * usize::from(grid.columns) == grid.choice_ids.len()
                && unique_nonempty(&grid.choice_ids)
        }),
        CaptchaChallengeKind::HorizontalOffset | CaptchaChallengeKind::PointSelection => {
            case.image_grid.is_none() && asset.original_width > 0 && asset.original_height > 0
        }
    }
}

fn truth_value_valid(
    kind: CaptchaChallengeKind,
    asset: &CorpusAsset,
    case: &CaptchaCorpusCase,
    truth: &CaptchaGroundTruth,
) -> bool {
    match (kind, truth) {
        (
            CaptchaChallengeKind::ImageGridSelection,
            CaptchaGroundTruth::ImageGrid {
                selected_choice_ids,
            },
        ) => case.image_grid.as_ref().is_some_and(|grid| {
            (grid.empty_selection_valid || !selected_choice_ids.is_empty())
                && unique_nonempty(selected_choice_ids)
                && selected_choice_ids
                    .iter()
                    .all(|id| grid.choice_ids.contains(id))
        }),
        (
            CaptchaChallengeKind::HorizontalOffset,
            CaptchaGroundTruth::HorizontalOffset {
                initial_piece_x,
                displacement_px,
                tolerance_px,
                annotation_method,
            },
        ) => {
            initial_piece_x.is_none_or(|x| finite_in_bounds(x, asset.original_width))
                && displacement_px.is_finite()
                && *displacement_px >= 0.0
                && *displacement_px <= f64::from(asset.original_width)
                && tolerance_px.is_finite()
                && *tolerance_px > 0.0
                && !annotation_method.trim().is_empty()
        }
        (
            CaptchaChallengeKind::PointSelection,
            CaptchaGroundTruth::PointSelection {
                x,
                y,
                accepted_region,
                tolerance_px,
                annotation_method,
            },
        ) => {
            finite_in_bounds(*x, asset.original_width)
                && finite_in_bounds(*y, asset.original_height)
                && accepted_region.is_none_or(|[x0, y0, x1, y1]| {
                    finite_in_bounds(x0, asset.original_width)
                        && finite_in_bounds(x1, asset.original_width)
                        && finite_in_bounds(y0, asset.original_height)
                        && finite_in_bounds(y1, asset.original_height)
                        && x0 <= x1
                        && y0 <= y1
                })
                && tolerance_px.is_finite()
                && *tolerance_px > 0.0
                && !annotation_method.trim().is_empty()
        }
        _ => false,
    }
}

fn validate_splits(draft: &CaptchaCorpusDraft) -> Result<(), CorpusFreezeFailure> {
    let cases: HashSet<_> = draft
        .cases
        .iter()
        .map(|case| case.case_id.as_str())
        .collect();
    let mut assigned = HashSet::new();
    let mut development = 0usize;
    let mut qualification = 0usize;
    for assignment in &draft.splits {
        if !cases.contains(assignment.case_id.as_str())
            || !assigned.insert(assignment.case_id.as_str())
        {
            return Err(CorpusFreezeFailure::InvalidSplits);
        }
        match assignment.split {
            CorpusSplit::Development => development += 1,
            CorpusSplit::Qualification => qualification += 1,
        }
    }
    if assigned.len() != cases.len() || development == 0 || qualification == 0 {
        return Err(CorpusFreezeFailure::InvalidSplits);
    }
    Ok(())
}

fn corpus_digest(draft: &CaptchaCorpusDraft, attestation: &CorpusFreezeAttestation) -> String {
    let mut digest = Sha256::new();
    feed(&mut digest, &draft.corpus_id.0);
    feed(&mut digest, &draft.version.0);
    feed(&mut digest, kind_name(draft.challenge_kind));
    for value in [
        &draft.source.source_authority,
        &draft.source.source_revision,
        &draft.source.acquisition_authorization,
        &draft.source.rights_record,
        &draft.source.provenance_sha256,
    ] {
        feed(&mut digest, value);
    }
    let mut assets: Vec<_> = draft.assets.iter().collect();
    assets.sort_by_key(|asset| &asset.path);
    for asset in assets {
        feed(&mut digest, &asset.path);
        feed(&mut digest, &asset.size_bytes.to_string());
        feed(&mut digest, &asset.sha256);
        feed(&mut digest, &asset.original_width.to_string());
        feed(&mut digest, &asset.original_height.to_string());
    }
    let mut cases: Vec<_> = draft.cases.iter().collect();
    cases.sort_by_key(|case| &case.case_id);
    for case in cases {
        feed(&mut digest, &case.case_id);
        feed(&mut digest, &case.asset_path);
        feed(&mut digest, &case.instruction);
        feed_case(&mut digest, case);
    }
    let mut splits: Vec<_> = draft.splits.iter().collect();
    splits.sort_by_key(|split| &split.case_id);
    for split in splits {
        feed(&mut digest, &split.case_id);
        feed(
            &mut digest,
            match split.split {
                CorpusSplit::Development => "development",
                CorpusSplit::Qualification => "qualification",
            },
        );
    }
    for value in [
        &attestation.rights_review_record,
        &attestation.annotation_completion_record,
        &attestation.qualification_seal_record,
        &attestation.threshold_policy_id,
        &attestation.attestation_sha256,
    ] {
        feed(&mut digest, value);
    }
    format!("{:x}", digest.finalize())
}

fn feed_case(digest: &mut Sha256, case: &CaptchaCorpusCase) {
    if let Some(grid) = &case.image_grid {
        feed(digest, &grid.rows.to_string());
        feed(digest, &grid.columns.to_string());
        feed(
            digest,
            if grid.empty_selection_valid {
                "empty-ok"
            } else {
                "nonempty"
            },
        );
        for choice in &grid.choice_ids {
            feed(digest, choice);
        }
    }
    for annotation in &case.independent_annotations {
        feed(digest, &annotation.annotator.0);
        feed(digest, &annotation.annotation_sha256);
        feed_truth(digest, &annotation.ground_truth);
    }
    feed(digest, &case.adjudication.adjudicator.0);
    feed(digest, &case.adjudication.method);
    feed(digest, &case.adjudication.adjudication_sha256);
    feed(
        digest,
        if case.adjudication.disagreement_recorded {
            "disagreed"
        } else {
            "agreed"
        },
    );
    feed_truth(digest, &case.adjudication.final_ground_truth);
}

fn feed_truth(digest: &mut Sha256, truth: &CaptchaGroundTruth) {
    match truth {
        CaptchaGroundTruth::ImageGrid {
            selected_choice_ids,
        } => {
            feed(digest, "grid");
            for id in selected_choice_ids {
                feed(digest, id);
            }
        }
        CaptchaGroundTruth::HorizontalOffset {
            initial_piece_x,
            displacement_px,
            tolerance_px,
            annotation_method,
        } => {
            feed(digest, "offset");
            feed(digest, &format!("{initial_piece_x:?}"));
            feed(digest, &displacement_px.to_bits().to_string());
            feed(digest, &tolerance_px.to_bits().to_string());
            feed(digest, annotation_method);
        }
        CaptchaGroundTruth::PointSelection {
            x,
            y,
            accepted_region,
            tolerance_px,
            annotation_method,
        } => {
            feed(digest, "point");
            feed(digest, &x.to_bits().to_string());
            feed(digest, &y.to_bits().to_string());
            feed(digest, &format!("{accepted_region:?}"));
            feed(digest, &tolerance_px.to_bits().to_string());
            feed(digest, annotation_method);
        }
    }
}

fn feed(digest: &mut Sha256, value: &str) {
    digest.update(value.len().to_le_bytes());
    digest.update(value.as_bytes());
}

fn kind_name(kind: CaptchaChallengeKind) -> &'static str {
    match kind {
        CaptchaChallengeKind::ImageGridSelection => "image-grid-selection",
        CaptchaChallengeKind::HorizontalOffset => "horizontal-offset",
        CaptchaChallengeKind::PointSelection => "point-selection",
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn invalid_identity(value: &str) -> bool {
    value.trim().is_empty() || value.contains('\n') || value.contains('\r')
}

fn invalid_relative_path(path: &str) -> bool {
    path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

fn unique_nonempty(values: &[String]) -> bool {
    let mut unique = HashSet::new();
    values
        .iter()
        .all(|value| !value.trim().is_empty() && unique.insert(value))
}

fn finite_in_bounds(value: f64, dimension: u32) -> bool {
    value.is_finite() && value >= 0.0 && value < f64::from(dimension)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn grid_truth() -> CaptchaGroundTruth {
        CaptchaGroundTruth::ImageGrid {
            selected_choice_ids: vec!["choice-0".into()],
        }
    }

    fn draft(count: usize) -> CaptchaCorpusDraft {
        let mut assets = Vec::new();
        let mut cases = Vec::new();
        let mut splits = Vec::new();
        for index in 0..count {
            let path = format!("assets/{index}.png");
            let case_id = format!("case-{index:03}");
            assets.push(CorpusAsset {
                path: path.clone(),
                size_bytes: 1,
                sha256: HASH.into(),
                original_width: 320,
                original_height: 224,
            });
            cases.push(CaptchaCorpusCase {
                case_id: case_id.clone(),
                asset_path: path,
                instruction: "Select every bus".into(),
                image_grid: Some(ImageGridLayout {
                    rows: 1,
                    columns: 2,
                    choice_ids: vec!["choice-0".into(), "choice-1".into()],
                    empty_selection_valid: true,
                }),
                independent_annotations: vec![
                    IndependentAnnotation {
                        annotator: AnnotatorId("annotator-a".into()),
                        ground_truth: grid_truth(),
                        annotation_sha256: HASH.into(),
                    },
                    IndependentAnnotation {
                        annotator: AnnotatorId("annotator-b".into()),
                        ground_truth: grid_truth(),
                        annotation_sha256: HASH.into(),
                    },
                ],
                adjudication: CorpusAdjudication {
                    disagreement_recorded: false,
                    final_ground_truth: grid_truth(),
                    adjudicator: AnnotatorId("adjudicator".into()),
                    method: "two-reviewer-adjudication-v1".into(),
                    adjudication_sha256: HASH.into(),
                },
            });
            splits.push(CorpusSplitAssignment {
                case_id,
                split: if index < count / 4 {
                    CorpusSplit::Development
                } else {
                    CorpusSplit::Qualification
                },
            });
        }
        CaptchaCorpusDraft {
            corpus_id: CorpusId("authorized-grid-corpus".into()),
            version: CorpusVersion("1.0.0".into()),
            challenge_kind: CaptchaChallengeKind::ImageGridSelection,
            source: CorpusSourceProvenance {
                source_authority: "authorized-owner".into(),
                source_revision: "revision-001".into(),
                acquisition_authorization: "authorization-record-001".into(),
                rights_record: "evaluation-rights-001".into(),
                provenance_sha256: HASH.into(),
            },
            assets,
            cases,
            splits,
        }
    }

    fn attestation() -> CorpusFreezeAttestation {
        CorpusFreezeAttestation {
            rights_review_record: "rights-review-001".into(),
            annotation_completion_record: "annotation-freeze-001".into(),
            qualification_seal_record: "sealed-test-split-001".into(),
            threshold_policy_id: "qwen-captcha-thresholds-v1".into(),
            attestation_sha256: HASH.into(),
        }
    }

    #[test]
    fn complete_independent_corpus_freezes_deterministically() {
        let first = draft(200).freeze(attestation()).unwrap();
        let second = draft(200).freeze(attestation()).unwrap();
        assert_eq!(first.corpus_sha256(), second.corpus_sha256());
        assert_eq!(first.cases().len(), 200);
        assert!(first
            .splits()
            .iter()
            .any(|s| s.split == CorpusSplit::Development));
        assert!(first
            .splits()
            .iter()
            .any(|s| s.split == CorpusSplit::Qualification));
    }

    #[test]
    fn minimum_size_is_enforced_per_family() {
        assert_eq!(
            draft(199).freeze(attestation()).unwrap_err(),
            CorpusFreezeFailure::InsufficientCases
        );
    }

    #[test]
    fn annotations_must_be_independent_and_disagreement_truthful() {
        let mut duplicate = draft(200);
        duplicate.cases[0].independent_annotations[1].annotator = AnnotatorId("annotator-a".into());
        assert_eq!(
            duplicate.freeze(attestation()).unwrap_err(),
            CorpusFreezeFailure::InvalidAnnotation
        );
        let mut hidden_disagreement = draft(200);
        hidden_disagreement.cases[0].independent_annotations[1].ground_truth =
            CaptchaGroundTruth::ImageGrid {
                selected_choice_ids: vec!["choice-1".into()],
            };
        assert_eq!(
            hidden_disagreement.freeze(attestation()).unwrap_err(),
            CorpusFreezeFailure::InvalidAnnotation
        );
    }

    #[test]
    fn splits_are_total_disjoint_and_nonempty() {
        let mut missing = draft(200);
        missing.splits.pop();
        assert_eq!(
            missing.freeze(attestation()).unwrap_err(),
            CorpusFreezeFailure::InvalidSplits
        );
        let mut duplicate = draft(200);
        duplicate.splits.push(duplicate.splits[0].clone());
        assert_eq!(
            duplicate.freeze(attestation()).unwrap_err(),
            CorpusFreezeFailure::InvalidSplits
        );
    }

    #[test]
    fn digest_binds_assets_annotations_labels_and_splits() {
        let original = draft(200)
            .freeze(attestation())
            .unwrap()
            .corpus_sha256()
            .to_owned();
        let mut changed = draft(200);
        changed.assets[0].sha256 =
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
        assert_ne!(
            original,
            changed.freeze(attestation()).unwrap().corpus_sha256()
        );
        let mut changed = draft(200);
        for annotation in &mut changed.cases[0].independent_annotations {
            annotation.ground_truth = CaptchaGroundTruth::ImageGrid {
                selected_choice_ids: vec!["choice-1".into()],
            };
        }
        changed.cases[0].adjudication.final_ground_truth = CaptchaGroundTruth::ImageGrid {
            selected_choice_ids: vec!["choice-1".into()],
        };
        assert_ne!(
            original,
            changed.freeze(attestation()).unwrap().corpus_sha256()
        );
        let mut changed = draft(200);
        changed.splits[0].split = CorpusSplit::Qualification;
        assert_ne!(
            original,
            changed.freeze(attestation()).unwrap().corpus_sha256()
        );
    }

    #[test]
    fn mutable_identity_and_missing_rights_fail_closed() {
        let mut mutable = draft(200);
        mutable.version = CorpusVersion("latest".into());
        assert_eq!(
            mutable.freeze(attestation()).unwrap_err(),
            CorpusFreezeFailure::InvalidGovernance
        );
        let mut no_rights = attestation();
        no_rights.rights_review_record.clear();
        assert_eq!(
            draft(200).freeze(no_rights).unwrap_err(),
            CorpusFreezeFailure::InvalidGovernance
        );
    }
}
