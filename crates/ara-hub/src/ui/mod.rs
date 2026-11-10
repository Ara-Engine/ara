//! The Hub's interface on stillkit (D34). [`HubApp`] is the runtime's state: the view-model
//! ([`Hub`], changed only by intents and service events) plus what only the window needs
//! (the look, images, the palette). [`view`] builds the element tree from it; handlers call
//! [`HubApp::intent`], and the host drains the model's calls and effects after each frame.
mod agents;
mod create;
mod dialogs;
pub mod images;
mod installs;
mod learn;
mod markdown;
mod palette;
mod projects;
mod settings;
mod shell;

use ara_kit::look::Look;
use stillkit::el::{El, Handled, KeyCode, col, row};

use crate::model::{Hub, Intent, View};
pub use images::Images;
pub use palette::Palette;

/// What the window adds around the drawn content: where the system's window controls sit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    /// The macOS traffic lights' right edge, design px (0 elsewhere).
    pub inset: f32,
    /// Drawn minimize/maximize/close, and whether maximized (`None` on macOS).
    pub controls: Option<bool>,
}

/// The Hub runtime's state.
pub struct HubApp {
    /// The view-model.
    pub hub: Hub,
    /// The current scheme's tokens and theme.
    pub look: Look,
    /// Wall-clock milliseconds, for relative times ("2 h ago"); the host ticks it.
    pub now_ms: u64,
    /// Decoded pictures and generated covers.
    pub images: Images,
    /// The window's own controls.
    pub frame: Frame,
    /// The window's width in design px (grid columns).
    pub width: f32,
    /// The command palette (⌘K).
    pub palette: Palette,
    /// The dropdown open now, by its trigger's key.
    pub select: Option<String>,
    /// Agent clients' marks, rasterized for the current scheme.
    pub logos:
        std::collections::BTreeMap<crate::model::AgentClient, std::sync::Arc<stillkit::Image>>,
}

impl HubApp {
    /// A Hub window's state over `hub`.
    pub fn new(hub: Hub, width: f32) -> Self {
        let look = Look::new(!hub.prefs.light);
        let logos = logos(&look);
        Self {
            hub,
            look,
            now_ms: 0,
            images: Images::default(),
            frame: Frame::default(),
            width,
            palette: Palette::default(),
            select: None,
            logos,
        }
    }

    /// Opens the dropdown `key`, or closes it if it is the open one.
    pub fn toggle_select(&mut self, key: &str) {
        self.select = if self.select.as_deref() == Some(key) {
            None
        } else {
            Some(key.to_owned())
        };
    }

    /// Runs a person's intent on the model.
    pub fn intent(&mut self, intent: Intent) {
        self.hub.intent(intent);
    }

    /// The look follows the model's theme preference (the host swaps the runtime theme).
    pub fn sync_look(&mut self) -> bool {
        let dark = !self.hub.prefs.light;
        if self.look.dark == dark {
            return false;
        }
        self.look = Look::new(dark);
        self.logos = logos(&self.look);
        true
    }
}

/// Each agent client's mark at 2x a 20 px slot: Claude in its brand colour, the others in
/// the scheme's ink (their SVGs paint `currentColor`).
fn logos(
    look: &Look,
) -> std::collections::BTreeMap<crate::model::AgentClient, std::sync::Arc<stillkit::Image>> {
    let ink = look.c.foreground.0;
    let hex = format!(
        "#{:02x}{:02x}{:02x}",
        (ink.r * 255.0) as u8,
        (ink.g * 255.0) as u8,
        (ink.b * 255.0) as u8
    );
    crate::model::AgentClient::ALL
        .into_iter()
        .filter_map(|client| {
            let svg = client.logo().replace("currentColor", &hex);
            ara_kit::art::svg(&svg, 40, 40).map(|image| (client, std::sync::Arc::new(image)))
        })
        .collect()
}

/// The whole window: title bar, then the sidebar and the current page (Create takes the full
/// window), with dialogs, toasts and the palette above.
pub fn view(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let create = s.hub.view == View::Create;
    let accent = l.c.ring.0;
    let mut body = row().grow(1.0).w_full().min_h(0.0);
    if !create {
        body = body.child(shell::sidebar(s));
    }
    body = body.child(shell::page(s));
    let root = col()
        .key("hub")
        .w_full()
        .h_full()
        .fill(l.c.background.0)
        .on_key(keys)
        .on_drop_files(|s: &mut HubApp, paths| {
            for path in paths.iter().filter(|p| p.is_dir()) {
                s.intent(Intent::Register(path.clone()));
            }
        })
        .dropping(move |e| e.border(2.0, accent))
        .child(shell::title_bar(s))
        .child(body);
    let root = dialogs::layer(s, root);
    let root = palette::layer(s, root);
    shell::toasts(s, root)
}

/// Window-wide shortcuts (the platform's command key is `ctrl`): ⌘K palette, ⌘N new
/// project, ⌘O add existing, ⌘1-4 pages, ⌘, settings, / search, Esc back from Create.
fn keys(s: &mut HubApp, key: KeyCode, tx: &mut stillkit::el::Tx) -> Handled {
    let m = tx.modifiers();
    let page = |v: View| Some(Intent::Navigate(v));
    let intent = match (m.ctrl, key.0) {
        (true, 0x4B) => {
            s.palette.open();
            tx.focus("palette/query");
            return Handled::Yes;
        }
        (true, 0x4E) => page(View::Create),
        (true, 0x4F) => Some(Intent::AddExisting),
        (true, 0x31) => page(View::Projects),
        (true, 0x32) => page(View::Installs),
        (true, 0x33) => page(View::Agents),
        (true, 0x34) => page(View::Learn),
        (true, 0xBC) => page(View::Settings),
        (false, 0xBF) if s.hub.view == View::Projects => {
            tx.focus("search");
            return Handled::Yes;
        }
        (false, 0x1B) if s.hub.view == View::Create && s.hub.dialog.is_none() => Some(Intent::Back),
        _ => None,
    };
    match intent {
        Some(i) => {
            s.intent(i);
            Handled::Yes
        }
        None => Handled::No,
    }
}
