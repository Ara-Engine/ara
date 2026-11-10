//! Mesh Paint in the legacy window: its pointer as a world ray for `crate::painter`.
use super::*;

impl Controller {
    /// The world ray under the pointer, if it is over the Scene.
    fn paint_ray(editor: &Editor, shell: &Shell, size: [u32; 2]) -> Option<(Vec3, Vec3)> {
        let rect = shell.viewport()?;
        let ndc = [
            (shell.pointer[0] - rect.x) / rect.w * 2. - 1.,
            1. - (shell.pointer[1] - rect.y) / rect.h * 2.,
        ];
        editor.state.camera.ray(size, ndc)
    }

    /// A press in Mesh Paint mode. False when it hits none of the selected meshes,
    /// so the press selects as usual.
    pub(super) fn begin_paint(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        view: &crate::world::DocumentView,
        scene: &viewport::SceneView,
        size: [u32; 2],
    ) -> ara_ui::Result<bool> {
        match Self::paint_ray(editor, shell, size) {
            Some(ray) => self.paint.begin(editor, view, scene, ray),
            None => Ok(false),
        }
    }

    /// A drag of the current stroke.
    pub(super) fn drive_paint(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        size: [u32; 2],
    ) -> ara_ui::Result<bool> {
        match Self::paint_ray(editor, shell, size) {
            Some(ray) => self.paint.drive(editor, ray),
            None => Ok(false),
        }
    }

    /// Ends the stroke (`commit` writes it).
    pub(super) fn finish_paint(&mut self, editor: &mut Editor, commit: bool) -> ara_ui::Result<()> {
        self.paint.finish(editor, commit)
    }
}
