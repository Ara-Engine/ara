//! Physical-key chords, keycap labels and keymap presets for the command registry.
//!
//! Chords use SDL scancodes, so a binding means the same physical key on every
//! layout. `Keymap::new` layers a preset and the user's overrides over the
//! registry's default (Ara) shortcuts.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One physical key plus modifiers, e.g. `Ctrl+Shift+Z`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// SDL scancode.
    pub key: i32,
}

/// Scancode names, in the spelling used by shortcut strings and keycaps.
const KEYS: &[(&str, i32)] = &[
    ("A", 4),
    ("B", 5),
    ("C", 6),
    ("D", 7),
    ("E", 8),
    ("F", 9),
    ("G", 10),
    ("H", 11),
    ("I", 12),
    ("J", 13),
    ("K", 14),
    ("L", 15),
    ("M", 16),
    ("N", 17),
    ("O", 18),
    ("P", 19),
    ("Q", 20),
    ("R", 21),
    ("S", 22),
    ("T", 23),
    ("U", 24),
    ("V", 25),
    ("W", 26),
    ("X", 27),
    ("Y", 28),
    ("Z", 29),
    ("1", 30),
    ("2", 31),
    ("3", 32),
    ("4", 33),
    ("5", 34),
    ("6", 35),
    ("7", 36),
    ("8", 37),
    ("9", 38),
    ("0", 39),
    ("Enter", 40),
    ("Esc", 41),
    ("Backspace", 42),
    ("Tab", 43),
    ("Space", 44),
    ("-", 45),
    ("=", 46),
    ("[", 47),
    ("]", 48),
    ("\\", 49),
    (";", 51),
    ("'", 52),
    ("`", 53),
    (",", 54),
    (".", 55),
    ("/", 56),
    ("F1", 58),
    ("F2", 59),
    ("F3", 60),
    ("F4", 61),
    ("F5", 62),
    ("F6", 63),
    ("F7", 64),
    ("F8", 65),
    ("F9", 66),
    ("F10", 67),
    ("F11", 68),
    ("F12", 69),
    ("Pause", 72),
    ("Insert", 73),
    ("Home", 74),
    ("PageUp", 75),
    ("Del", 76),
    ("End", 77),
    ("PageDown", 78),
    ("Right", 79),
    ("Left", 80),
    ("Down", 81),
    ("Up", 82),
    ("Num/", 84),
    ("Num*", 85),
    ("Num-", 86),
    ("Num+", 87),
    ("NumEnter", 88),
    ("Num1", 89),
    ("Num2", 90),
    ("Num3", 91),
    ("Num4", 92),
    ("Num5", 93),
    ("Num6", 94),
    ("Num7", 95),
    ("Num8", 96),
    ("Num9", 97),
    ("Num0", 98),
    ("Num.", 99),
    ("Menu", 101),
];

impl Chord {
    /// Parse `Ctrl+Shift+Z`; modifiers may come in any order. Unknown keys fail.
    pub fn parse(text: &str) -> Option<Self> {
        let mut chord = Self {
            ctrl: false,
            shift: false,
            alt: false,
            key: 0,
        };
        // `+` is not a key name, so a plain split is unambiguous.
        for part in text.split('+').map(str::trim) {
            match part {
                "Ctrl" => chord.ctrl = true,
                "Shift" => chord.shift = true,
                "Alt" => chord.alt = true,
                key => chord.key = KEYS.iter().find(|(n, _)| *n == key)?.1,
            }
        }
        (chord.key != 0).then_some(chord)
    }
    /// Keycap labels in display order: modifiers, then the key.
    pub fn caps(self) -> Vec<&'static str> {
        let mut caps = Vec::new();
        if self.ctrl {
            caps.push("Ctrl");
        }
        if self.shift {
            caps.push("Shift");
        }
        if self.alt {
            caps.push("Alt");
        }
        caps.push(
            KEYS.iter()
                .find(|(_, code)| *code == self.key)
                .map_or("?", |(n, _)| n),
        );
        caps
    }
    /// The canonical shortcut string, e.g. `Ctrl+Shift+Z`.
    pub fn text(self) -> String {
        self.caps().join("+")
    }
}
/// Parse a registry shortcut list; alternatives are separated by ` / `.
pub(crate) fn parse_list(text: &str) -> Vec<Chord> {
    text.split(" / ").filter_map(Chord::parse).collect()
}

