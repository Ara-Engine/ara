//! The Game view outside Play: the scene through its gameplay camera, the view Play
//! starts with, rendered as a second camera on the Scene's renderer
//! (`SceneView::preview_game`) and shown as UI texture `game-preview`.
//!
//! It renders only while the Game surface is on screen, Play is not running and the
//! scene has a camera, and only when what it shows changes: the scene (store
//! revision), the camera, the frame size or newly published assets. Each change runs
//! a short burst so temporal history and arrival fades settle. Until the scene's
//! content is published it asks again every frame, keeping the empty state. The
//! image is always detached from the UI before the preview camera is released.
//! It does not poll the shared content's own work (GI, emitter search): each preview
//! frame currently restarts that work, so polling it never settles.
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
    /// `texture()` points at this preview's image.
    attached: bool,
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

    /// Whether this frame has preview work: a change to show, a burst still settling
    /// (it also runs while content is not published yet), or an image to detach.
    pub fn wants_frame(&mut self, editor: &Editor, shell: &mut Shell) -> bool {
        match Self::want(editor, shell) {
            None => self.attached,
            Some((camera, size)) => {
                let key = self.key(editor, camera, size);
                self.settle_until.is_some_and(|t| Instant::now() < t)
                    || self.shown.as_ref() != Some(&key)
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

    /// Render the preview when the Game view wants one, or let it go when it does
    /// not. True when the Game panel must rebuild (an image appeared or went away).
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
            return self.release(renderer, shell, gpu);
        };
        let key = self.key(editor, camera, size);
        let now = Instant::now();
        if self.shown.as_ref() != Some(&key) {
            self.settle_until = Some(now + SETTLE);
        } else if self.settle_until.is_none_or(|t| now >= t) {
            self.settle_until = None;
            return Ok(false);
        }
        let spec = camera.spec();
        let request = PreviewCamera {
            camera: &spec,
            size,
            // The project's rendering setting, as Play uses.
            quality: None,
        };
        let output = match scene.preview_game(renderer, editor, view, &mut self.camera, request) {
            Ok(Some(output)) => output,
            // No content published yet: keep the empty state and ask next frame.
            Ok(None) => {
                self.settle_until = Some(now + SETTLE);
                return Ok(false);
            }
            Err(error) => {
                // Reported once per change; the last image (if any) stays.
                self.shown = Some(key);
                self.settle_until = None;
                editor.report_failure(&*error);
                return Ok(false);
            }
        };
        // SAFETY: the image and view live on the renderer's device, the one `gpu`
        // draws with; the preview keeps them until it renders again (re-attached
        // here) or is released, which detaches them first.
        unsafe {
            gpu.external_image(
                texture(),
                renderer.gpu().native_image(output.image)?,
                output.view,
            )?;
        }
        self.attached = true;
        self.shown = Some(key);
        let appeared = shell.game_preview.is_none();
        shell.game_preview = Some(output.size);
        Ok(appeared)
    }

    /// Detach and free the preview at shutdown, before the Scene view is destroyed.
    pub fn destroy(mut self, renderer: &mut Renderer, gpu: &mut NativeUi) -> ara_ui::Result<()> {
        if self.attached {
            gpu.forget_external(texture());
        }
        self.camera.release(renderer)
    }

    /// Detach and free the preview (Play started, the Game view hid, no camera).
    fn release(
        &mut self,
        renderer: &mut Renderer,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<bool> {
        if self.attached {
            gpu.forget_external(texture());
            self.attached = false;
        }
        self.camera.release(renderer)?;
        self.shown = None;
        self.settle_until = None;
        Ok(shell.game_preview.take().is_some())
    }
}
