# Native component stories

`ara-ui-stories` owns native component examples and fixture data. `ara-ui-test` runs
them through Ara's real retained layout, Parley text, input, AccessKit and CPU renderer.
The same library seams are ready for the editor UI Designer and Engine Protocol adapters.

A `.stories` file is an ordinary Ara document with `$schema: ara://schema/stories/1`,
a `us_` asset ID and flat `st_` records. Register it with `register(&mut SchemaDb)`.
`StoryBook::read` checks strict JSON (including duplicate keys), the registered schema,
fixtures, locales, references, bindings and capture dimensions. `StoryBook::edit(Edit)`
validates atomically and returns a reversible `ara_doc::Op`; the host wraps these in
attributed transactions. `StoryBook::write` delegates to the canonical writer.

Each `Story` names its `component` (a primitive or `@path#assetId`), embeds native
`markup` and token-only `stylesheet`, and declares a registered `fixtureType` plus
immutable fixture values. `bind='text'`, `bind='checked'` and `bind='value'` initialize
the corresponding native destinations after validating them against the fixture schema.
Hosts may register their own fixture model types before reading a story book.

`locales` contains complete message packs. A caption such as `text='loc:resume'`
resolves to a localized string before template expansion. `pseudo: true` accents and
expands captions by 35% (plus brackets), and `rtl: true` requests a layout mirror pass
in the capture/test harness. IDs, actions, bindings and model data remain stable.
Missing translations are errors. Named locales do not silently fall back to English.

Every `views` entry explicitly fixes `width`, `height`, `dpi`, `locale`, `input`, `theme`
and `timeMs`. Physical PNG dimensions are `round(logical * dpi / 96)`. Views describe
representative configurations explicitly; there is no hidden Cartesian expansion.
Transitions sample paint translation tracks from this clock. Zero duration is a step.

`steps` contains semantic assertions and interactions (`visible`, `focus`, `focused`,
`activate`, `text`, `value`, `visibleText`, `state`, `action`, `advance`). Queries select
`role`, `label`, `text` or `testId`; localized expectations can also use `loc:key`.
Activation uses the view's chosen mouse/keyboard/gamepad mode and the native input seam.
`images` supplies decoded RGBA fixture resources; production hosts resolve their own images.

The [minimal button story](../ara-ui-test/examples/button.stories),
[complete catalogue](../ara-ui-test/examples/catalogue.stories) and
[generated schema](../ara-ui-test/schema/stories.schema.json) are canonical examples.
The catalogue covers all native source primitives, template/slot projection, inspector,
dock, hierarchy, console, pause, inventory and HUD. Six views per story cover Dark/Light,
English/Arabic RTL/pseudo, 96/144 DPI, two sizes, all input modes and fixed animation times.

Run `CARGO_BUILD_JOBS=2 cargo test -p ara-ui-stories -p ara-ui-test`.
See [capture and semantic testing](../ara-ui-test/README.md) for the agent API and evidence.
