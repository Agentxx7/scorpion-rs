//! Thin canonical binding between one immutable browser challenge attempt and
//! the provider-neutral CAPTCHA solver seam.

#![cfg(feature = "chrome")]

use std::collections::HashSet;
use std::time::Duration;

use chromiumoxide::Page;

use crate::features::browser_challenge::{
    BrowserChallengeAction, BrowserChallengeFailure, BrowserChallengeSnapshot,
};
use crate::features::captcha::{
    CaptchaChallenge, CaptchaChallengeKind, CaptchaImageGridCell, CaptchaImageGridInput,
    CaptchaImageGridValidationError, CaptchaProviderId, CaptchaProviderRegistry,
    CaptchaRouteAttempts, CaptchaSolution, CaptchaSolveOutcome, CaptchaSolveRequest,
    CaptchaVisualInput,
};
use crate::features::frame_context::FrameContext;

/// Caller-supplied row and column identity for one already-bound grid target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptchaBrowserGridCell {
    /// Stable ID already bound by [`BrowserChallengeSnapshot`].
    pub choice_id: String,
    /// Zero-based grid row.
    pub row: usize,
    /// Zero-based grid column.
    pub column: usize,
}

/// Browser extraction facts needed to materialize one canonical challenge.
#[derive(Clone, Debug)]
pub enum CaptchaBrowserChallenge {
    /// One already-captured full image with exact retained cell targets.
    ImageGridSelection {
        /// Semantic provider-neutral instruction.
        instruction: String,
        /// Number of grid rows.
        rows: usize,
        /// Number of grid columns.
        columns: usize,
        /// Explicit row/column identity for every retained browser target.
        cells: Vec<CaptchaBrowserGridCell>,
        /// Whether an empty selection is a successful answer.
        empty_selection_valid: bool,
    },
    /// Select one point in the immutable captured image.
    PointSelection {
        /// Semantic provider-neutral instruction.
        instruction: String,
    },
    /// Drag one exact retained handle by the returned horizontal offset.
    HorizontalOffset {
        /// Semantic provider-neutral instruction.
        instruction: String,
        /// Stable ID of the exact handle retained in the snapshot.
        handle_target_id: String,
    },
}

/// One explicitly routed immutable browser CAPTCHA attempt.
#[derive(Clone, Debug)]
pub struct CaptchaBrowserAttempt {
    /// Caller-owned attempt correlation identity.
    pub correlation_id: String,
    /// Exactly one provider selected before execution.
    pub selected_provider: CaptchaProviderId,
    /// Caller-owned provider operation deadline.
    pub deadline: Duration,
    /// Already-identified browser challenge form.
    pub challenge: CaptchaBrowserChallenge,
}

/// Stage reached by one browser binding invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptchaBrowserExecutionStage {
    /// No provider solution was produced.
    Materialization,
    /// Provider execution completed without a usable solution.
    ProviderExecution,
    /// A solution exists but has not passed browser-side validation.
    SolutionProduced,
    /// The immutable snapshot was revalidated immediately before action.
    SnapshotRevalidated,
    /// One or more exact browser actions were applied.
    ActionApplied,
}

/// Typed binding failure; the provider attempt ledger and partial action count
/// are retained without converting action dispatch into a solved claim.
#[derive(Debug)]
pub struct CaptchaBrowserExecutionFailure {
    /// Exact failure classification.
    pub kind: CaptchaBrowserExecutionFailureKind,
    /// Furthest truthful stage reached.
    pub stage: CaptchaBrowserExecutionStage,
    /// Canonical explicit provider attempt, if routing began.
    pub attempts: CaptchaRouteAttempts,
    /// Number of exact browser actions successfully dispatched before failure.
    pub actions_applied: usize,
}

