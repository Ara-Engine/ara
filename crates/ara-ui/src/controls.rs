//! Shared control-choice rules and markup checks for native applications.
use crate::source::Element;

/// What an exclusive choice changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// A stored enum value, such as an importer or quality setting.
    Enum,
    /// A small set of immediate, mutually exclusive interaction modes.
    Mode,
    /// The visible page or view.
    View,
}

/// The control appropriate for an exclusive choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// An anchored select menu.
    Select,
    /// Two to four short, exclusive options.
    Segmented,
    /// A page or view switcher.
    Tabs,
}

/// Choose a control before generating markup. Labels are measured in characters,
/// so the rule also works for localized strings rather than counting UTF-8 bytes.
pub fn choice(purpose: Purpose, labels: &[impl AsRef<str>]) -> Choice {
    if purpose == Purpose::View {
        return Choice::Tabs;
    }
    let limit = if purpose == Purpose::Enum { 3 } else { 4 };
    if (2..=limit).contains(&labels.len())
        && labels
            .iter()
            .all(|label| label.as_ref().chars().count() <= 12)
        && labels
            .iter()
            .map(|label| label.as_ref().chars().count())
            .sum::<usize>()
            <= 30
    {
        Choice::Segmented
    } else {
        Choice::Select
    }
}

/// A control whose markup violates the kit's choice rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Stable authored identity of the control.
    pub id: String,
    /// Actionable explanation for the application author.
    pub reason: &'static str,
}

fn group_choices<'a>(elements: &'a [Element], group: &Element) -> (Vec<&'a str>, bool) {
    let mut labels = Vec::new();
    let mut select = false;
    let mut seen = std::collections::BTreeSet::new();
    let mut pending: Vec<_> = group.children.iter().rev().copied().collect();
    while let Some(index) = pending.pop() {
        if !seen.insert(index) {
            continue;
        }
        let Some(child) = elements.get(index) else {
            continue;
        };
        match child.tag.as_str() {
            "button" => labels.push(
                child
                    .attrs
                    .get("label")
                    .or_else(|| child.attrs.get("text"))
                    .map(String::as_str)
                    .unwrap_or(""),
            ),
            "dropdown" => select = true,
            "panel" | "scroll" => pending.extend(child.children.iter().rev().copied()),
            _ => {}
        }
    }
    (labels, select)
}

/// Check compiled application markup. Declare button-based data groups with
/// `name='enum'`, regardless of CSS class. Unannotated segments are modes; legacy
/// type chips are data filters. Ordinary rows of independent actions stay valid.
pub fn audit(elements: &[Element]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for element in elements {
        let classes = element.attrs.get("class").map(String::as_str).unwrap_or("");
        let purpose = element.attrs.get("name").map(String::as_str);
        let segment = classes
            .split_whitespace()
            .any(|class| class == "segment" || class.ends_with("-segment"));
        let legacy_filter = classes
            .split_whitespace()
            .any(|class| class == "type-chips");
        let declared = element.tag == "panel" && matches!(purpose, Some("enum" | "mode"));
        if segment || legacy_filter || declared {
            let (labels, select) = group_choices(elements, element);
            let kind = if purpose == Some("enum") || legacy_filter {
                Purpose::Enum
            } else {
                Purpose::Mode
            };
            if (segment || legacy_filter || !select || !labels.is_empty())
                && choice(kind, &labels) != Choice::Segmented
            {
                violations.push(Violation { id: element.id.clone(), reason: "Use a select for many options or long labels; segments need 2–4 short exclusive modes (at most 3 enum values)." });
            }
        }
        if element.tag == "tabs" && purpose == Some("enum") {
            violations.push(Violation {
                id: element.id.clone(),
                reason: "Tabs switch pages or views; use a select for enum values.",
            });
        }
    }
    violations
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn long_labels_and_large_enums_use_selects() {
        assert_eq!(
            choice(Purpose::Enum, &["Low", "Medium", "High"]),
            Choice::Segmented
        );
        assert_eq!(
            choice(Purpose::Enum, &["Low", "Medium", "High", "Epic"]),
            Choice::Select
        );
        assert_eq!(
            choice(Purpose::Mode, &["Move", "Rotate", "Scale", "Rect"]),
            Choice::Segmented
        );
        assert_eq!(
            choice(Purpose::Enum, &["Small", "A much longer label"]),
            Choice::Select
        );
        assert_eq!(choice(Purpose::View, &["Scene", "Game"]), Choice::Tabs);
    }
    #[test]
    fn markup_audit_rejects_enum_tabs_and_oversized_segments() {
        let elements = crate::source::parse("<ui id='ui'><tabs id='quality' name='enum'/><panel id='mode' class='segment'><button id='a' label='A very long label'/><button id='b' label='B'/></panel></ui>").unwrap();
        assert_eq!(audit(&elements).len(), 2);
    }

    #[test]
    fn enum_groups_are_checked_through_layout_wrappers_without_relying_on_button_css() {
        let elements=crate::source::parse("<ui id='root'><panel id='data' name='enum' class='unrelated'><panel id='layout'><button id='a' text='A'/><button id='b' text='B'/><button id='c' text='C'/><button id='d' text='D'/></panel></panel><panel id='legacy' class='type-chips'><button id='e' label='All'/><button id='f' label='Models'/><button id='g' label='Textures'/><button id='h' label='Materials'/></panel><panel id='long' name='enum'><button id='i' text='A long localized data value'/><button id='j' text='Short'/></panel><panel id='native' name='enum'><panel id='native-value'><dropdown id='select' text='Selected'/></panel></panel><panel id='actions'><button id='create' text='Create'/><button id='copy' text='Copy'/><button id='rename' text='Rename'/><button id='delete' text='Delete'/></panel></ui>").unwrap();
        assert_eq!(
            audit(&elements)
                .into_iter()
                .map(|violation| violation.id)
                .collect::<Vec<_>>(),
            ["data", "legacy", "long"]
        );
    }
}
