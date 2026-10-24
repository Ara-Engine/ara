# The UI stack: markup, styles, text, runtime, rendering and inspection (ARA4000–ARA4199)

## ARA4001 ui::MARKUP: Markup does not parse
owner: ara-ui-document

The UI markup is not well-formed, so no element tree was built and nothing from this source is shown.

Causes:
- A tag is unclosed, misnested or closed twice.
- An attribute value is unquoted or a character is not escaped.

Fixes:
- Fix the reported line and column; close every element in order.
- Escape `<`, `&` and quotes in text with `escape()`.

## ARA4002 ui::ELEMENT: Unsupported element
owner: ara-ui-document

The markup names an element that is not a registered native UI primitive or component.

Causes:
- A typo in the tag name.
- An HTML tag the native UI does not implement.

Fixes:
- Use one of the native primitives (`panel`, `text`, `button`, `field`, `image`, `scroll`, ...) or register a component.

## ARA4003 ui::DEPTH: Markup nests too deeply
owner: ara-ui-document

Elements nest deeper than 128 levels, the limit that keeps layout and hit testing bounded.

Causes:
- Generated markup recurses without a bound.

Fixes:
- Split the document into components or flatten wrapper panels.

## ARA4004 ui::MISSING_ID: Element without an ID
owner: ara-ui-document

Every interactive or styled element needs an authored `id`: bindings, focus, inspection and agents address nodes by it.

Causes:
- A generated element omits its `id` attribute.

Fixes:
- Add a unique, stable `id` (derive it from the data the element shows, not its position).

## ARA4005 ui::DUPLICATE_ID: Duplicate element ID
owner: ara-ui-document

Two elements share an `id` (or two IDs hash to the same identity), or an expanded component produced no root.

Causes:
- A list repeats an ID prefix without a per-item key.
- A component template is empty.

Fixes:
- Include the item key in generated IDs.
- Give every component exactly one root element.

## ARA4006 ui::ATTRIBUTE: Invalid attribute
owner: ara-ui-document

An attribute is not supported on the element, or its value is invalid (a boolean that is not `true`/`false`, a bad tab index).

Causes:
- A typo in the attribute name.
- A value of the wrong type.

Fixes:
- Use a supported attribute with a valid value; see the element reference in `ara-ui-document`.

## ARA4007 ui::MIXED_TEXT: Mixed text content
owner: ara-ui-document

Text appears directly between elements; native UI text lives in `text` elements.

Causes:
- Inline text written HTML-style.

Fixes:
- Wrap the text in a `text` element and put it in the `text` attribute.

## ARA4008 ui::COMPONENT: Invalid component
owner: ara-ui-document

A component definition is invalid, refers to itself recursively, or a use site is missing a required slot.

Causes:
- A component template includes itself.
- A slot name does not match the definition.

Fixes:
- Break the recursion; fill every slot the component declares.

## ARA4010 ui::STYLESHEET: Stylesheet rule does not parse
owner: ara-ui-document

A stylesheet rule or selector is malformed or names an unknown element type.

Causes:
- Unsupported selector syntax (descendant or combinator selectors).
- A typo in a type selector.

Fixes:
- Use type, `.class` or `#id` selectors with at most one state.

## ARA4011 ui::PROPERTY: Unsupported style value
owner: ara-ui

A style property has a value the native UI does not support (for example an unknown `position` or alignment keyword).

Causes:
- A CSS keyword the native layout does not implement.

Fixes:
- Use one of the supported values listed in the message.

## ARA4012 ui::TOKEN: Unknown theme token
owner: ara-ui

A `var(--token)` names no token in the active theme.

Causes:
- A typo in the token name.
- A token that exists in only one theme.

Fixes:
- Use a token defined in both `assets/themes/ara-dark.json` and `ara-light.json`, or add it to both.

## ARA4013 ui::TOKEN_CYCLE: Theme token cycle
owner: ara-ui

