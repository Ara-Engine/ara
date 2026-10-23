//! Markup for every Hub page, generated from the view-model on each change.
//! Controls carry stable IDs; [`Page::bindings`] maps each one to the [`Intent`]
//! it raises, so the window only routes `ara_ui` actions back to the model.
mod agents;
mod create;
mod installs;
mod learn;
mod projects;
mod shell;

use crate::model::{Hub, Intent};
use ara_ui::source::{escape, stable_id};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    path::{Path, PathBuf},
};

/// The stylesheet; colors are theme tokens only (D18).
pub const CSS: &str = include_str!("hub.css");

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
}

impl Page {
    fn auto(&mut self) -> String {
        self.next += 1;
        format!("n{}", self.next)
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
        body(self);
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
    shell::sidebar(&mut page, hub);
    page.leaf("sidebar-edge");
    page.panel("main", |p| {
        shell::header(p, hub, ctx);
        let scroll = format!("scroll-{:?}", hub.view).to_lowercase();
        let _ = write!(
            p.markup,
            "<scroll id='{scroll}' class='content-scroll'><panel id='content' class='content'>"
        );
        shell::offline(p, hub);
        match hub.view {
            crate::model::View::Projects => projects::body(p, hub, ctx),
            crate::model::View::Create => create::body(p, hub, ctx),
            crate::model::View::Installs => installs::body(p, hub),
            crate::model::View::Agents => agents::body(p, hub),
            crate::model::View::Learn => learn::body(p, hub, ctx),
        }
        p.markup.push_str("</panel></scroll>");
    });
    projects::menu(&mut page, hub, ctx);
    shell::toasts(&mut page, hub);
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