/// Failure classes owned by the thin browser/CAPTCHA composition seam.
#[derive(Debug)]
pub enum CaptchaBrowserExecutionFailureKind {
    /// Canonical full-grid representation rejected extracted facts.
    Materialization(CaptchaImageGridValidationError),
    /// Explicitly selected provider was unavailable or failed.
    ProviderFailure,
    /// Provider returned a different solution kind than the challenge.
    SolutionKindMismatch,
    /// A returned grid ID was not bound by this immutable snapshot.
    UnknownChoiceIdentity,
    /// A returned empty selection contradicted challenge semantics.
    EmptySelectionNotAllowed,
    /// A point or offset was non-finite or outside snapshot bounds.
    SolutionOutOfBounds,
    /// Canonical snapshot revalidation or exact action failed.
    Browser(BrowserChallengeFailure),
}

/// Caller-observed progression is deliberately outside this binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptchaBrowserProgression {
    /// The binding dispatched actions but made no solved/progression claim.
    NotObservedByBinding,
}

/// Successful solve and exact action dispatch facts for one immutable attempt.
#[derive(Debug)]
pub struct CaptchaBrowserExecutionReport {
    /// Canonical explicit provider attempt with full outcome/provenance.
    pub attempts: CaptchaRouteAttempts,
    /// Furthest truthful stage reached by the binding.
    pub stage: CaptchaBrowserExecutionStage,
    /// Number of exact browser actions successfully dispatched.
    pub actions_applied: usize,
    /// Progression remains caller-observed after this binding returns.
    pub progression: CaptchaBrowserProgression,
}

/// Materialize, explicitly route, revalidate and apply one immutable attempt.
pub async fn execute_browser_captcha_attempt(
    page: &Page,
    snapshot: &BrowserChallengeSnapshot,
    registry: &CaptchaProviderRegistry<'_>,
    attempt: CaptchaBrowserAttempt,
) -> Result<CaptchaBrowserExecutionReport, CaptchaBrowserExecutionFailure> {
    let (attempts, actions) = solve_and_select_actions(snapshot, registry, attempt).await?;
    if let Err(browser) = snapshot.revalidate(page).await {
        return Err(CaptchaBrowserExecutionFailure {
            kind: CaptchaBrowserExecutionFailureKind::Browser(browser),
            stage: CaptchaBrowserExecutionStage::SolutionProduced,
            attempts,
            actions_applied: 0,
        });
    }
    let mut applied = 0;
    for action in actions {
        if let Err(browser) = snapshot.apply(page, action).await {
            return Err(CaptchaBrowserExecutionFailure {
                kind: CaptchaBrowserExecutionFailureKind::Browser(browser),
                stage: CaptchaBrowserExecutionStage::SnapshotRevalidated,
                attempts,
                actions_applied: applied,
            });
        }
        applied += 1;
    }
    Ok(CaptchaBrowserExecutionReport {
        attempts,
        stage: if applied == 0 {
            CaptchaBrowserExecutionStage::SnapshotRevalidated
        } else {
            CaptchaBrowserExecutionStage::ActionApplied
        },
        actions_applied: applied,
        progression: CaptchaBrowserProgression::NotObservedByBinding,
    })
}

