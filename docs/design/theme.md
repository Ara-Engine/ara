# Editor theme

Ara's editor ships two default themes that match **T3 Code's default dark and light palettes**: [`assets/themes/ara-dark.json`](../../assets/themes/ara-dark.json) (the default) and [`assets/themes/ara-light.json`](../../assets/themes/ara-light.json). The values were read from T3 Code's live theme variables on 2026-10-23.

## The look

- **Near-black neutral surfaces.** Dark mode uses `background` #0a0a0a for the app and `card`/`muted` #111111 for panels.
- **Barely-there borders.** `border` is #191919 in dark mode and #e4e4e7 in light mode. Depth comes from small surface steps, not lines or shadows.
- **One blue accent.** `primary`/`ring` is #346bf1 in dark mode and #1b4ed8 in light mode. Use it for focus, selection and primary actions.
- **Status colours.** `destructive`, `warning`, `success` and `info` each come in three tokens: text (`*-foreground`), fill (base), and a tinted background (`*-surface`).
- **Rounded corners.** The base radius is 10 px, matching T3's `0.625rem`; use `md` 8, `sm` 6 and `xs` 4 for dense editor controls.
- **Compact type.** Body text is about 13 px, with muted labels in `muted-foreground` and a monospace face for values and code. The font choice is still open; whatever we pick must be bundled with the engine.

## Rules

- **Panels and widgets use semantic tokens only.** No raw hex values in markup or stylesheets; `ara check` should flag them.
- **The same tokens drive game UI starter themes.** Games can override them.
- **Selection** uses `accent-surface` with an `accent` outline or text.
- **Hover** is one step lighter than the surface underneath.
- **Agent-change overlays** (R03 review UX) use `success` for added, `warning` for modified and `destructive` for deleted, as tinted `*-surface` outlines in the viewport and hierarchy.
