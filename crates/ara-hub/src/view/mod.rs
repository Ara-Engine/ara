//! Markup for every Hub page, generated from the view-model on each change.
//! Controls carry stable IDs; [`Page::bindings`] maps each one to the [`Intent`]
//! it raises, so the window only routes `ara_ui` actions back to the model.
mod agents;
mod create;
mod dialogs;
mod installs;
mod learn;
mod projects;
mod settings;
mod shell;

use crate::model::{Hub, Intent};
use ara_ui::select::{Select, SelectOption};
use ara_ui::source::{escape, stable_id};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    path::{Path, PathBuf},
};

/// The stylesheet; colors are theme tokens only (D18).
pub const CSS: &str = include_str!("hub.css");

/// Hub surfaces include the shared choice styles and let long option rows grow.
pub fn stylesheet() -> &'static str {
    static SHEET: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SHEET.get_or_init(|| {
        format!(
            "{CSS}{}\n.kit-select-option {{height:auto;min-height:32px;flex-shrink:0;}}",
            ara_ui::select::CSS
        )
    })
}

/// Fonts shared with the editor: Inter, JetBrains Mono and Arabic fallback.
pub fn text_system() -> ara_ui::Result<ara_ui_text::TextSystem> {
    ara_ui_text::TextSystem::new(
        [
            include_bytes!("../../../ara-editor/assets/fonts/InterVariable.ttf").to_vec(),
            include_bytes!("../../../ara-editor/assets/fonts/JetBrainsMono-Regular.ttf").to_vec(),
            include_bytes!("../../../ara-ui-text/fonts/NotoNaskhArabic.ttf").to_vec(),
        ],
        "'Inter Variable', 'Noto Naskh Arabic'",
    )
}

/// What a control does when used.
#[derive(Clone, Debug)]
pub enum Binding {
    /// Raise this intent on activation.
    Intent(Intent),
    /// A text field; its committed value becomes this intent.
    Field(fn(String) -> Intent),
}

/// A Learn link: title, caption and the local file it opens.
#[derive(Clone, Debug)]
pub struct Doc {
    /// Link title.
    pub title: String,
    /// One-line caption.
    pub caption: String,
    /// Local file.
    pub path: PathBuf,
}

/// Inputs the views need besides the model.
pub struct Ctx<'a> {
    /// Show the close button's non-interactive hint after native hover hit-testing.
    pub dialog_close_hint: bool,
    /// Wall clock for relative times (fixed in captures).
    pub now_ms: u64,
    /// Logical window size.
    pub size: [f32; 2],
    /// `src` of a loaded preview image, if the window has it.
    pub thumbnail: &'a dyn Fn(&Path) -> Option<String>,
    /// Screen rect of the open menu's button, from the previous layout.
    pub anchor: Option<[f32; 4]>,
    /// Learn links that exist on this machine.
    pub docs: &'a [Doc],
}

