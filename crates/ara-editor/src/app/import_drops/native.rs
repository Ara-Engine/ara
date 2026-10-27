//! Native transport retains files across event pumps and never imports before completion.
use super::*;

fn window_id(sdl: u32, main: &Window) -> u32 {
    if sdl == 0 { main.id() } else { sdl }
}
fn alive(sdl: u32, editor: &Editor, main: &Window, floats: &windows::Floats) -> bool {
    sdl == main.id()
        || floats.windows.iter().any(|window| {
            window.sdl == sdl
                && editor
                    .state
                    .floating
                    .iter()
                    .any(|state| state.id == window.id)
        })
}

impl ImportDrops {
    /// Run before editor/game event routing. Lifecycle cancellation continues through
    /// normal routing; native drop events and Escape are consumed here.
    #[allow(clippy::too_many_arguments)]
    pub fn event(
        &mut self,
        event: &Event,
        sdl: u32,
        editor: &Editor,
        shell: &mut Shell,
        main: &Window,
        dpi: f32,
        floats: &windows::Floats,
    ) -> ara_ui::Result<bool> {
        let tagged = window_id(sdl, main);
        match event {
            Event::FileDropBegin => {
                self.cancel(shell)?;
                self.next = self.next.wrapping_add(1).max(1);
                self.stream = Some(Stream {
                    id: self.next,
                    window: tagged,
                    files: Batch::default(),
                    positions: BTreeMap::new(),
                    current: None,
                    cancelled: false,
                });
                shell.begin_file_drag(self.next)?;
            }
            Event::FileDropPosition { position } => {
                self.position(*position, tagged, editor, shell, main, dpi, floats)?;
            }
            Event::FileDrop { path, position } => {
                self.native(
                    path.clone(),
                    *position,
                    sdl,
                    editor,
                    shell,
                    main,
                    dpi,
                    floats,
                )?;
            }
            Event::FileDropComplete => {
                self.complete(tagged, editor, shell, main, dpi, floats)?;
            }
            Event::Key {
                scancode: 41,
                pressed: true,
                ..
            } if self.stream.as_ref().is_some_and(|stream| !stream.cancelled) => {
                self.cancel(shell)?;
            }
            Event::FocusLost | Event::Quit => {
                if self
                    .stream
                    .as_ref()
                    .map_or(!self.batches.is_empty(), |stream| {
                        stream.current.map_or(stream.window, |(window, _)| window) == tagged
                    })
                {
                    self.cancel(shell)?;
                }
                return Ok(false);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    /// Cancellation keeps an inert stream until COMPLETE, so late FILE events
    /// cannot fall back to legacy frame imports after Escape or focus loss.
    pub fn cancel(&mut self, shell: &mut Shell) -> ara_ui::Result<bool> {
        let mut changed = !self.batches.is_empty();
        self.batches.clear();
        if let Some(stream) = &mut self.stream {
            changed |= !stream.cancelled;
            stream.cancelled = true;
            stream.files = Batch::default();
            stream.positions.clear();
            stream.current = None;
        }
        changed |= shell.cancel_file_drag()?;
        Ok(changed)
    }
    /// Revalidate retained geometry and auto-scroll even without another native
    /// position event. A closed destination cancels the complete pending batch.
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        main: &Window,
        dpi: f32,
        floats: &windows::Floats,
        seconds: f32,
    ) -> ara_ui::Result<bool> {
        let Some(stream) = &self.stream else {
            return Ok(false);
        };
        if stream.cancelled {
            return Ok(false);
        }
        let Some((window, position)) = stream.current else {
            return if alive(stream.window, editor, main, floats) {
                Ok(false)
            } else {
                self.cancel(shell)
            };
        };
        if !alive(window, editor, main, floats) {
            return self.cancel(shell);
        }
        let result = self.show(position, window, editor, shell, main, dpi, floats, seconds);
        if result.is_err() {
            self.cancel(shell)?;
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    fn position(
        &mut self,
        position: [f32; 2],
        window: u32,
        editor: &Editor,
        shell: &mut Shell,
        main: &Window,
        dpi: f32,
        floats: &windows::Floats,
    ) -> ara_ui::Result<()> {
        let Some(stream) = &mut self.stream else {
            return Ok(());
        };
        if stream.cancelled {
            return Ok(());
        }
        if !position.into_iter().all(f32::is_finite) {
            self.cancel(shell)?;
            return Err("The native file drag position is non-finite".into());
        }
        if !alive(window, editor, main, floats) {
            self.cancel(shell)?;
            return Ok(());
        }
        stream.positions.insert(window, position);
        stream.current = Some((window, position));
        let result = self
            .show(position, window, editor, shell, main, dpi, floats, 0.)
            .map(|_| ());
        if result.is_err() {
            self.cancel(shell)?;
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn native_file(
        &mut self,
        source: PathBuf,
        position: [f32; 2],
        sdl: u32,
        editor: &Editor,
        shell: &mut Shell,
        main: &Window,
        dpi: f32,
        floats: &windows::Floats,
    ) -> ara_ui::Result<()> {
        let window = window_id(sdl, main);
        let Some(stream) = &mut self.stream else {
            return Ok(());
        };
        if stream.cancelled {
            return Ok(());
        }
        stream.files.push(source);
        // SDL FILE coordinates share its global last-position cache. Prefer a
        // position already tagged to this window over that unscoped cache.
        let position = match stream.positions.get(&window).copied() {
            Some(position) => position,
            None if position != [0.; 2] => position,
            // SDL initializes its completion/file coordinate cache to zero.
            // Without a tagged position, this is not an authored destination.
            None => return Ok(()),
        };
        self.position(position, window, editor, shell, main, dpi, floats)
    }
    #[allow(clippy::too_many_arguments)]
    fn complete(
        &mut self,
        window: u32,
        editor: &Editor,
        shell: &mut Shell,
        main: &Window,
        dpi: f32,
        floats: &windows::Floats,
    ) -> ara_ui::Result<()> {
        let Some(stream) = &self.stream else {
            return Ok(());
        };
        if stream.cancelled || stream.files.sources.is_empty() {
            self.stream = None;
            shell.cancel_file_drag()?;
            return Ok(());
        }
        let Some((current, position)) = stream.current.filter(|(current, _)| *current == window)
        else {
            self.stream = None;
            shell.cancel_file_drag()?;
            return Ok(());
        };
        if !alive(current, editor, main, floats) {
            self.cancel(shell)?;
            self.stream = None;
            return Ok(());
        }
        if let Err(error) = self.show(position, current, editor, shell, main, dpi, floats, 0.) {
            self.cancel(shell)?;
            self.stream = None;
            return Err(error);
        }
        let stream = self.stream.take().ok_or("Native file batch disappeared")?;
        let Some(folder) = shell.finish_file_drag()? else {
            return Ok(());
        };
        if folder.batch != stream.id {
            return Err("Native file drop belongs to a stale drag".into());
        }
        let destination = destination(
            &folder.storage,
            shell.asset_authored_files(),
            shell.asset_reference_root(),
        )?;
        if destination.authored != folder.authored {
            return Err("The import folder changed before the drop completed".into());
        }
        shell.queue_asset_import(Request {
            sources: stream.files.sources,
            destination: destination.authored,
            conflicts: destination.conflicts,
        })
    }
    /// Enter exactly the event's window and retain its own scale/position. COMPLETE
    /// never supplies a new coordinate: SDL's global completion cache is not used.
    #[allow(clippy::too_many_arguments)]
    fn show(
        &self,
        position: [f32; 2],
        window: u32,
        editor: &Editor,
        shell: &mut Shell,
        main: &Window,
        dpi: f32,
        floats: &windows::Floats,
        seconds: f32,
    ) -> ara_ui::Result<bool> {
        shell.leave();
        let (scale, rect) = if window == main.id() {
            (
                crate::native::points_to_logical(main.raw(), dpi),
                main.rect()?,
            )
        } else {
            let float = floats
                .windows
                .iter()
                .find(|float| float.sdl == window)
                .ok_or("The native file drop window was closed")?;
            let rect = float.window.rect()?;
            if !shell.enter(&float.id) {
                return Err("The native file drop surface was closed".into());
            }
            (
                crate::native::points_to_logical(
                    float.window.raw(),
                    float.window.display_scale() * windows::ui_scale(editor),
                ),
                rect,
            )
        };
        let result = if !scale.is_finite() || scale <= 0. {
            Err("The native file drop surface scale is invalid".into())
        } else {
            shell.file_drag_over(
                editor,
                position.map(|value| value * scale),
                [rect[0] as f32 + position[0], rect[1] as f32 + position[1]],
                seconds,
            )
        };
        shell.leave();
        result
    }
}
