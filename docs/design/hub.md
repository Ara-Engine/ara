# Ara Hub: the first screen

The Hub (D25, `crates/ara-hub`) is the first thing a downloader sees. This doc is the design its pages are built to. It follows D18 (tokens only) and the brand (`brand.md`). Keep retained reconciliation and event-driven presentation; downloads and verification run off the input thread.

**References:**
- Unity Hub 3 (`D:\AraRefs\editor-ui\unity-hub\`, from Unity's documentation): `hub-projects`, `hub-new-project`, `installs-window`, `installs-manage`, `installs-downloads`.
- The Epic Games Launcher's Unreal Engine section, from Epic's "Install Unreal Engine" documentation. Its screenshots sit behind a bot challenge, so the layout is taken from the page's text: tabs for News, Samples, Fab and Library; a Library of engine-version tiles, each with a Launch button and a version dropdown, a "+" tile to add a version, and My Projects as thumbnails; downloads tracked in their own panel.

**Benchmarks:**
- **Unity Hub:** a left navigation card, then one content panel with a large page title, search, a secondary button and one primary button. Projects is a dense table whose row opens the project; secondary actions sit behind ⋯. New project is a template gallery beside a settings column.
- **Epic Games Launcher:** the version is the hero of each engine tile, each project is a picture, and notifications and downloads live outside the page content.

**Where Ara beats them:** projects can be open in the editor or headless for agents, and the Hub shows which. Agents are first-class: they're connected once, and live activity is shown per project. Engine builds install side by side, each project pins one, and a missing pin installs on open.

## Earlier layout (`crates/ara-hub/evidence/`)

The Hub is sound. Its clutter:
- **The update banner repeats.** The same "Ara Hub 0.1.0 is available" banner tops Projects, New project, Installs, Agents and Learn, and it stacks under the offline banner.
- **The agent card never leaves.** The sidebar shows "Connect an AI agent" on every page, even with two agents live.
- **Every card has two buttons.** Each project card carries a row of actions (Open + Headless, or Open editor + Close), repeated across the grid. Headless is already in the card's ⋯ menu.
- **No-image tiles look unfinished.** Projects without a thumbnail show 44 px initials in a chart colour on a flat dark tile. New projects start like this, so it is the common case.
- **The first run repeats the header.** A 290 px empty box repeats the header's New project and Add existing buttons above the templates.
- **Two smaller layout faults:**
  - On Installs, "Release notes" stretches into a full-width bar.
  - On Agents, the setup steps sit above live activity even while agents are working.

## Design

- **Updates are a user decision.** A quiet sidebar indicator opens a focused release-notes dialog. Update now downloads; Later closes the dialog; Skip this version persists the signed release/build identity. Ask first is the default download preference; metadata checks only offer the release. An explicit Download in background opt-in can prepare new unskipped updates, and still waits for Restart and update. The download shows byte progress and verification, supports cancellation, and runs in the background when the dialog closes. A second explicit Restart and update step applies the independently verified archive. Settings always keeps the offered release discoverable, including a skipped release.
- **Confirmations and errors use dialogs.** Uninstalling an engine and switching a project's engine state their consequences before a typed Hub operation runs. Reversible project Remove stays one action with a toast Undo. Project opening and closing stay direct. Error dialogs retain catalog codes, fixes, logs and retry. Success notices stay as short-lived toasts; Remove keeps Undo. Service connectivity stays an inline status while reconnection runs.
- **Dialogs use the shared native kit layer.** The dialog blocks background clicks and shortcuts, traps Tab focus, closes on Escape and restores focus to its launcher. Destructive decisions initially focus Cancel. Background errors wait for the current confirmation to finish; replacing transfer content or an error chooses a valid safe action while ordinary progress preserves focus.
- **Technical source settings use Advanced.** Repository overrides, environment variables and release-signing internals are disclosed only on request. Normal installation copy focuses on the editor version, destination, space and affected projects.
- **Enum settings use dropdowns.** Release channel, project sorting, agent client, create engine and per-project engine version use the shared searchable Select. Native committed text filters the list; replies preserve the selected and active typed values. The two icon buttons for card/list presentation remain a view toggle.
- **Engine versions install side by side.** Install opens notes and a download summary; jobs show downloading, signature/checksum verification, extraction and installation. A read-only preflight shows version, channel, destination, estimated required space, available space and pin impact before the transfer. Disk checks run again before transfer and extraction. Repair uses the same progress/cancel/retry path and keeps version pins. Cancel is available until the atomic commit; failed or cancelled transfers can be retried. Uninstall first runs a cancellable, read-only inspection and shows the exact installation, approximate space freed and pinned projects. The destructive action stays disabled until inspection succeeds and the user confirms. Closing this summary cancels only the inspection; stale replies cannot delete files or reopen an error. Uninstall then reports its own asynchronous progress and keeps projects on disk.
- **Setup shares Hub's language.** A compact native dialog distinguishes upgrade from replacing the same version, names the destination, explains rollback, and keeps projects/settings. Its states can all be rendered offscreen.
- **The sidebar shows at most one card.** An update indicator outranks agent onboarding. "Connect an AI agent" shows only while no agent is around, and never on the Agents page.
- **Each project card has one action:** Open (or "Install 0.0.3-alpha and open"), Open editor for a headless project, or Close for an open one; a headless project also keeps its Close. Everything else lives in the card's ⋯ menu: open in the editor or headless, close, rename, show in Explorer and remove from the list.
- **No-image tiles** read like avatars, with 30 px initials. The project's ID picks one of six looks: the `info` or `success` tinted surface with initials in its colour, or the neutral `raised` surface with chart-coloured initials. Never the warning or error tints, which would read as a problem.
- **First run:** a single welcome row ("No projects yet", one line on what to do), then the templates and samples as the hero.
- **Installs:** Release notes is a compact button.
- **Agents:** live activity comes first when anyone is attached; the setup steps follow. When nobody is attached, the setup steps lead and the empty state follows.