/// Generated markup plus its control bindings and wanted preview images.
#[derive(Default)]
pub struct Page {
    /// `ara_ui` markup.
    pub markup: String,
    /// Stable control identity → behavior.
    pub bindings: BTreeMap<u64, Binding>,
    /// Preview images referenced but not loaded yet.
    pub wanted: Vec<PathBuf>,
    next: u32,
    scope: String,
    pub(crate) choices: Vec<Choice>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct Choice {
    pub control: String,
    pub label: String,
    pub options: Vec<(String, Intent)>,
    pub selected: Option<Intent>,
    pub markup: String,
}

impl Choice {
    pub(crate) fn kit_options(&self) -> Vec<SelectOption<Intent>> {
        self.options
            .iter()
            .map(|(label, value)| SelectOption::new(value.clone(), label.clone()))
            .collect()
    }
    pub(crate) fn select(&self) -> Select<Intent> {
        Select::new(
            format!("hub-select-{}", self.control),
            self.label.clone(),
            self.kit_options(),
            self.selected.clone(),
        )
        .with_control_id(self.control.clone())
    }
}

impl Page {
    /// An enum selector whose options open in a root-level kit popup.
    pub fn dropdown(
        &mut self,
        id: &str,
        label: &str,
        options: Vec<(String, Intent)>,
        selected: Option<Intent>,
    ) {
        let mut choice = Choice {
            control: id.into(),
            label: label.into(),
            options,
            selected,
            markup: String::new(),
        };
        choice.markup = choice.select().control_markup();
        self.markup.push_str(&choice.markup);
        self.choices.push(choice);
    }
    fn auto(&mut self) -> String {
        self.next += 1;
        format!("node-{:x}-{}", stable_id(&self.scope), self.next)
    }
    /// Keep generated children stable when an unrelated section is inserted.
    fn scoped(&mut self, key: &str, body: impl FnOnce(&mut Self)) {
        let scope = format!("{}/{key}", self.scope);
        let previous = std::mem::replace(&mut self.scope, scope);
        let next = std::mem::take(&mut self.next);
        body(self);
        self.scope = previous;
        self.next = next;
    }
    /// A keyed collection item; identity follows the item when sorted or filtered.
    pub fn panel_key(&mut self, key: &str, class: &str, body: impl FnOnce(&mut Self)) {
        self.scoped(key, |p| p.panel(class, body));
    }
    fn open(&mut self, tag: &str, class: &str, style: &str) {
        let id = self.auto();
        let _ = write!(self.markup, "<{tag} id='{id}' class='{class}'");
        if !style.is_empty() {
            let _ = write!(self.markup, " style='{style}'");
        }
        self.markup.push('>');
    }
    fn close(&mut self, tag: &str) {
        let _ = write!(self.markup, "</{tag}>");
    }
    /// A container.
    pub fn panel(&mut self, class: &str, body: impl FnOnce(&mut Self)) {
        self.panel_styled(class, "", body);
    }
    /// A container with inline layout (sizes only; never colors).
    pub fn panel_styled(&mut self, class: &str, style: &str, body: impl FnOnce(&mut Self)) {
        self.open("panel", class, style);
        body(self);
        self.close("panel");
    }
    /// A leaf panel (spacer, dot, divider).
    pub fn leaf(&mut self, class: &str) {
        let id = self.auto();
        let _ = write!(self.markup, "<panel id='{id}' class='{class}' />");
    }
    /// A label.
    pub fn text(&mut self, class: &str, text: &str) {
        let id = self.auto();
        let _ = write!(
            self.markup,
            "<text id='{id}' class='{class}' text='{}' />",
            escape(text, b'\'')
        );
    }
    /// A Lucide icon tinted by its `color`.
    pub fn icon(&mut self, name: &str, class: &str) {
        let id = self.auto();
        let _ = write!(
            self.markup,
            "<image id='{id}' class='icon {class}' src='icon-{name}' tint='true' />"
        );
    }
    /// An untinted image (logo, preview).
    pub fn image(&mut self, src: &str, class: &str) {
        let id = self.auto();
        let _ = write!(
            self.markup,
            "<image id='{id}' class='{class}' src='{}' />",
            escape(src, b'\'')
        );
    }
    /// A button raising `intent`; `None` renders it disabled.
    pub fn button(
        &mut self,
        id: &str,
        class: &str,
        label: &str,
        intent: Option<Intent>,
        body: impl FnOnce(&mut Self),
    ) {
        let disabled = if intent.is_some() {
            ""
        } else {
            " disabled='true'"
        };
        let _ = write!(
            self.markup,
            "<button id='{}' class='{class}' label='{}' action='Hub'{disabled}>",
            escape(id, b'\''),
            escape(label, b'\'')
        );
        if let Some(intent) = intent {
            self.bindings.insert(stable_id(id), Binding::Intent(intent));
        }
        self.scoped(id, body);
        self.close("button");
    }
    /// Icon + label button, the common case.
    pub fn action(
        &mut self,
        id: &str,
        class: &str,
        icon: &str,
        label: &str,
        intent: Option<Intent>,
    ) {
        self.button(id, class, label, intent, |p| {
            if !icon.is_empty() {
                p.icon(icon, "inherit");
            }
            if !label.is_empty() {
                p.text("", label);
            }
        });
    }
    /// A copy button; after use it reads "Copied" until the next navigation.
    pub fn copy(&mut self, copied: Option<&str>, id: &str, text: &str, icon_only: bool) {
        let (icon, label) = if copied == Some(text) {
            ("check", "Copied")
        } else {
            ("copy", "Copy")
        };
        let intent = Some(Intent::Copy(text.to_owned()));
        if icon_only {
            self.button(id, "ghost icon-compact", label, intent, |p| {
                p.icon(icon, "inherit");
            });
        } else {
            self.action(id, "compact", icon, label, intent);
        }
    }
    /// A single-line text field whose edits raise `on(value)`.
    pub fn field(&mut self, id: &str, class: &str, value: &str, on: fn(String) -> Intent) {
        let _ = write!(
            self.markup,
            "<field id='{}' class='{class}' text='{}' label='{}' action='Hub' />",
            escape(id, b'\''),
            escape(value, b'\''),
            escape(id, b'\'')
        );
        self.bindings.insert(stable_id(id), Binding::Field(on));
    }
    /// A preview image, or a tinted initials tile while it loads or when it is absent.
    pub fn thumbnail(&mut self, ctx: &Ctx, path: Option<&Path>, id: &str, name: &str, small: bool) {
        if let Some(path) = path {
            if let Some(src) = (ctx.thumbnail)(path) {
                self.image(&src, "thumb-image");
                return;
            }
            self.wanted.push(path.to_owned());
        }
        let tint = crate::model::text::tint(id);
        self.panel(&format!("tile tile-{tint}"), |p| {
            let class = if small { "tile-text-sm" } else { "tile-text" };
            p.text(class, &crate::model::text::initials(name));
        });
    }
}

/// The whole window for the current model.
pub fn page(hub: &Hub, ctx: &Ctx) -> Page {
    let mut page = Page::default();
    page.markup.push_str("<ui id='hub'>");
    if hub.view != crate::model::View::Create {
        page.scoped("sidebar", |p| shell::sidebar(p, hub));
        page.leaf("sidebar-edge");
    }
    page.panel("main", |p| {
        p.scoped("header", |p| shell::header(p, hub, ctx));
        let scroll = format!("scroll-{:?}", hub.view).to_lowercase();
        let _ = write!(
            p.markup,
            "<scroll id='{scroll}' class='content-scroll'><panel id='content' class='content'>"
        );
        p.scoped("offline", |p| shell::offline(p, hub));
        p.scoped("update", |p| shell::update(p, hub));
        p.scoped(&format!("{:?}", hub.view), |p| match hub.view {
            crate::model::View::Projects => projects::body(p, hub, ctx),
            crate::model::View::Create => create::body(p, hub, ctx),
            crate::model::View::Installs => installs::body(p, hub),
            crate::model::View::Agents => agents::body(p, hub),
            crate::model::View::Learn => learn::body(p, hub, ctx),
            crate::model::View::Settings => settings::body(p, hub),
        });
        p.markup.push_str("</panel></scroll>");
    });
    page.scoped("menu", |p| projects::menu(p, hub, ctx));
    page.scoped("toasts", |p| shell::toasts(p, hub));
    page.scoped("dialog", |p| dialogs::body(p, hub, ctx));
    page.markup.push_str("</ui>");
    page
}

/// Grid columns that fit `width` with cards at least `min` wide.
fn columns(width: f32, min: f32, gap: f32) -> usize {
    (((width + gap) / (min + gap)).floor() as usize).max(1)
}

/// `grid-template-columns` for `n` equal tracks.
fn tracks(n: usize) -> String {
    vec!["1fr"; n].join(" ")
}