/// Identical contract to [`execute_browser_captcha_attempt`], composing the
/// canonical frame-aware browser-challenge seam
/// (`BrowserChallengeSnapshot::revalidate_in_frame`/`apply_in_frame`) instead
/// of the top-level-only pair, for a challenge captured inside a genuine
/// child frame (same-origin or OOPIF) via
/// [`BrowserChallengeSnapshot::capture_in_frame`]. Materialization and
/// solution-to-action translation are unchanged and shared exactly — both
/// operate only on already-captured snapshot facts, never a live page/frame
/// handle, so a frame-captured snapshot needs no different treatment there.
pub async fn execute_browser_captcha_attempt_in_frame(
    page: &Page,
    top_level: &FrameContext,
    frame: &FrameContext,
    snapshot: &BrowserChallengeSnapshot,
    registry: &CaptchaProviderRegistry<'_>,
    attempt: CaptchaBrowserAttempt,
) -> Result<CaptchaBrowserExecutionReport, CaptchaBrowserExecutionFailure> {
    let (attempts, actions) = solve_and_select_actions(snapshot, registry, attempt).await?;
    if let Err(browser) = snapshot.revalidate_in_frame(page, top_level, frame).await {
        return Err(CaptchaBrowserExecutionFailure {
            kind: CaptchaBrowserExecutionFailureKind::Browser(browser),
            stage: CaptchaBrowserExecutionStage::SolutionProduced,
            attempts,
            actions_applied: 0,
        });
    }
    let mut applied = 0;
    for action in actions {
        if let Err(browser) = snapshot
            .apply_in_frame(page, top_level, frame, action)
            .await
        {
            return Err(CaptchaBrowserExecutionFailure {
                kind: CaptchaBrowserExecutionFailureKind::Browser(browser),
                stage: CaptchaBrowserExecutionStage::SnapshotRevalidated,
                attempts,
                actions_applied: applied,
            });
        }
        applied += 1;
    }
    Ok(CaptchaBrowserExecutionReport {
        attempts,
        stage: if applied == 0 {
            CaptchaBrowserExecutionStage::SnapshotRevalidated
        } else {
            CaptchaBrowserExecutionStage::ActionApplied
        },
        actions_applied: applied,
        progression: CaptchaBrowserProgression::NotObservedByBinding,
    })
}

/// Materialize, explicitly route and translate one attempt's solution into
/// exact browser actions, without touching a page/frame handle — the one
/// piece shared verbatim by both the top-level and frame-aware entry points.
async fn solve_and_select_actions(
    snapshot: &BrowserChallengeSnapshot,
    registry: &CaptchaProviderRegistry<'_>,
    attempt: CaptchaBrowserAttempt,
) -> Result<(CaptchaRouteAttempts, Vec<BrowserChallengeAction>), CaptchaBrowserExecutionFailure> {
    let request = materialize_request(snapshot, &attempt).map_err(failure)?;
    let mut attempts = CaptchaRouteAttempts::new();
    let solution = match attempts.execute_explicit_attempt(registry, &request).await {
        CaptchaSolveOutcome::Solved { solution, .. } => solution.clone(),
        CaptchaSolveOutcome::Failed { .. } => {
            return Err(CaptchaBrowserExecutionFailure {
                kind: CaptchaBrowserExecutionFailureKind::ProviderFailure,
                stage: CaptchaBrowserExecutionStage::ProviderExecution,
                attempts,
                actions_applied: 0,
            });
        }
    };
    let actions = match actions_for_solution(snapshot, &attempt.challenge, solution) {
        Ok(actions) => actions,
        Err(kind) => {
            return Err(CaptchaBrowserExecutionFailure {
                kind,
                stage: CaptchaBrowserExecutionStage::SolutionProduced,
                attempts,
                actions_applied: 0,
            });
        }
    };
    Ok((attempts, actions))
}

fn failure(kind: CaptchaBrowserExecutionFailureKind) -> CaptchaBrowserExecutionFailure {
    CaptchaBrowserExecutionFailure {
        kind,
        stage: CaptchaBrowserExecutionStage::Materialization,
        attempts: CaptchaRouteAttempts::new(),
        actions_applied: 0,
    }
}

