# Ara Hub: the first screen

The Hub (D25, `crates/ara-hub`) is the first thing a downloader sees. This doc is the design its pages are built to. It follows D18 (tokens only) and the brand (`brand.md`). It must keep the Hub's retained reconcile, its theme tokens and its responsiveness: no new per-frame work, markup and styles only.

**References:**
- Unity Hub 3 (`D:\AraRefs\editor-ui\unity-hub\`, from Unity's documentation): `hub-projects`, `hub-new-project`, `installs-window`, `installs-manage`, `installs-downloads`.
- The Epic Games Launcher's Unreal Engine section, from Epic's "Install Unreal Engine" documentation. Its screenshots sit behind a bot challenge, so the layout is taken from the page's text: tabs for News, Samples, Fab and Library; a Library of engine-version tiles, each with a Launch button and a version dropdown, a "+" tile to add a version, and My Projects as thumbnails; downloads tracked in their own panel.

**Benchmarks:**
- **Unity Hub:** a left navigation card, then one content panel with a large page title, search, a secondary button and one primary button. Projects is a dense table whose row opens the project; secondary actions sit behind ⋯. New project is a template gallery beside a settings column.
- **Epic Games Launcher:** the version is the hero of each engine tile, each project is a picture, and notifications and downloads live outside the page content.

**Where Ara beats them:** projects can be open in the editor or headless for agents, and the Hub shows which. Agents are first-class: they're connected once, and live activity is shown per project. Engine builds install side by side, each project pins one, and a missing pin installs on open.

## Today (`crates/ara-hub/evidence/`)

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

- **Notifications have one place each:**
  - Errors (service offline) stay as an inline banner at the top of the content, because they block work.
  - **A Hub update is a sidebar card on every page except Settings**, where it shows in full. The card has the version, one line of state (available, downloading with its %, or ready) and the one action that moves it on: Download, Cancel or Restart to update. It also has a Dismiss button.
  - Results of actions (opened, removed with Undo, failed with fixes) stay as toasts, bottom-right.
- **The sidebar shows at most one card.** An update outranks agent onboarding. "Connect an AI agent" shows only while no agent is around, and never on the Agents page.
- **Each project card has one action:** Open (or "Install 0.0.3-alpha and open"), Open editor for a headless project, or Close for an open one; a headless project also keeps its Close. Everything else lives in the card's ⋯ menu: open in the editor or headless, close, rename, show in Explorer and remove from the list.
- **No-image tiles** read like avatars, with 30 px initials. The project's ID picks one of six looks: the `info` or `success` tinted surface with initials in its colour, or the neutral `raised` surface with chart-coloured initials. Never the warning or error tints, which would read as a problem.
- **First run:** a single welcome row ("No projects yet", one line on what to do), then the templates and samples as the hero.
- **Installs:** Release notes is a compact button.
- **Agents:** live activity comes first when anyone is attached; the setup steps follow. When nobody is attached, the setup steps lead and the empty state follows.
