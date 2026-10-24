# Scene viewport quality presets

`menu-lighting-{dark,light}.png`: the Scene view's Lit menu open at 1440x900 (1x), from
the `--verify-ux` route's `menu-lighting` step (offscreen, RTX 4070 SUPER). The menu
lists the project's default (with what it resolves to on this GPU), Auto and the four
presets; the active one is checked. The toolbar reads `Lit · High` and the status bar
`Auto · High`. The choice is the user preference `viewport.quality`
(`EditorState::render_quality`); the project's default is `rendering.quality` in
ara.project.json (Project Settings > Rendering).
