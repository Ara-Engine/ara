# Editor theme

Ara's editor ships two default themes that match **T3 Code's default dark and light palettes**: [`assets/themes/ara-dark.json`](../../assets/themes/ara-dark.json) (the default) and [`assets/themes/ara-light.json`](../../assets/themes/ara-light.json). The values were read from T3 Code's live theme variables on 2026-10-23.

## The look

- **Near-black neutral surfaces.** Dark mode uses `background` #0a0a0a for the app and `card`/`muted` #111111 for panels.
- **Barely-there borders.** `border` is #191919 in dark mode and #e4e4e7 in light mode. Depth comes from small surface steps, not lines or shadows.
- **One blue accent.** `primary`/`ring` is #346bf1 in dark mode and #1b4ed8 in light mode. Use it for focus, selection and primary actions.
- **Status colours.** `destructive`, `warning`, `success` and `info` each come in three tokens: text (`*-foreground`), fill (base), and a tinted background (`*-surface`).
- **Rounded corners.** The base radius is 10 px, matching T3's `0.625rem`; use `md` 8, `sm` 6 and `xs` 4 for dense editor controls.
- **Editor chrome tokens (UX v2).** `chrome` paints the menu bar and the native caption (DWM on Windows); `separator-strong` is the window border and dock splitters; `panel-header` is the 28 px panel header; `raised` lifts controls one step off a panel; `hover-surface` is the neutral hover fill; `menu-surface` and `keycap` style menus and shortcut chips.
- **Selection is solid.** `selection-surface` (#2f62dc dark, #2a5bdb light) with `selection-foreground` text (`selection-muted-foreground` for paths and ids) keeps selected rows at 3:1 or better against panels; `selection-surface-inactive` marks selection in an unfocused panel.
- **Compact type.** Body text is about 13 px, with muted labels in `muted-foreground` and a monospace face for values and code. The font choice is still open; whatever we pick must be bundled with the engine.

## Rules

- **Panels and widgets use semantic tokens only.** No raw hex values in markup or stylesheets; `ara check` should flag them.
- **The same tokens drive game UI starter themes.** Games can override them.
- **Selection** of rows, menu items and segments uses `selection-surface` with `selection-foreground`; `accent-surface` stays for soft accents such as focus rings and badges.
- **Hover** is one step lighter than the surface underneath (`hover-surface` on rows and menus, `raised` on controls).
- **Agent-change overlays** (R03 review UX) use `success` for added, `warning` for modified and `destructive` for deleted, as tinted `*-surface` outlines in the viewport and hierarchy.

## Live editing

Debug editors watch `assets/themes/ara-dark.json`, `ara-light.json`, and the
stylesheets in `crates/ara-editor/src/shell/` every 250 ms on a worker. Save a
resource to restyle every open editor surface in place. Colors, radii and the
`metrics` object are CSS variables: `control-size` becomes `--control-size` in
pixels. The default Scene controls are 24 px; `viewport-inset`, `space-*`,
`font-size-*`, `panel-padding`, header and tab sizes control the shared chrome.
Panel styles remain in their own CSS files.

Invalid JSON, duplicate keys, negative metrics and invalid CSS keep the last good
snapshot; the editor reports the failure through its existing diagnostic path.
Fix and save to recover. Text fields keep their caret and draft when restyled.
Release builds embed the resources and never start the watcher. A debug binary
can use another source checkout by setting `ARA_EDITOR_STYLE_ROOT` to its root.

After editing JSON, use the canonical writer before committing:

```powershell
cargo run -p ara-editor --example format-theme -- assets/themes/ara-dark.json assets/themes/ara-light.json
```

When adding a panel stylesheet, register it in `Shell::stylesheet` with
`self.styles.css("panel.css", include_str!("panel.css"))`: debug uses the watched
version and release uses the embedded fallback. Keep inspector selectors in
`inspector.css` and viewport controls in `viewport.css`.
