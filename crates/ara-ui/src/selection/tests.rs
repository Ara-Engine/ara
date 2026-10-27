use super::*;

const SHIFT: Modifiers = Modifiers {
    extend: true,
    additive: false,
};
const PRIMARY: Modifiers = Modifiers {
    extend: false,
    additive: true,
};

#[test]
fn range_crosses_unmounted_rows_shrinks_and_escape_restores_original_selection() {
    let order: Vec<_> = (0..100_000).collect();
    let mut selection = Selection::default();
    selection.select(&order, &7, Modifiers::default());
    selection.select(&order, &80_000, SHIFT);
    assert_eq!(selection.selected().len(), 79_994);
    selection.navigate(&order, Navigation::First, SHIFT);
    assert_eq!(selection.selected(), &(0..=7).collect());
    assert_eq!(selection.anchor(), Some(&7));
    assert!(selection.cancel_range());
    assert_eq!(selection.selected(), &[7].into_iter().collect());
    assert_eq!(selection.active(), Some(&7));
}

#[test]
fn additive_ranges_keep_their_base_and_primary_arrows_only_move_focus() {
    let order: Vec<_> = (0..12).collect();
    let mut selection = Selection::default();
    selection.select(&order, &1, Modifiers::default());
    selection.select(&order, &7, PRIMARY);
    selection.select(
        &order,
        &10,
        Modifiers {
            extend: true,
            additive: true,
        },
    );
    assert_eq!(
        selection.selected(),
        &[1, 7, 8, 9, 10].into_iter().collect()
    );
    selection.select(
        &order,
        &8,
        Modifiers {
            extend: true,
            additive: true,
        },
    );
    assert_eq!(selection.selected(), &[1, 7, 8].into_iter().collect());
    selection.navigate(&order, Navigation::Next, PRIMARY);
    assert_eq!(selection.active(), Some(&9));
    assert!(!selection.selected().contains(&9));
    selection.select(&order, &1, PRIMARY);
    assert!(!selection.selected().contains(&1));
}

#[test]
fn filtering_and_deleted_anchor_repair_do_not_erase_hidden_valid_keys() {
    let order = [10, 20, 30, 40, 50];
    let mut selection = Selection::default();
    selection.select(&order, &10, Modifiers::default());
    selection.select(&order, &40, PRIMARY);
    selection.reconcile(&[20, 30, 40], |key| *key != 10);
    assert_eq!(selection.anchor(), Some(&40));
    assert_eq!(selection.selected(), &[40].into_iter().collect());
    selection.select(&[20, 30, 40], &20, SHIFT);
    assert_eq!(selection.selected(), &[20, 30, 40].into_iter().collect());
    selection.reconcile(&[20, 40], |_| true);
    assert!(
        selection.selected().contains(&30),
        "filtered rows remain selected"
    );
    selection.reconcile(&[20, 40], |key| *key != 30);
    assert!(
        !selection.selected().contains(&30),
        "deleted rows are pruned"
    );
}

#[test]
fn source_reorder_uses_stable_anchor_and_new_display_order() {
    let mut selection = Selection::default();
    selection.select(&[1, 2, 3, 4], &2, Modifiers::default());
    selection.select(&[1, 2, 3, 4], &4, SHIFT);
    let reordered = [4, 1, 2, 3];
    selection.reconcile(&reordered, |_| true);
    selection.select(&reordered, &4, SHIFT);
    assert_eq!(selection.selected(), &[1, 2, 4].into_iter().collect());
    selection.apply([2, 2, 3], SetOperation::Toggle);
    assert_eq!(selection.selected(), &[1, 3, 4].into_iter().collect());
}

#[test]
fn focused_unselected_item_remains_a_keyboard_destination_after_clear() {
    let order: Vec<_> = (0..1_000).collect();
    let mut selection = Selection::default();
    selection.clear();
    selection.focus(500);
    selection.navigate(&order, Navigation::Next, PRIMARY);
    assert_eq!(selection.active(), Some(&501));
    assert!(selection.selected().is_empty());
    selection.navigate(&order, Navigation::Next, SHIFT);
    assert_eq!(selection.selected(), &[501, 502].into_iter().collect());
}
