//! The Game view outside Play: the scene through its gameplay camera, the view Play
//! starts with, rendered as a second camera on the Scene's renderer
//! (`SceneView::preview_game`) and shown as UI texture `game-preview`.
//!
//! It renders only while the Game surface is on screen, Play is not running and the
//! scene has a camera, and only when what it shows changes: the scene (store
//! revision), the camera, the frame size or newly published assets. Each change runs
//! a short burst so temporal history and arrival fades settle. Frames are submitted
//! without waiting: the panel shows the last completed image and re-attaches it when
//! a newer one completes. After the burst the last submitted frame is only polled,
//! never followed by another, unless the shared content still has work the preview
//! must show (`SceneView::preview_needs_frame`: publication, late GI, emitter fades).
//! Until the scene's content is published it asks again every frame, keeping the
//! empty state. Hiding renders nothing and keeps the last image attached, so showing
//! the Game view again needs no second rebuild for it. Play starting in the Game view
//! keeps the last image under its progress until its first frame (a lit one: the
//! player publishes once its lighting has settled) replaces it, then lets it go;
//! editor shutdown detaches and releases its targets.
use std::time::{Duration, Instant};

use ara_editor_gpu::NativeUi;
use ara_play_host::PublishedAssets;

use super::viewport::{GamePreview, PreviewCamera, SceneView};
use crate::{Editor, shell::Shell, state::Camera, world::DocumentView};
use ara_render::Renderer;

/// How long frames keep coming after a change: temporal history converges and
/// arrival fades finish.
const SETTLE: Duration = Duration::from_millis(600);

/// UI texture of the preview image (`<image src='game-preview'>`).
pub fn texture() -> u32 {
    ara_ui::source::stable_id("game-preview") as u32 & 0x7fff_ffff
}

/// What a preview frame shows; a different key renders again.
#[derive(Clone, PartialEq)]
struct Key {
    revision: u64,
    doc: String,
    camera: Camera,
    size: [u32; 2],
    published: u64,
}

/// The Game view's preview camera and what it last showed.
#[derive(Default)]
pub struct Preview {
    camera: GamePreview,
    /// Read-only published-assets feed: new content renders again.
    published: Option<PublishedAssets>,
    shown: Option<Key>,
    /// Frames keep coming until then (see [`SETTLE`]).
    settle_until: Option<Instant>,
    /// The completed image `texture()` points at.
    attached: Option<ara_gpu::Image>,
}

impl Preview {
    /// The camera and size a preview would render now, if the Game view (in the main
    /// window or a floating one) wants one.
    fn want(editor: &Editor, shell: &mut Shell) -> Option<(Camera, [u32; 2])> {
        let target = shell.game_target;
        let visible = shell.game_host().is_some() && target.iter().all(|v| *v >= 64);
        if !visible || editor.state.playing.is_some() || shell.game_live {
            return None;
        }
        Some((shell.gameplay_camera(editor)?, target))
    }

    /// Whether a preview that is not wanted keeps its image: its Game view is only
    /// hidden (another tab, or not laid out) and nothing else needs the view, or Play
    /// is starting in the Game view and has not shown a frame yet.
    fn keeps(editor: &Editor, shell: &Shell) -> bool {
        !shell.game_live
            && match editor.state.playing {
                None => shell.game_host().is_none(),
                Some(_) => editor.plays_in_game_view(),
            }
    }

    /// Whether this frame has preview work: a change to show, a burst still settling,
    /// shared content work the preview must show, a submitted frame to collect, or an
    /// image to detach (not while its view is only hidden).
    pub fn wants_frame(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        renderer: &Renderer,
        scene: &SceneView,
    ) -> bool {
        match Self::want(editor, shell) {
            None => self.attached.is_some() && !Self::keeps(editor, shell),
            Some((camera, size)) => {
                let key = self.key(editor, camera, size);
                self.shown.as_ref() != Some(&key)
                    || self.settle_until.is_some_and(|t| Instant::now() < t)
                    || self.camera.pending(renderer)
                    || scene.preview_needs_frame()
            }
        }
    }

    fn key(&mut self, editor: &Editor, camera: Camera, size: [u32; 2]) -> Key {
        if self.published.is_none() {
            self.published = PublishedAssets::for_project(editor.host.root()).ok();
        }
        let published = self.published.as_ref().map_or(0, PublishedAssets::revision);
        Key {
            revision: editor.host.store().revision(),
            doc: editor.state.doc.clone(),
            camera,
            size,
            published,
        }
    }

    /// Submit or collect preview frames when the Game view wants one, or let it go
    /// when it does not. True when the Game panel must rebuild (an image appeared or
    /// went away).
    pub fn update(
        &mut self,
        renderer: &mut Renderer,
        scene: &mut SceneView,
        editor: &mut Editor,
        view: &DocumentView,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<bool> {
        let Some((camera, size)) = Self::want(editor, shell) else {
            if Self::keeps(editor, shell) {
                return Ok(false);
            }
            return self.release(renderer, shell, gpu);
        };
        let key = self.key(editor, camera, size);
        let now = Instant::now();
        if self.shown.as_ref() != Some(&key) {
            self.settle_until = Some(now + SETTLE);
        }
        let settling = self.settle_until.is_some_and(|t| now < t);
        if !settling {
            self.settle_until = None;
        }
        let output = if settling || scene.preview_needs_frame() {
            let spec = camera.spec();
            let request = PreviewCamera {
                camera: &spec,
                size,
                // The project's rendering setting, as Play uses.
                quality: None,
            };
            scene.preview_game(renderer, editor, view, &mut self.camera, request)
        } else if self.camera.pending(renderer) {
            // The burst is over: collect its last frame without asking for another.
            self.camera.poll(renderer)
        } else {
            return Ok(false);
        };
        self.shown = Some(key);
        let output = match output {
            Ok(Some(output)) => output,
            // Nothing completed yet. Unpublished content submits nothing: keep the
            // burst going so the preview asks again.
            Ok(None) => {
                if !self.camera.pending(renderer) {
                    self.settle_until = Some(now + SETTLE);
                }
                return Ok(false);
            }
            Err(error) => {
                // Reported once per change; the last image (if any) stays.
                self.settle_until = None;
                editor.report_failure(&*error);
                return Ok(false);
            }
        };
        if self.attached == Some(output.image) {
            return Ok(false);
        }
        // SAFETY: the image and view live on the renderer's device, the one `gpu`
        // draws with; the camera retains this completed image until a newer one
        // completes (re-attached here) or it is released, which detaches it first.
        unsafe {
            gpu.external_image(
                texture(),
                renderer.gpu().native_image(output.image)?,
                output.view,
            )?;
        }
        self.attached = Some(output.image);
        let appeared = shell.game_preview.is_none();
        shell.game_preview = Some(output.size);
        Ok(appeared)
    }

    /// Detach and free the preview at shutdown, before the Scene view is destroyed.
    pub fn destroy(mut self, renderer: &mut Renderer, gpu: &mut NativeUi) -> ara_ui::Result<()> {
        if self.attached.is_some() {
            gpu.forget_external(texture());
        }
        self.camera.release(renderer)
    }

    /// Detach the preview without drawing when Play takes the view (or it has no
    /// camera or room). Keep its single camera resident so Stop reuses its targets and
    /// history; `destroy` releases them.
    fn release(
        &mut self,
        _renderer: &mut Renderer,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<bool> {
        if self.attached.take().is_some() {
            gpu.forget_external(texture());
        }
        self.shown = None;
        self.settle_until = None;
        Ok(shell.game_preview.take().is_some())
    }
}
