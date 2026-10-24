use ara_ui_document::{Command, DocumentKind, SourceDocument, compile, stylesheet::Stylesheet};
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
fn roundtrip(input: &str, kind: DocumentKind, command: Command) -> Result<String> {
    let mut doc = SourceDocument::parse(input, kind)?;
    assert_eq!(doc.serialize().as_bytes(), input.as_bytes());
    let op = doc.command(0, command)?;
    doc.apply(&op)?;
    let edited = doc.serialize().to_owned();
    assert!(doc.apply(&op).is_err());
    let inverse = op.inverse(doc.revision());
    doc.apply(&inverse)?;
    assert_eq!(doc.serialize().as_bytes(), input.as_bytes());
    Ok(edited)
}
#[test]
fn byte_exact_attribute_and_structure_edits() -> Result {
    let source = "<!-- العربية -->\r\n<panel id='root'>\r\n  <button  id = 'a' text='Go &amp; play' />\r\n  <!-- keep --> <panel id='b'></panel>\r\n</panel>\r\n";
    let edited = roundtrip(
        source,
        DocumentKind::Markup,
        Command::SetAttribute {
            id: "a".into(),
            name: "text".into(),
            value: Some("Let's \"go\" & play".into()),
        },
    )?;
    assert_eq!(
        edited,
        source.replace("Go &amp; play", "Let&apos;s \"go\" &amp; play")
    );
    let edited = roundtrip(
        source,
        DocumentKind::Markup,
        Command::Move {
            id: "a".into(),
            parent: "b".into(),
            before: None,
        },
    )?;
    assert_eq!(
        edited,
        source
            .replace("<button  id = 'a' text='Go &amp; play' />", "")
            .replace(
                "<panel id='b'></panel>",
                "<panel id='b'><button  id = 'a' text='Go &amp; play' /></panel>"
            )
    );
    let edited = roundtrip(
        source,
        DocumentKind::Markup,
        Command::Remove { id: "a".into() },
    )?;
    assert_eq!(
        edited,
        source.replace("<button  id = 'a' text='Go &amp; play' />", "")
    );
    let edited = roundtrip(
        source,
        DocumentKind::Markup,
        Command::Insert {
            parent: "b".into(),
            before: None,
            source: "<text id='new' text='日本' />".into(),
        },
    )?;
    assert_eq!(
        edited,
        source.replace(
            "</panel>\r\n</panel>",
            "<text id='new' text='日本' /></panel>\r\n</panel>"
        )
    );
    let source = "<panel id='root'><panel id='empty' /></panel>";
    roundtrip(
        source,
        DocumentKind::Markup,
        Command::Insert {
            parent: "empty".into(),
            before: None,
            source: "<text id='new' text='New' />".into(),
        },
    )?;
    roundtrip(
        source,
        DocumentKind::Markup,
        Command::SetAttribute {
            id: "empty".into(),
            name: "label".into(),
            value: Some("Panel".into()),
        },
    )?;
    Ok(())
}
#[test]
fn css_edits_touch_only_the_value_and_preserve_comments() -> Result {
    let css = "/* ع */\r\n:root { --accent: #346bf1; }\r\nbutton:hover { color : var(--accent) ; height:30px }\r\n";
    let edited = roundtrip(
        css,
        DocumentKind::Stylesheet,
        Command::SetStyle {
            rule: 1,
            property: "height".into(),
            value: "32px".into(),
        },
    )?;
    assert_eq!(edited, css.replace("30px", "32px"));
    roundtrip(
        css,
        DocumentKind::Stylesheet,
        Command::SetStyle {
            rule: 1,
            property: "gap".into(),
            value: "4px".into(),
        },
    )?;
    Ok(())
}
#[test]
fn atomic_conflicts_and_cycles_leave_source_untouched() -> Result {
    let input = "<panel id='root'><panel id='a'><text id='b' /></panel></panel>";
    let doc = SourceDocument::parse(input, DocumentKind::Markup)?;
    assert!(doc.command(1, Command::Remove { id: "a".into() }).is_err());
    assert!(
        doc.command(
            0,
            Command::Move {
                id: "a".into(),
                parent: "b".into(),
                before: None
            }
        )
        .is_err()
    );
    assert!(
        doc.command(
            0,
            Command::Insert {
                parent: "root".into(),
                before: None,
                source: "<text id='b' />".into()
            }
        )
        .is_err()
    );
    assert_eq!(doc.serialize(), input);
    Ok(())
}
#[test]
fn components_slots_keep_instance_identity() -> Result {
    let ui = "<ui id='root'><template id='card'><panel id='surface'><text id='title' text='Shared' /><slot id='content' name='body' /></panel></template><instance id='left' template='card'><button id='ok' slot='body' text='OK' /></instance><instance id='right' template='card'><text id='details' slot='body' text='Details' /></instance></ui>";
    let e = compile(ui)?;
    assert_eq!(e.len(), 7);
    assert_eq!(e[1].id, "left/surface");
    assert_eq!(e[3].id, "left/ok");
    assert_eq!(e[4].id, "right/surface");
    assert_eq!(e[6].id, "right/details");
    assert!(compile("<ui id='r'><template id='loop'><instance id='again' template='loop' /></template><instance id='start' template='loop' /></ui>").is_err());
    Ok(())
}
#[test]
fn unsupported_syntax_has_stable_diagnostics_and_fixes() {
    for css in [
        "button > text {gap:4px}",
        "button:visited {color:#000000}",
        ".unused {filter:blur(2px)}",
        ":root{--a:var(--b);--b:var(--a)}",
    ] {
        let e = Stylesheet::parse(css).expect_err("unsupported source must be rejected");
        assert!(e.code.starts_with("ARA40"));
        assert!(!e.fix.is_empty());
        assert!(e.line >= 1 && e.column >= 1);
    }
}
