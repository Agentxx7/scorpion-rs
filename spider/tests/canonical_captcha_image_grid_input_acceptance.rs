//! Acceptance contract for canonical materialized full-grid CAPTCHA input.

use spider::features::captcha::{
    CaptchaImageGridCell, CaptchaImageGridInput, CaptchaImageGridValidationError,
    CaptchaVisualInput,
};

fn cells(rows: usize, columns: usize) -> Vec<CaptchaImageGridCell> {
    (0..rows)
        .flat_map(|row| {
            (0..columns).map(move |column| {
                CaptchaImageGridCell::new(
                    format!("stable-{row}-{column}"),
                    row,
                    column,
                    column as u32 * 20,
                    row as u32 * 20,
                    20,
                    20,
                )
            })
        })
        .collect()
}

fn build(
    rows: usize,
    columns: usize,
    cells: Vec<CaptchaImageGridCell>,
    empty: bool,
) -> Result<CaptchaImageGridInput, CaptchaImageGridValidationError> {
    CaptchaImageGridInput::new(
        CaptchaVisualInput::materialized(None, "image/png", [1, 2, 3]),
        (columns as u32 * 20, rows as u32 * 20),
        rows,
        columns,
        cells,
        empty,
    )
}

#[test]
fn three_by_three_is_a_single_materialized_full_grid() {
    let visual =
        CaptchaVisualInput::materialized_full_grid(build(3, 3, cells(3, 3), false).unwrap());
    let grid = visual.image_grid().unwrap();
    assert_eq!(grid.layout(), (3, 3));
    assert_eq!(grid.original_dimensions(), (60, 60));
    assert_eq!(visual.bytes(), Some([1, 2, 3].as_slice()));
}

#[test]
fn four_by_four_retains_all_explicit_stable_ids() {
    let grid = build(4, 4, cells(4, 4), false).unwrap();
    assert_eq!(grid.cells().len(), 16);
    assert_eq!(grid.cells()[0].choice_id(), "stable-0-0");
    assert_eq!(grid.cells()[15].choice_id(), "stable-3-3");
}

#[test]
fn supplied_order_never_changes_explicit_cell_identity() {
    let mut values = cells(3, 3);
    values.rotate_left(4);
    let grid = build(3, 3, values, false).unwrap();
    for (index, cell) in grid.cells().iter().enumerate() {
        assert_eq!((cell.row(), cell.column()), (index / 3, index % 3));
        assert_eq!(
            cell.choice_id(),
            format!("stable-{}-{}", index / 3, index % 3)
        );
    }
}

#[test]
fn duplicate_identity_and_incomplete_membership_fail_closed() {
    let mut duplicate = cells(3, 3);
    duplicate[1] = CaptchaImageGridCell::new("stable-0-0", 0, 1, 20, 0, 20, 20);
    assert_eq!(
        build(3, 3, duplicate, false).unwrap_err(),
        CaptchaImageGridValidationError::InvalidChoiceIdentity
    );
    let mut missing = cells(3, 3);
    missing.pop();
    assert_eq!(
        build(3, 3, missing, false).unwrap_err(),
        CaptchaImageGridValidationError::ChoiceCountMismatch
    );
}

#[test]
fn geometry_outside_image_or_ambiguous_overlap_fails_closed() {
    let mut outside = cells(3, 3);
    outside[8] = CaptchaImageGridCell::new("stable-2-2", 2, 2, 40, 40, 21, 20);
    assert_eq!(
        build(3, 3, outside, false).unwrap_err(),
        CaptchaImageGridValidationError::CellOutsideImage
    );
    let mut overlap = cells(3, 3);
    overlap[1] = CaptchaImageGridCell::new("stable-0-1", 0, 1, 19, 0, 20, 20);
    assert_eq!(
        build(3, 3, overlap, false).unwrap_err(),
        CaptchaImageGridValidationError::AmbiguousCellOverlap
    );
}

#[test]
fn empty_selection_semantics_are_explicit_not_prompt_inferred() {
    assert!(build(3, 3, cells(3, 3), true)
        .unwrap()
        .empty_selection_valid());
    assert!(!build(3, 3, cells(3, 3), false)
        .unwrap()
        .empty_selection_valid());
}

#[test]
fn remote_asset_cannot_masquerade_as_materialized_full_grid() {
    let remote = CaptchaVisualInput::RemoteAsset {
        id: None,
        media_type: "image/png".into(),
        url: url::Url::parse("https://example.invalid/grid.png").unwrap(),
    };
    assert_eq!(
        CaptchaImageGridInput::new(remote, (60, 60), 3, 3, cells(3, 3), false).unwrap_err(),
        CaptchaImageGridValidationError::FullGridNotMaterialized
    );
}

#[test]
fn existing_multi_visual_form_remains_distinct() {
    let ordinary = vec![
        CaptchaVisualInput::materialized(Some("a".into()), "image/png", [1]),
        CaptchaVisualInput::materialized(Some("b".into()), "image/png", [2]),
    ];
    assert!(ordinary.iter().all(|visual| visual.image_grid().is_none()));
}