/// Keymap preset chosen in Settings › Keyboard shortcuts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Preset {
    /// The registry's own defaults.
    #[default]
    Ara,
    /// Unreal Engine 5 bindings for shared concepts.
    Unreal,
    /// Unity 6 bindings for shared concepts.
    Unity,
    /// Blender 4 bindings for shared concepts.
    Blender,
}
impl Preset {
    pub const ALL: [Self; 4] = [Self::Ara, Self::Unreal, Self::Unity, Self::Blender];
    pub fn name(self) -> &'static str {
        match self {
            Self::Ara => "ara",
            Self::Unreal => "unreal",
            Self::Unity => "unity",
            Self::Blender => "blender",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Ara => "Ara",
            Self::Unreal => "Unreal",
            Self::Unity => "Unity",
            Self::Blender => "Blender",
        }
    }
    pub fn parse(name: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|p| p.name() == name)
            .unwrap_or_default()
    }
    /// Command bindings that differ from Ara's. An empty string unbinds.
    pub fn overrides(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Ara => &[],
            Self::Unreal => &[
                ("play.start", "Alt+P / F5"),
                ("play.simulate", "Alt+S"),
                ("play.pause", "Pause / F6"),
                ("play.stop", "Esc / Shift+F5"),
                ("play.eject", "F8"),
                ("edit.duplicate", "Ctrl+D / Ctrl+W"),
                ("edit.rename", "F2"),
                ("view.game", "G"),
                ("view.frame_all", "Home"),
                ("edit.transform.drop", "End"),
                ("window.assets", "Ctrl+Space"),
                ("window.console", "`"),
                ("edit.select_all", "Ctrl+A"),
                ("view.tool.select", "Q"),
            ],
            Self::Unity => &[
                ("play.start", "Ctrl+P"),
                ("play.pause", "Ctrl+Shift+P"),
                ("play.step", "Ctrl+Alt+P"),
                ("play.stop", "Shift+F5"),
                ("create.empty", "Ctrl+Shift+N"),
                ("create.empty_child", "Alt+Shift+N"),
                ("edit.duplicate", "Ctrl+D"),
                ("edit.rename", "F2"),
                ("view.frame", "F"),
                ("view.tool.select", "Q"),
                ("window.inspector", "Ctrl+3"),
                ("window.hierarchy", "Ctrl+4"),
                ("window.assets", "Ctrl+5"),
                ("window.console", "Ctrl+Shift+C"),
                ("edit.copy_reference", ""),
                ("window.maximize", "Shift+Space"),
            ],
            Self::Blender => &[
                ("play.start", "Shift+Ctrl+Space"),
                ("view.tool.move", "G"),
                ("view.tool.rotate", "R"),
                ("view.tool.scale", "S"),
                ("create.at_cursor", "Shift+A"),
                ("edit.delete", "X / Del"),
                ("edit.duplicate", "Shift+D"),
                ("view.frame", "Num."),
                ("view.frame_all", "Home"),
                ("view.camera.front", "Num1"),
                ("view.camera.right", "Num3"),
                ("view.camera.top", "Num7"),
                ("edit.select_all", "A"),
                ("edit.deselect", "Alt+A"),
                ("edit.hide", "H"),
                ("window.maximize", "Ctrl+Space"),
                ("edit.rename", "F2"),
                ("edit.palette", "F3 / Ctrl+K"),
            ],
        }
    }
}

/// Resolved bindings: preset over defaults, then per-command user overrides.
#[derive(Clone, Debug, Default)]
pub(crate) struct Keymap {
    by_chord: BTreeMap<Chord, Vec<&'static str>>,
    by_command: BTreeMap<&'static str, Vec<Chord>>,
}
impl Keymap {
    /// `defaults` yields `(command id, Ara shortcut list)` for every registry command.
    pub fn new<'a>(
        defaults: impl Iterator<Item = (&'static str, &'a str)>,
        preset: Preset,
        overrides: &BTreeMap<String, String>,
    ) -> Self {
        let mut map = Self::default();
        for (id, keys) in defaults {
            let keys = overrides
                .get(id)
                .map(String::as_str)
                .or_else(|| {
                    preset
                        .overrides()
                        .iter()
                        .find(|(c, _)| *c == id)
                        .map(|(_, k)| *k)
                })
                .unwrap_or(keys);
            let chords = parse_list(keys);
            for chord in &chords {
                map.by_chord.entry(*chord).or_default().push(id);
            }
            map.by_command.insert(id, chords);
        }
        map
    }
    /// Commands bound to a chord; more than one means a conflict.
    pub fn commands(&self, chord: Chord) -> &[&'static str] {
        self.by_chord.get(&chord).map_or(&[], Vec::as_slice)
    }
    /// A command's bindings, primary first.
    pub fn chords(&self, id: &str) -> &[Chord] {
        self.by_command.get(id).map_or(&[], Vec::as_slice)
    }
    /// Other commands sharing any of `id`'s chords.
    pub fn conflicts(&self, id: &str) -> Vec<&'static str> {
        let mut out: Vec<_> = self
            .chords(id)
            .iter()
            .flat_map(|c| self.commands(*c).iter().copied())
            .filter(|other| *other != id)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chords_round_trip_and_alternatives_parse() {
        let chord = Chord::parse("Shift+Ctrl+Z").expect("valid chord");
        assert_eq!(chord.text(), "Ctrl+Shift+Z");
        assert_eq!(parse_list("Ctrl+Y / Ctrl+Shift+Z").len(), 2);
        assert!(Chord::parse("Ctrl+Nope").is_none());
        assert_eq!(Chord::parse("Num.").map(|c| c.key), Some(99));
    }
    #[test]
    fn presets_and_overrides_layer_over_defaults() {
        let defaults = [("play.start", "F5"), ("edit.duplicate", "Ctrl+D")];
        let unity = Keymap::new(defaults.into_iter(), Preset::Unity, &BTreeMap::new());
        assert_eq!(unity.chords("play.start")[0].text(), "Ctrl+P");
        let user = BTreeMap::from([("play.start".to_owned(), "Ctrl+D".to_owned())]);
        let map = Keymap::new(defaults.into_iter(), Preset::Ara, &user);
        assert_eq!(map.conflicts("play.start"), ["edit.duplicate"]);
    }
}
