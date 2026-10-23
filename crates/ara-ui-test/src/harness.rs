use crate::{Capture, CaptureEngine, Query, Result, query};
use ara_schema::SchemaDb;
use ara_ui::{
    Ui,
    binding::Action,
    input::{Direction, InputEvent, InputState},
};
use ara_ui_stories::{InputMode, Locale, Story, StoryView, Transition, localize};
use ara_ui_text::{Clipboard, EditEvent};
mod assertions;
mod recipe;

#[derive(Default)]
struct MemoryClipboard(String);
impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> Result<()> {
        self.0 = text.into();
        Ok(())
    }
}

/// Common keyboard keys, translated through the same input/edit seam as the native host.
pub enum Key {
    /// Forward or reverse tab traversal.
    Tab {
        /// True for Shift+Tab.
        reverse: bool,
    },
    /// Enter/Space activation.
    Confirm,
    /// Escape/cancel scope or pointer capture.
    Escape,
    /// Spatial focus navigation.
    Arrow(Direction),
    /// Delete the preceding grapheme.
    Backspace,
    /// Delete the following grapheme.
    Delete,
}

/// One isolated production runtime, per-player input state and deterministic UI clock.
pub struct Harness {
    ui: Ui,
    engine: CaptureEngine,
    input: InputState,
    clipboard: MemoryClipboard,
    view: StoryView,
    locale: Locale,
    story: Story,
    actions: Vec<Action>,
}
impl Harness {
    /// Mount a schema-validated story using the pinned bundled font set.
    pub fn mount(story: &Story, view: &StoryView, db: &SchemaDb) -> Result<Self> {
        Self::with_engine(story, view, db, CaptureEngine::bundled()?)
    }
    /// Mount using caller-supplied font/image/atlas resources.
    pub fn with_engine(
        story: &Story,
        view: &StoryView,
        db: &SchemaDb,
        mut engine: CaptureEngine,
    ) -> Result<Self> {
        let ui = story.mount(view, db)?;
        for image in &story.images {
            engine.image(
                &image.source,
                ara_ui_render::gpu::Image {
                    width: image.width,
                    height: image.height,
                    rgba: image.rgba.clone(),
                },
            )?;
        }
        let mut harness = Self {
            ui,
            engine,
            input: InputState::default(),
            clipboard: MemoryClipboard::default(),
            view: view.clone(),
            locale: story.locale(&view.locale)?.clone(),
            story: story.clone(),
            actions: Vec::new(),
        };
        harness.frame()?;
        let root = ara_ui::source::stable_id(&harness.ui.nodes[0].element.id);
        harness.input.push(&mut harness.ui, root, None, false)?;
        harness.frame()?;
        Ok(harness)
    }
    /// Inspect the actual runtime without exposing authoring mutation paths.
    pub fn ui(&self) -> &Ui {
        &self.ui
    }
    /// Inspect the fixed view configuration and current deterministic time.
    pub fn view(&self) -> &StoryView {
        &self.view
    }
    /// All emitted named actions since mount; observation never consumes host commands.
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }
    /// Find exactly one visible semantic control and return its stable authored identity.
    pub fn query(&self, query: &Query) -> Result<String> {
        let query = self.resolve(query)?;
        let matches = query::find(&self.ui, &query);
        if let [i] = matches.as_slice() {
            return Ok(self.ui.nodes[*i].element.id.clone());
        }
        Err(query::fail(
            &self.ui,
            format!(
                "Expected one {:?} {:?}, found {}",
                query.by,
                query.value,
                matches.len()
            ),
        )
        .into())
    }
    /// Find all visible matches, preserving native reading order.
    pub fn query_all(&self, query: &Query) -> Result<Vec<String>> {
        Ok(query::find(&self.ui, &self.resolve(query)?)
            .into_iter()
            .map(|i| self.ui.nodes[i].element.id.clone())
            .collect())
    }
    fn resolve(&self, query: &Query) -> Result<Query> {
        let resolve = |value: &str| {
            if value.starts_with("loc:") {
                localize(value, &self.locale)
            } else {
                Ok(value.into())
            }
        };
        Ok(Query {
            by: query.by,
            value: resolve(&query.value)?,
            name: query.name.as_deref().map(resolve).transpose()?,
        })
    }
    fn index(&self, query: &Query) -> Result<usize> {
        let id = self.query(query)?;
        self.ui
            .find(&id)
            .ok_or_else(|| "queried node disappeared".into())
    }
    fn frame(&mut self) -> Result<()> {
        sample(&mut self.ui, &self.story.transitions, self.view.time_ms)?;
        self.engine.frame(&mut self.ui, self.locale.rtl)?;
        self.actions.extend(self.ui.drain_actions());
        Ok(())
    }
    /// Inject any production pointer/keyboard/gamepad/IME/clipboard event, then update one frame.
    pub fn event(&mut self, event: InputEvent) -> Result<()> {
        self.input
            .event(&mut self.ui, event, &mut self.clipboard)
            .map_err(|error| query::fail(&self.ui, format!("Input failed: {error}")))?;
        self.frame()
    }
    /// Simulate an ordinary key through native navigation or text editing.
    pub fn key(&mut self, key: Key) -> Result<()> {
        self.event(match key {
            Key::Tab { reverse } => InputEvent::Tab { reverse },
            Key::Confirm => InputEvent::Activate,
            Key::Escape => InputEvent::Cancel,
            Key::Arrow(direction) => InputEvent::Navigate(direction),
            Key::Backspace => InputEvent::Edit(EditEvent::Backspace),
            Key::Delete => InputEvent::Edit(EditEvent::Delete),
        })
    }
    /// Move controller focus through the production directional graph.
    pub fn gamepad(&mut self, direction: Direction) -> Result<()> {
        self.event(InputEvent::Navigate(direction))
    }
    /// Focus an enabled, unclipped control in the current screen/modal scope.
    pub fn focus(&mut self, query: &Query) -> Result<()> {
        let id = self.query(query)?;
        self.input
            .focus(&mut self.ui, ara_ui::source::stable_id(&id))
            .map_err(|error| query::fail(&self.ui, format!("Focus on {id} failed: {error}")))?;
        self.frame()
    }
    /// Push a native screen/modal scope, preserving the previous scope's stable focus identity.
    pub fn push_scope(&mut self, root: &Query, initial: Option<&Query>, modal: bool) -> Result<()> {
        let root = ara_ui::source::stable_id(&self.query(root)?);
        let initial = initial
            .map(|q| self.query(q))
            .transpose()?
            .map(|id| ara_ui::source::stable_id(&id));
        self.input.push(&mut self.ui, root, initial, modal)?;
        self.frame()
    }
    /// Pop a native scope and restore the last surviving focus in its parent scope.
    pub fn pop_scope(&mut self) -> Result<()> {
        self.input.pop(&mut self.ui);
        self.frame()
    }
    /// Activate using the view's selected input device, respecting clipping, hit testing and focus.
    pub fn activate(&mut self, query: &Query) -> Result<()> {
        let i = self.index(query)?;
        if self.view.input == InputMode::Mouse {
            let n = &self.ui.nodes[i];
            let r = n.rect.intersection(n.clip);
            let p = [r.x + r.w / 2.0, r.y + r.h / 2.0];
            if self.ui.hit_test(p[0], p[1]) != Some(i) {
                return Err(query::fail(
                    &self.ui,
                    format!(
                        "{} cannot receive the pointer at its visible center",
                        n.element.id
                    ),
                )
                .into());
            }
            self.event(InputEvent::PointerMove(p))?;
            self.event(InputEvent::PointerDown(p))?;
            self.event(InputEvent::PointerUp(p))
        } else {
            self.focus(query)?;
            self.event(InputEvent::Activate)
        }
    }
    /// Type into the currently focused field through its grapheme-safe editing seam.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        self.event(InputEvent::Edit(EditEvent::Insert(text.into())))
    }
    /// Advance an explicit integer UI clock; no sleeping, wall-clock sampling or gameplay ticks.
    pub fn advance(&mut self, milliseconds: u32) -> Result<()> {
        self.view.time_ms = self
            .view
            .time_ms
            .checked_add(milliseconds)
            .ok_or("UI clock overflow")?;
        self.frame()
    }
    /// Capture this exact retained state, including focused identity and fixture provenance.
    pub fn capture(&mut self) -> Result<Capture> {
        self.frame()?;
        let mut capture = self
            .engine
            .capture(&mut self.ui, &self.view, self.locale.rtl)?;
        capture.report.story = Some(self.story.name.clone());
        capture.report.fixture_type = Some(self.story.fixture_type.clone());
        capture.report.focused = self
            .input
            .focused()
            .and_then(|id| self.ui.by_id(id))
            .map(|i| self.ui.nodes[i].element.id.clone());
        capture.report.focus_graph = serde_json::from_value(self.input.focus_graph(&self.ui))?;
        Ok(capture)
    }
}
fn sample(ui: &mut Ui, tracks: &[Transition], time: u32) -> Result<()> {
    for track in tracks {
        let i = ui.find(&track.node).ok_or("transition node disappeared")?;
        let t = if time < track.start_ms {
            0.0
        } else if track.duration_ms == 0 {
            1.0
        } else {
            (time.saturating_sub(track.start_ms) as f32 / track.duration_ms as f32).min(1.0)
        };
        let offset = [
            track.from[0] + (track.to[0] - track.from[0]) * t,
            track.from[1] + (track.to[1] - track.from[1]) * t,
        ];
        if ui.nodes[i].paint_offset != offset {
            ui.nodes[i].paint_offset = offset;
            ui.nodes[i].dirty.paint = true;
        }
    }
    Ok(())
}
