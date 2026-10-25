# Ara Hub: before and after

Design: [`docs/design/hub.md`](../../../docs/design/hub.md).
Open `gallery.html` to compare each page side by side.

## Method

- **Before:** `main` at `75a66c7e`. **After:** round 3.
- `ara-hub --capture <dir> --size 1440x900 --dpi 1` and `--size 2560x1440 --dpi 2`: a headless
  Vulkan device drives the real `NativeUi` adapter from the seeded fixture (`src/capture.rs`);
  no window opens, nothing connects or checks releases. A fresh config dir per run; the exe runs
  from inside the checkout so the fixture finds the sample previews.
- `before/1440` and `after/1440` hold the reviewed pages as WebP (quality 90, legibility
  checked); the PNGs are in `target/tool-pages-png/r3-hub`. Full sets for both sizes, including
  the menu, rename and dismiss interaction shots, are at
  `D:\AraBuild\tool-pages-evidence\r3-hub-{before,after}`.

## What changed

| Page | Before | After |
|---|---|---|
| Every page | The "Ara Hub 0.1.0 is available" banner topped every page; the "Connect an AI agent" card stayed in the sidebar with two agents live | The update is one sidebar card (in full on Settings); the agent card shows only while no agent is around |
| Projects | Two buttons on every card (Open + Headless); 44 px chart initials on flat tiles | One action per card (Headless is in ⋯); 30 px avatar initials on neutral or blue/green tiles |
| First run | A 290 px empty box repeating the header's buttons | One welcome row, then the templates and samples |
| Installs | "Release notes" stretched into a full-width bar | A compact button |
| Agents | Setup steps above live activity, even with agents at work | Live activity first; setup follows |

Responsiveness is unchanged: markup and styles only, no new per-frame work. The Hub's retained
reconcile, theme tokens and every responsiveness test stay as they were.

## Gates

`cargo clippy -p ara-hub --all-targets -D warnings` and `cargo test -p ara-hub` with an isolated
`ARA_CONFIG_DIR` (views, responsiveness, input, async RPC, updates, config isolation).