Theme tokens refer to each other in a cycle or nest beyond the resolution limit.

Causes:
- Two tokens defined in terms of each other.

Fixes:
- Define one of the tokens with a literal colour.

## ARA4014 ui::INLINE_STYLE: Invalid inline style
owner: ara-ui

An inline `style` attribute holds a declaration the native UI rejects: an unknown property, an invalid unit, or a negative length.

Causes:
- A computed position or size became negative or not finite (for example a scrolled grid placing a row above its container).
- A property that only exists in web CSS.

Fixes:
- Clamp computed lengths at zero; move scrolled content with `translate-x`/`translate-y`, which may be negative.
- Use a supported property with `px` or `%` units.

## ARA4018 ui::RAW_COLOR: Raw colour in an editor style
owner: ara-ui

Editor styles and themes must use semantic tokens so both themes and game UI starter themes stay consistent.

Causes:
- A `#rrggbb` literal in an editor stylesheet or inline style.

Fixes:
- Replace the literal with a `var(--token)` from the theme.

## ARA4020 ui::EDIT: Markup edit does not apply
owner: ara-ui-document

A structured designer edit was computed against a different revision of the source, or targets a node that no longer exists.

Causes:
- The source changed after the edit was prepared.

Fixes:
- Reload the current source and regenerate the typed command.

## ARA4030 ui::FONT: Fonts missing or invalid
owner: ara-ui-text

The text system has no usable font: none was given, or the bytes contain no valid font face.

Causes:
- A font file is truncated or not a font.

Fixes:
- Bundle at least one valid font with the UI; check the font pack paths.

## ARA4031 ui::COMPOSITION: IME composition out of order
owner: ara-ui-text

An input-method composition event arrived out of order or with an out-of-range span.

Causes:
- A platform layer forwarded a commit without a preceding composition.

Fixes:
- Forward IME start/update/commit events in order; reset composition on focus loss.

## ARA4032 ui::GRAPHEME: Selection splits a grapheme
owner: ara-ui-text

A text selection boundary falls inside a grapheme cluster (an emoji sequence or a combining mark).

Causes:
- A caller computed selection offsets in bytes or code points.

Fixes:
- Move selections with the text system's cursor API, which steps by grapheme cluster.

## ARA4040 ui::NODE: Invalid node or viewport
owner: ara-ui

A node handle does not exist in the retained tree, or the viewport size is not finite and positive.

Causes:
- A handle from a previous mount was reused after a rebuild.
- A window reported a zero or NaN size.

Fixes:
- Look nodes up again by ID after each mount; skip frames while minimized.

## ARA4041 ui::VIRTUAL: Invalid virtual list request
owner: ara-ui

A virtualized tree or table was given duplicate keys, a wrong column count, a non-finite scroll, or more rows than its pool.

Causes:
- Keys collide in the source model.
- The mount target scroll node is not empty.

Fixes:
- Give every row a unique key; mount virtual views into empty scroll nodes.

## ARA4042 ui::DOCKING: Invalid docking operation
owner: ara-ui

A docking operation names a panel, area or floating window that is not open, or an invalid split ratio, path or window size.

Causes:
- A saved layout refers to a panel that no longer exists.
- A drop target went away during a drag.

Fixes:
- Reset the layout (Window > Layouts > Reset Layout) or open the panel first.

## ARA4043 ui::BINDING: Binding node missing
owner: ara-ui

A data binding refers to a node that is not in the tree.

Causes:
- The markup changed and dropped the bound element.

Fixes:
- Re-create the binding after the mount that adds its node.

## ARA4044 ui::WIDGET: Invalid widget operation
owner: ara-ui

A widget operation does not fit the control: wrong control kind, an out-of-range choice, or a non-finite value.

Causes:
- A boolean bound to a non-toggle.
- A slider given min > max.

Fixes:
- Bind values to the matching control kind and keep ranges and values finite.

