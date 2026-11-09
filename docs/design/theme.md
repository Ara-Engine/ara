# Editor theme

Ara's editor and Hub ship two themes in the **satin graphite** look (D31):
[`assets/themes/ara-dark.json`](../../assets/themes/ara-dark.json) (the default) and
[`assets/themes/ara-light.json`](../../assets/themes/ara-light.json). The look was
iterated in the `ara-UI` web mock (its `/kit` page, section 9 "Satin tactile"); that
mock's theme files are the token source and are copied here verbatim.

## The look

- **Graphite surfaces.** `chrome` is the window, `card` the docks, `background` the
  canvases. Steps between them are small; depth does the rest.
- **White primary ink.** `primary` is near-white in dark and near-black in light. It
  marks the one main action, on states (checks, toggles, slider fills) and focus.
- **Blue is an accent.** `brand`, `accent`, `info`, links and text selection keep the
  Ara blue. It never marks the chosen item.
- **Depth carries state; no stripes.** Selection, the front tab and an active tool lift
  out (raised) or press in; never a bar, underline or rail.
- **Status colours.** `destructive`, `warning`, `success` and `info` each come in text
  (`*-foreground`), fill (base) and tinted background (`*-surface`) tokens.
- **Rounded corners.** `md` 8, `sm` 6 and `xs` 4 for dense controls; docks use the base.

## Materials

Six flat materials, one recipe for both themes. Every value is a token; the native
dialect draws `box-shadow` and `backdrop-filter` exactly as the web mock does.

| Material | Use | Recipe |
|---|---|---|
| Well | fields, tracks, code | `background:var(--well); box-shadow:inset 0 1px 0 var(--well-shade), 0 1px 0 var(--well-lip)` |
| Raised | buttons, pop-ups, keycaps, segment thumbs, the chosen row | `background:var(--raised); box-shadow:inset 0 1px 0 var(--edge-highlight), inset 0 -1px 0 var(--edge-lowlight), 0 0 0 1px var(--edge-shadow)`; hover `--raised-hover`; active presses in like Pressed |
| Pressed | latched toggles, the active tool | `background:var(--well); box-shadow:inset 0 1px 1px var(--well-shade), inset 0 0 0 1px var(--edge-lowlight), 0 1px 0 var(--well-lip)` |
| Primary | the one main action, on states | `background:var(--primary); color:var(--primary-foreground); box-shadow:inset 0 1px 0 var(--primary-highlight), inset 0 -1px 0 var(--primary-lowlight), 0 0 0 1px var(--edge-shadow)` |
| Float | menus, toasts, dialogs (the only glass) | `background:var(--menu-surface); backdrop-filter:blur(20px) saturate(1.2); box-shadow:inset 0 1px 0 var(--edge-highlight), inset 0 0 0 1px var(--float-ring), 0 0 0 1px var(--edge-shadow), 0 14px 34px var(--float-shadow)` |
| Dock | panels | `background:var(--card); box-shadow:inset 0 1px 0 var(--dock-highlight), 0 0 0 1px var(--edge-shadow), 0 4px 14px var(--shadow)` |

Keyboard focus is a soft halo outside the control: `0 0 0 1px` of the surface colour
under `0 0 0 3px var(--focus-halo)`.

## Rules

- **Panels and widgets use semantic tokens only.** No raw hex in markup or stylesheets.
- **The same tokens drive game UI starter themes.** Games can override them.
- **Selection** of rows, tabs and segments lifts the item as Raised with
  `selection-foreground`; `selection-surface-inactive` marks it in an unfocused panel.
- **Hover** is one step up: `hover-surface` on rows and menus, `raised-hover` on controls.
- **Agent-change overlays** (R03 review UX) use `success` for added, `warning` for
  modified and `destructive` for deleted, as tinted `*-surface` outlines.

## Live editing

Debug editors and the Hub watch `assets/themes/ara-dark.json`, `ara-light.json` and their
stylesheets every 250 ms on a worker (`cargo run -p ara-dev -- live hub|editor` keeps
one open; `cargo run -p ara-ui --example check_styles -- <css...>` validates sheets). Save a
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

When adding a panel stylesheet, declare it with `sheet!` in `live_styles::embedded` and
register it in `Shell::stylesheet` with `self.styles.css("panel.css", embedded::PANEL)`.
Release builds compile it in; debug builds compile no shell CSS (a CSS edit never
recompiles the editor) and read the checkout at startup and on every save. Keep inspector selectors in
`inspector.css` and viewport controls in `viewport.css`.
