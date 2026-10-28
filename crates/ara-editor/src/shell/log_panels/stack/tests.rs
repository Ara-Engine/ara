#![allow(clippy::unwrap_used)] // Isolated fixtures and retained interaction assertions.
use super::*;
use ara_ui::{input::InputEvent, source::stable_id};
#[path = "tests/fixture.rs"]
mod fixture;
use fixture::Fixture;

fn frame(method: &str, file: Option<&str>, line: Option<u32>, column: Option<u32>) -> Value {
    json!({"method":method,"file":file,"line":line,"column":column})
}

fn exception(kind: &str, frames: Vec<Value>, inners: Vec<Value>) -> Value {
    json!({"schema":1,"type":kind,"message":format!("{kind} message"),"frames":frames,"inners":inners,"truncated":false})
}

#[test]
fn borrowed_projection_bounds_total_frames_nodes_depth_and_utf8_strings() {
    let long = "界".repeat(MAX_STRING);
    let many = exception(
        &long,
        (0..80)
            .map(|_| frame(&long, Some(&long), Some(1), Some(1)))
            .collect(),
        vec![],
    );
    let stack = Stack::parse(&many);
    assert_eq!(stack.frames, MAX_FRAMES);
    assert_eq!(stack.nodes.len(), 1);
    assert!(stack.truncated);
    assert!(stack.nodes[0].kind.len() <= MAX_STRING);
    assert!(
        stack.nodes[0]
            .frames
            .iter()
            .all(|row| row.method.len() <= MAX_STRING && !row.link)
    );
    assert!(std::str::from_utf8(stack.nodes[0].kind.as_bytes()).is_ok());

    let mut deep = exception("Leaf", vec![], vec![]);
    for _ in 0..20 {
        deep = exception("Outer", vec![], vec![deep]);
    }
    let stack = Stack::parse(&deep);
    assert_eq!(stack.nodes.len(), MAX_DEPTH);
    assert!(stack.truncated);

    let wide = exception(
        "Aggregate",
        vec![],
        (0..40)
            .map(|_| exception("Inner", vec![], vec![]))
            .collect(),
    );
    let stack = Stack::parse(&wide);
    assert_eq!(stack.nodes.len(), MAX_NODES);
    assert!(stack.truncated);
    let invalid = exception("Aggregate", vec![], vec![Value::Null; 1000]);
    let stack = Stack::parse(&invalid);
    assert_eq!(
        stack.visited, MAX_NODES,
        "invalid children consume the defensive node budget too"
    );
    assert!(stack.malformed && stack.truncated);
}

#[test]
fn malformed_metadata_never_invents_a_source_location() {
    let value = exception(
        "Failure",
        vec![
            frame("Runtime.WithoutSymbols", None, None, None),
            json!({"method":"Bad coordinates","file":"content/Main.cs","line":-2,"column":1}),
            frame("Pseudo source", Some("<dynamic>"), Some(4), Some(1)),
            frame(
                "URI source",
                Some("https://example.invalid/source.cs"),
                Some(4),
                Some(1),
            ),
            frame(
                "File without coordinates",
                Some("content/Main.cs"),
                None,
                None,
            ),
        ],
        vec![json!({"schema":999,"type":"Unsupported"})],
    );
    let stack = Stack::parse(&value);
    assert!(stack.malformed);
    assert_eq!(stack.frames, 5);
    assert_eq!(
        stack.nodes[0]
            .frames
            .iter()
            .map(|row| row.link)
            .collect::<Vec<_>>(),
        [false, false, false, false, true]
    );
    assert_eq!(stack.nodes[0].frames[4].line, None);
    assert_eq!(stack.nodes[0].frames[4].column, None);
    let unsupported = json!({"schema":2});
    let stack = Stack::parse(&unsupported);
    assert!(stack.nodes.is_empty() && stack.malformed);
}