fn materialize_request(
    snapshot: &BrowserChallengeSnapshot,
    attempt: &CaptchaBrowserAttempt,
) -> Result<CaptchaSolveRequest, CaptchaBrowserExecutionFailureKind> {
    let visual = CaptchaVisualInput::materialized(None, "image/png", snapshot.visual_bytes.clone());
    let (kind, instruction, visual) = match &attempt.challenge {
        CaptchaBrowserChallenge::ImageGridSelection {
            instruction,
            rows,
            columns,
            cells,
            empty_selection_valid,
        } => {
            if cells.len() != snapshot.targets.len() {
                return Err(CaptchaBrowserExecutionFailureKind::UnknownChoiceIdentity);
            }
            let mut observed = HashSet::with_capacity(cells.len());
            let mut canonical = Vec::with_capacity(cells.len());
            for cell in cells {
                if !observed.insert(cell.choice_id.as_str()) {
                    return Err(CaptchaBrowserExecutionFailureKind::UnknownChoiceIdentity);
                }
                let target = snapshot
                    .targets
                    .get(&cell.choice_id)
                    .ok_or(CaptchaBrowserExecutionFailureKind::UnknownChoiceIdentity)?;
                let (x, y, width, height) = snapshot
                    .transform
                    .browser_rect_to_image(target.geometry)
                    .map_err(CaptchaBrowserExecutionFailureKind::Browser)?;
                canonical.push(CaptchaImageGridCell::new(
                    cell.choice_id.clone(),
                    cell.row,
                    cell.column,
                    x,
                    y,
                    width,
                    height,
                ));
            }
            let grid = CaptchaImageGridInput::new(
                visual,
                (
                    snapshot.captured_pixel_width,
                    snapshot.captured_pixel_height,
                ),
                *rows,
                *columns,
                canonical,
                *empty_selection_valid,
            )
            .map_err(CaptchaBrowserExecutionFailureKind::Materialization)?;
            (
                CaptchaChallengeKind::ImageGridSelection,
                instruction.clone(),
                CaptchaVisualInput::materialized_full_grid(grid),
            )
        }
        CaptchaBrowserChallenge::PointSelection { instruction } => (
            CaptchaChallengeKind::PointSelection,
            instruction.clone(),
            visual,
        ),
        CaptchaBrowserChallenge::HorizontalOffset {
            instruction,
            handle_target_id,
        } => {
            if !snapshot.targets.contains_key(handle_target_id) {
                return Err(CaptchaBrowserExecutionFailureKind::UnknownChoiceIdentity);
            }
            (
                CaptchaChallengeKind::HorizontalOffset,
                instruction.clone(),
                visual,
            )
        }
    };
    Ok(CaptchaSolveRequest {
        correlation_id: attempt.correlation_id.clone(),
        selected_provider: attempt.selected_provider,
        challenge: CaptchaChallenge {
            kind,
            instruction,
            visuals: vec![visual],
        },
        deadline: attempt.deadline,
    })
}

fn actions_for_solution(
    snapshot: &BrowserChallengeSnapshot,
    challenge: &CaptchaBrowserChallenge,
    solution: CaptchaSolution,
) -> Result<Vec<BrowserChallengeAction>, CaptchaBrowserExecutionFailureKind> {
    match (challenge, solution) {
        (
            CaptchaBrowserChallenge::ImageGridSelection {
                empty_selection_valid,
                ..
            },
            CaptchaSolution::SelectedChoices(ids),
        ) => {
            if ids.is_empty() && !empty_selection_valid {
                return Err(CaptchaBrowserExecutionFailureKind::EmptySelectionNotAllowed);
            }
            let mut observed = HashSet::with_capacity(ids.len());
            ids.into_iter()
                .map(|stable_id| {
                    if !observed.insert(stable_id.clone())
                        || !snapshot.targets.contains_key(&stable_id)
                    {
                        return Err(CaptchaBrowserExecutionFailureKind::UnknownChoiceIdentity);
                    }
                    Ok(BrowserChallengeAction::ExactTargetClick { stable_id })
                })
                .collect()
        }
        (CaptchaBrowserChallenge::PointSelection { .. }, CaptchaSolution::Point { x, y }) => {
            snapshot
                .transform
                .image_to_browser(x, y)
                .map_err(|_| CaptchaBrowserExecutionFailureKind::SolutionOutOfBounds)?;
            Ok(vec![BrowserChallengeAction::ExactPoint { x, y }])
        }
        (
            CaptchaBrowserChallenge::HorizontalOffset {
                handle_target_id, ..
            },
            CaptchaSolution::HorizontalOffset(offset),
        ) => Ok(vec![BrowserChallengeAction::ExactHorizontalDrag(
            snapshot
                .horizontal_drag_from_target(handle_target_id, offset)
                .map_err(|_| CaptchaBrowserExecutionFailureKind::SolutionOutOfBounds)?,
        )]),
        _ => Err(CaptchaBrowserExecutionFailureKind::SolutionKindMismatch),
    }
}
