//! Semantic AccessKit projection; platform adapters are installed by the host.
/// Resolve the native semantic role, shared by AccessKit and offline inspection of hidden nodes.
pub fn role_for_tag(tag: &str) -> accesskit::Role {
    use accesskit::Role;
    match tag {
        "button" => Role::Button,
        "field" | "input" => Role::TextInput,
        "toggle" => Role::CheckBox,
        "slider" => Role::Slider,
        "progress" => Role::ProgressIndicator,
        "dropdown" => Role::ComboBox,
        "list" => Role::List,
        "splitter" => Role::Splitter,
        "scroll" => Role::ScrollView,
        "tooltip" => Role::Tooltip,
        "tabs" => Role::TabList,
        "tab" => Role::Tab,
        "menu" => Role::Menu,
        "modal" => Role::Dialog,
        "tree" => Role::Tree,
        "table" => Role::Table,
        "image" => Role::Image,
        "text" => Role::Label,
        _ => Role::GenericContainer,
    }
}
/// Produce actual AccessKit nodes rather than an approximation of its wire format.
pub fn accessibility(ui: &crate::Ui, focus: Option<u64>) -> accesskit::TreeUpdate {
    use crate::source::stable_id;
    use accesskit::{Action, Node, NodeId, Rect};
    use std::collections::HashSet;
    // Node IDs are hashes of element IDs. Markup that repeats an element ID would
    // list one node twice, and accessibility clients reject the whole tree (a Windows
    // UI Automation client panics in `accesskit_consumer`). Walk from the root and
    // keep each ID's first occurrence; a repeat is dropped with its subtree.
    let painted: HashSet<usize> = ui.paint_order.iter().copied().collect();
    let mut kept: HashSet<usize> = HashSet::new();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut stack = vec![0];
    while let Some(index) = stack.pop() {
        if !painted.contains(&index) || !seen.insert(stable_id(&ui.nodes[index].element.id)) {
            continue;
        }
        kept.insert(index);
        stack.extend(ui.nodes[index].element.children.iter().rev());
    }
    let nodes = ui
        .paint_order
        .iter()
        .filter(|index| kept.contains(index))
        .map(|index| {
            let n = &ui.nodes[*index];
            let role = role_for_tag(&n.element.tag);
            let mut node = Node::new(role);
            node.set_children(
                n.element
                    .children
                    .iter()
                    .filter(|i| kept.contains(i))
                    .map(|i| NodeId(stable_id(&ui.nodes[*i].element.id)))
                    .collect::<Vec<_>>(),
            );
            node.set_label(
                n.element
                    .attrs
                    .get("label")
                    .map(String::as_str)
                    .unwrap_or_else(|| {
                        n.text
                            .as_ref()
                            .map(|t| t.value.as_str())
                            .unwrap_or(&n.element.id)
                    }),
            );
            if matches!(n.element.tag.as_str(), "field" | "input") {
                if let Some(crate::widgets::Widget::Input(input)) =
                    ui.widgets.get(&stable_id(&n.element.id))
                {
                    node.set_value(input.value());
                }

                node.add_action(Action::SetValue);
            }
            if matches!(
                n.element.tag.as_str(),
                "field" | "input" | "button" | "toggle" | "slider" | "tab" | "dropdown"
            ) {
                node.add_action(Action::Focus);
            }
            if n.element.tag == "button" {
                node.add_action(Action::Click);
            }
            match ui.widgets.get(&stable_id(&n.element.id)) {
                Some(crate::widgets::Widget::Toggle(value)) => {
                    node.set_toggled(if *value {
                        accesskit::Toggled::True
                    } else {
                        accesskit::Toggled::False
                    });
                    node.add_action(Action::Click);
                }
                Some(crate::widgets::Widget::Slider {
                    value,
                    min,
                    max,
                    step,
                }) => {
                    node.set_numeric_value(*value);
                    node.set_min_numeric_value(*min);
                    node.set_max_numeric_value(*max);
                    node.set_numeric_value_step(*step);
                    node.add_action(Action::SetValue);
                }
                Some(crate::widgets::Widget::Scroll(p)) => {
                    node.set_scroll_x(f64::from(p[0]));
                    node.set_scroll_y(f64::from(p[1]));
                }
                _ => {}
            }
            if n.state.disabled {
                node.set_disabled();
            }
            if let Some(description) = n.element.attrs.get("description") {
                node.set_description(description.as_str());
            }
            node.set_bounds(Rect {
                x0: f64::from(n.rect.x),
                y0: f64::from(n.rect.y),
                x1: f64::from(n.rect.x + n.rect.w),
                y1: f64::from(n.rect.y + n.rect.h),
            });
            (NodeId(stable_id(&n.element.id)), node)
        })
        .collect();
    accesskit::TreeUpdate {
        nodes,
        tree: Some(accesskit::TreeInfo::new(NodeId(stable_id(
            &ui.nodes[0].element.id,
        )))),
        tree_id: accesskit::TreeId::ROOT,
        focus: NodeId(
            focus
                .filter(|id| ui.by_id(*id).is_some_and(|i| kept.contains(&i)))
                .unwrap_or_else(|| stable_id(&ui.nodes[0].element.id)),
        ),
    }
}