## ARA4045 ui::INPUT: Focus or capture rule violated
owner: ara-ui

Focus moved to a disabled, clipped or out-of-scope control, or a drag started without pointer capture.

Causes:
- A modal scope is active and focus targets the page behind it.

Fixes:
- Focus controls inside the active scope; capture the pointer before dragging.

## ARA4046 ui::LAYER: Invalid overlay layer
owner: ara-ui

An overlay layer is already open, positioned invalidly, or not mounted under the UI root.

Causes:
- The same popup opened twice.

Fixes:
- Close a layer before reopening it; mount overlays under the root.

## ARA4050 ui::ATLAS: Glyph atlas request invalid
owner: ara-ui-render

A glyph or atlas request is outside the supported limits: atlas size outside 64-4096, a glyph larger than an atlas page, or a scalable glyph without an outline.

Causes:
- A very large font size.
- A corrupt font.

Fixes:
- Use a larger atlas or a smaller raster scale; replace the font.

## ARA4051 ui::GPU_RESOURCE: Invalid GPU UI resource
owner: ara-ui-render

A draw refers to an image or render target that was not created, or a glyph arena range outside the frame.

Causes:
- An image was drawn before it was uploaded.

Fixes:
- Upload images before drawing them; rebuild the display list after resizing.

## ARA4052 ui::REFERENCE: CPU reference renderer misuse
owner: ara-ui-render

The CPU reference renderer was used out of order (draw before begin, missing texture) or with invalid dimensions or clips.

Causes:
- A test reads pixels before rendering a frame.

Fixes:
- Begin a frame, upload textures, draw, then read pixels.

## ARA4100 ui::OVERFLOW: Box overflows its parent or the viewport
owner: ara-ui-test

UI inspection found an element extending beyond its parent or the viewport at the inspected size and locale.

Causes:
- Fixed widths that do not fit longer translations or small windows.

Fixes:
- Allow wrapping, enlarge the container, use flexible tracks or add intentional scrolling.

## ARA4101 ui::TEXT_OVERFLOW: Text exceeds its bounds
owner: ara-ui-test

Text is wider or taller than its content box or clip at the inspected size and locale.

Causes:
- Single-line text with a long translation.

Fixes:
- Allow multiline text, expand the box or scroll; keep the accessible font size.

## ARA4102 ui::MISSING_GLYPHS: Missing glyphs
owner: ara-ui-test

Some characters have no glyph in the bundled fonts and render as boxes.

Causes:
- A locale whose script is not covered by the font pack.

Fixes:
- Supply a licensed font pack covering the reported clusters.

## ARA4103 ui::FALLBACK_FONT: Text uses a fallback font
owner: ara-ui-test

Shaping fell back to a secondary font for some text (information).

Causes:
- The primary font lacks the script.

Fixes:
- Keep the required fallback pack, or choose a primary font covering the script.

## ARA4104 ui::CONTRAST: Measured text contrast
owner: ara-ui-test

The contrast ratio of text over a solid background, reported for passes and failures alike.

Causes:
- Text and background tokens are too close.

Fixes:
- Choose foreground/background tokens with at least 4.5:1 (3:1 for large text and selection fills).

## ARA4105 ui::CONTRAST_UNCERTAIN: Contrast cannot be measured
owner: ara-ui-test

Text sits over imagery or a transparent surface, so no single contrast ratio is defensible.

Causes:
- Labels drawn directly over a game frame or thumbnail.

Fixes:
- Put a solid scrim behind the text, or inspect representative imagery.

## ARA4106 ui::TOUCH_TARGET: Touch target too small
owner: ara-ui-test

An interactive element is smaller than 44 x 44 logical pixels where touch input is expected.

Causes:
- Dense desktop controls reused for touch layouts.

Fixes:
- Increase padding or minimum size to 44 x 44 logical pixels on touch layouts.