#[test]
fn console_long_stack_scrolls_and_each_source_keeps_its_own_ide_coordinates() {
    let mut f = Fixture::new("console");
    let source = f.source(false);
    let inner = f.source(true);
    let rows: Vec<_> = (0..40)
        .map(|index| {
            frame(
                &format!("Gameplay.Controller.Frame{index}()"),
                Some(&source),
                Some(20 + index),
                Some(3 + index),
            )
        })
        .collect();
    let value = exception(
        "OuterFailure",
        rows,
        vec![exception(
            "InnerFailure",
            vec![
                frame("Gameplay.Inner.Throw()", Some(&inner), Some(91), Some(7)),
                frame("System.Runtime.Invoke()", None, None, None),
            ],
            vec![],
        )],
    );
    let revision = f.editor.host.store().revision();
    let (seq, original) = f.log(value);
    f.click(&format!("log-{seq}-row"));
    let scroll = format!("log-{seq}-stack-scroll");
    let root = f.shell.shown_node(&scroll).unwrap();
    assert!(f.shell.ui.nodes[root].rect.h > 0. && f.shell.ui.nodes[root].rect.h <= 260.1);
    assert!(f.shell.ui.scroll_extent(root).unwrap()[1] > f.shell.ui.nodes[root].rect.h * 3.);
    assert_eq!(f.shell.ui.scroll_position(root).unwrap()[1], 0.);
    for index in 0..40 {
        let key = stable_id(&format!("log-{seq}-stack-frame-{index}-source"));
        let Binding::Diagnostic(diagnostic) = &f.shell.bindings[&key] else {
            panic!("source frame binding");
        };
        assert_eq!(diagnostic.location.file.as_deref(), Some(source.as_str()));
        assert_eq!(diagnostic.location.line, Some(20 + index));
        assert_eq!(diagnostic.location.column, Some(3 + index));
        assert!(
            diagnostic.data.is_none(),
            "a link must not clone the exception tree"
        );
    }
    f.click(&format!("log-{seq}-stack-frame-0-source"));
    f.assert_request(&source, 20, 3);
    f.wheel(&scroll, 100_000.);
    let root = f.shell.shown_node(&scroll).unwrap();
    assert!(f.shell.ui.scroll_position(root).unwrap()[1] > 0.);
    f.click(&format!("log-{seq}-stack-frame-40-source"));
    f.assert_request(&inner, 91, 7);
    assert!(
        f.shell
            .shown_node(&format!("log-{seq}-stack-frame-41-method"))
            .is_some()
    );
    let cause = f.shell.ui.find(&format!("log-{seq}-stack-type-1")).unwrap();
    assert!(f.shell.ui.nodes[cause].element.attrs["text"].contains("Caused by: InnerFailure"));
    assert_eq!(f.editor.host.store().revision(), revision);
    let logs = f
        .editor
        .action("logs", json!({"group":false,"limit":500,"severity":"info"}))
        .unwrap();
    let retained = logs["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["seq"] == seq)
        .unwrap();
    assert_eq!(
        retained["diagnostic"],
        json!(original),
        "stack expansion and source clicks preserve the original bus row and scope"
    );
}

#[test]
fn problems_expands_inner_causes_and_malformed_stack_details_remain_usable() {
    let mut f = Fixture::new("problems");
    let source = f.source(true);
    let value = exception(
        "OuterFailure",
        vec![frame("Runtime.Invoke()", None, None, None)],
        vec![
            exception(
                "InnerFailure",
                vec![frame("Game.Inner()", Some(&source), Some(13), Some(11))],
                vec![],
            ),
            json!({"schema":99}),
        ],
    );
    let (seq, _) = f.log(value);
    f.click(&format!("problem-{seq}-why"));
    assert!(
        f.shell
            .shown_node(&format!("problem-{seq}-stack-malformed"))
            .is_some()
    );
    assert!(
        f.shell
            .shown_node(&format!("problem-{seq}-stack-frame-0-method"))
            .is_some()
    );
    assert!(
        !f.shell
            .bindings
            .contains_key(&stable_id(&format!("problem-{seq}-stack-frame-0-source")))
    );
    f.click(&format!("problem-{seq}-stack-frame-1-source"));
    f.assert_request(&source, 13, 11);
}
