//! Every diagnostic number of the UI stack (`ARA4000`–`ARA4199`): markup and styles
//! here, text in `ara-ui-text`, runtime in `ara-ui`, rendering in `ara-ui-render`,
//! inspection in `ara-ui-test`. Each has an entry in `crates/ara-diag/codes/ui.md`;
//! the `ara-diag` coverage test reads the `NAME = number,` lines below.

macro_rules! codes {
    ($($(#[$doc:meta])* $name:ident = $n:literal,)*) => {
        $($(#[$doc])* pub const $name: u16 = $n;)*
        /// Every UI code, ascending.
        pub const ALL: &[u16] = &[$($n),*];
    };
}

codes! {
    /// Markup does not parse.
    MARKUP = 4001,
    /// An element is not a registered native UI primitive.
    ELEMENT = 4002,
    /// Markup nests deeper than 128 elements.
    DEPTH = 4003,
    /// An element has no authored `id`.
    MISSING_ID = 4004,
    /// Two elements share an ID (or hash), or an expanded component has no root.
    DUPLICATE_ID = 4005,
    /// An attribute is unsupported or has an invalid value.
    ATTRIBUTE = 4006,
    /// Text content mixed with elements.
    MIXED_TEXT = 4007,
    /// A component is invalid, recursive or misses a slot.
    COMPONENT = 4008,
    /// A stylesheet rule or selector does not parse or names an unknown element.
    STYLESHEET = 4010,
    /// A style property has an unsupported value.
    PROPERTY = 4011,
    /// A `var(--token)` names no theme token.
    TOKEN = 4012,
    /// Theme tokens reference each other in a cycle or too deeply.
    TOKEN_CYCLE = 4013,
    /// An inline `style` attribute has an invalid declaration.
    INLINE_STYLE = 4014,
    /// A raw colour in an editor style instead of a semantic token.
    RAW_COLOR = 4018,
    /// A structured markup edit does not apply to the current source.
    EDIT = 4020,
    /// Fonts are missing or invalid.
    FONT = 4030,
    /// An IME composition is out of order or out of range.
    COMPOSITION = 4031,
    /// A text selection splits a grapheme cluster.
    GRAPHEME = 4032,
    /// A node handle or viewport is invalid.
    NODE = 4040,
    /// A virtualized list/tree request is invalid.
    VIRTUAL = 4041,
    /// A docking operation names a missing panel, area or window, or an invalid split.
    DOCKING = 4042,
    /// A binding's node is missing.
    BINDING = 4043,
    /// A widget operation does not fit the control (wrong kind, range or value).
    WIDGET = 4044,
    /// Focus, scope or pointer capture rules were violated.
    INPUT = 4045,
    /// An overlay layer is invalid.
    LAYER = 4046,
    /// A glyph atlas or scalable glyph request is invalid.
    ATLAS = 4050,
    /// A GPU UI resource (image, target, glyph range) is invalid.
    GPU_RESOURCE = 4051,
    /// The CPU reference renderer was used out of order or with invalid input.
    REFERENCE = 4052,
    /// Inspection: a box overflows its parent or the viewport.
    OVERFLOW = 4100,
    /// Inspection: text exceeds its content or clip bounds.
    TEXT_OVERFLOW = 4101,
    /// Inspection: glyphs are missing for some text.
    MISSING_GLYPHS = 4102,
    /// Inspection: text renders with a fallback font.
    FALLBACK_FONT = 4103,
    /// Inspection: measured text contrast.
    CONTRAST = 4104,
    /// Inspection: contrast cannot be measured over imagery or transparency.
    CONTRAST_UNCERTAIN = 4105,
    /// Inspection: a touch target is smaller than 44 × 44 logical pixels.
    TOUCH_TARGET = 4106,
}
