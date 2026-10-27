# ara-setup

The program at the front of both Windows setup executables: the engine
(`ara-<version>-windows-x64-setup.exe`) and the Hub (`ara-hub-<version>-…-setup.exe`).
`ara-release package` appends the portable ZIP to `target/release/ara-setup.exe`
with `ara_export_installer::append`; the payload's manifest (`engine.json` or
`hub.json`) decides what is installed. Native Win32 + GDI only: no GPU, no Ara UI
runtime, no .NET, so it runs on a fresh machine.

## Behaviour

- Double click: a 480 × 200 dark card (Ara Dark tokens from
  `assets/themes/ara-dark.json`), centred on the pointer's monitor, per-monitor-v2
  DPI aware, DWM-rounded on Windows 11 (painted square border on Windows 10),
  dragged anywhere outside its buttons. States: Busy (verify → extract → swap →
  register, byte progress), Confirm (replace?), Blocked (programs running from the
  folder), Error (code, message, Copy details, never auto-closes), Done (Launch Ara
  Hub). Close is ignored while busy. Enter = primary button, Esc = Cancel/Close.
- The window appears at once; `flow::run` works on a worker thread and sends
  `View`s. Questions (`Host::confirm_replace`, `Host::resolve`) block the worker
  until a button answers.
- Console modes (`--silent`, `--list`, `--help`, `--version`) attach to the parent
  console (`AttachConsole`) unless stdout/stderr are already redirected. It is a
  GUI-subsystem exe: PowerShell needs `| Out-Host` to wait for it.
- Defaults: `%LOCALAPPDATA%\Programs\Ara\<version>` / `…\Programs\Ara Hub`.

## Replace

Only a same-kind Ara installation (`engine.json` format `ara.engine/1`, or
`hub.json` format `ara.hub/1` without `engine.json`) can be replaced; anything else
is `ARA-EXP-INSTALL-006` and is never touched. The window asks; silent installs need
`--replace` (otherwise `ARA-EXP-INSTALL-004`, as before). Programs whose image is
inside the folder block it (`processes::running_from`); silent fails with `-007`,
the window offers Retry and, for the Hub service (PID from
`<config>/hub/.ara/daemon.json`), Stop Hub service = `ara hub shutdown`. Nothing is
ever killed. The swap: extract to `.ara-install-*`, rename old → `.ara-replaced-*`,
stage → destination, delete the backup; failures before the new files are in place
restore the old install (`-008`, or `-009` naming the backup). Registration
(`install.ps1 -RegisterOnly`, System32 PowerShell, `CREATE_NO_WINDOW`, output
captured) runs after the swap; failure is `-010` with the output, files kept.
Hub replacements also hold `.ara-hub-update.lock`, shared with self-update and engine
registration. A newer installed version/source generation is retained, never downgraded.

## Layout

- `args.rs` hand-parsed CLI (`USAGE`); `flow.rs` the transaction, `Kind`, `inspect`,
  `Host`/`Quiet`, `summarize`; `processes.rs`; `register.rs`.
- `ui/mod.rs` `View`, `Action`, buttons; `ui/canvas.rs` SDF shapes on DIB pixels;
  `ui/paint.rs` layout, GDI text, `render` (offscreen RGBA); `ui/window.rs` Win32.
- `build.rs` embeds `assets/brand/icons/engine/ara-engine.rc` (engine icon,
  including the shared Hub setup stub) and links with `/DEPENDENTLOADFLAG:0x800`
  (imports from System32 only: no DLL planting beside a downloaded setup).

## Tests

`CARGO_BUILD_JOBS=2`. `cargo test -p ara-setup`;
`cargo clippy -p ara-setup --all-targets -- -D warnings`.

- `tests/sfx.rs` appends tiny payloads (fixture `install.ps1`) to the built stub
  and runs it in console modes only, in temp destinations: fresh install, replace,
  foreign folder, failed registration, a running program, a locked file (rollback),
  `--list`/`--help`/`--version`, corrupt payloads.
- `tests/ui_states.rs` renders every state at 1x and 2x to
  `target/ara-setup-ui/*.png` (`Frame::Preview` = the Windows 11 look). Look at them
  after any paint change.

## Gotchas

- Never run the built exe without a console flag in tests or checks: that opens a
  window on the desktop. Verify visuals through `paint::render` only.
- Never install into the real `%LOCALAPPDATA%\Programs`; always pass `--destination`.
- GDI text clears the alpha byte; `paint` restores it (the DWM frame extension
  would otherwise show through).
