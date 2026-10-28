//! Generated panel checks share reconciliation's compiler without touching live state.
use super::*;

#[test]
fn markup_validation_matches_reconcile_without_changing_retained_controls() -> Result<()> {
    let mut ui = Ui::editor(
        "<ui id='root'><field id='draft' text='Keep this draft' /><scroll id='list' /></ui>",
        crate::theme::EDITOR_CSS,
        [600., 400.],
    )?;
    let mut text = ara_ui_text::TextSystem::bundled()?;
    ui.frame(&mut text)?;
    let before = ui.inspect();
    let draft = ui.find("draft").ok_or("Draft control missing")?;
    let input = match ui.widgets.get(&stable_id("draft")) {
        Some(Widget::Input(input)) => input.value().to_owned(),
        _ => return Err("Draft input state missing".into()),
    };
    for markup in [
        "<panel id='bad' style='align-self:stretch' />",
        "<panel id='bad' style='color:#123456' />",
        "<panel id='bad' style='color:var(--missing-panel-token)' />",
        "<panel id='bad'><text id='bad' /></panel>",
        "<panel id='bad'>",
        "<slider id='bad' value='NaN' />",
        "<dropdown id='bad' selected='NaN' />",
    ] {
        let validation = ui
            .validate_markup(markup)
            .err()
            .ok_or("Invalid generated markup was accepted")?;
        let reconciliation = ui
            .reconcile(markup)
            .err()
            .ok_or("Reconcile accepted invalid generated markup")?;
        assert_eq!(
            validation.to_string(),
            reconciliation.to_string(),
            "{markup}"
        );
        assert_eq!(
            ui.inspect(),
            before,
            "Validation/rejection changed the retained tree"
        );
        assert_eq!(ui.find("draft"), Some(draft));
        assert!(
            matches!(ui.widgets.get(&stable_id("draft")),Some(Widget::Input(value)) if value.value()==input)
        );
    }
    ui.validate_markup(
        "<panel id='fresh'><slider id='amount' min='0' max='10' value='3' /></panel>",
    )?;
    assert!(
        ui.find("fresh").is_none(),
        "Validation must not mount the prospective panel"
    );
    assert_eq!(ui.inspect(), before);
    assert_eq!(
        ui.frame(&mut text)?,
        crate::Work::default(),
        "Validation dirtied live layout/text/paint"
    );
    Ok(())
}
