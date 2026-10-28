use super::*;
use ara_ui::{
    theme::EDITOR_CSS,
    virtualize::{VirtualList, VirtualView},
};

#[test]
fn separately_compiled_hidden_fragments_report_both_identity_owners() -> crate::Result<()> {
    let skeleton = "<ui id='root'><panel id='titlebar' /><panel id='options' /></ui>";
    let mut ui = Ui::editor(skeleton, EDITOR_CSS, [640., 480.])?;
    ui.reconcile_parts(
        skeleton,
        &[
            (
                "titlebar",
                "<panel id='titlebar'><button id='play-mode' text='Play' /></panel>",
            ),
            (
                "options",
                "<panel id='options' style='display:none'><dropdown id='play-mode' /></panel>",
            ),
        ],
    )?;
    let findings = audit_attached_ids(&ui);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].code, "ARA4005");
    assert_eq!(findings[0].node, "play-mode");
    assert!(findings[0].message.contains("parent 'titlebar'"));
    assert!(findings[0].message.contains("parent 'options'"));
    ui.reconcile_parts(skeleton, &[
        ("titlebar", "<panel id='titlebar'><button id='play-mode' text='Play' /></panel>"),
        ("options", "<panel id='options' style='display:none'><dropdown id='play-options-mode' /></panel>"),
    ])?;
    assert!(audit_attached_ids(&ui).is_empty());
    Ok(())
}

#[test]
fn detached_virtual_pool_nodes_are_excluded_from_the_mounted_tree() -> crate::Result<()> {
    let mut ui = Ui::editor(
        "<ui id='root'><text id='rows/pool-1' text='Attached caption' /><scroll id='rows' /></ui>",
        EDITOR_CSS,
        [640., 480.],
    )?;
    let rows = ui.find("rows").expect("scroll");
    let mut view = VirtualView::mount(&mut ui, rows, 24., 96., vec![240.])?;
    // Mount allocates the pool; scrolling the empty model detaches its unused
    // slots while retaining their storage and parent metadata for later reuse.
    assert_eq!(audit_attached_ids(&ui).len(), 1);
    view.scroll(&mut ui, &VirtualList { items: Vec::new() }, 0., 96.)?;
    assert!(
        ui.nodes
            .iter()
            .filter(|node| node.element.id == "rows/pool-1")
            .count()
            > 1
    );
    assert!(audit_attached_ids(&ui).is_empty());
    let other_surface = Ui::editor(
        "<ui id='root'><button id='rows/pool-1' /></ui>",
        EDITOR_CSS,
        [320., 200.],
    )?;
    assert!(
        audit_attached_ids(&other_surface).is_empty(),
        "IDs are scoped to each surface"
    );
    Ok(())
}
