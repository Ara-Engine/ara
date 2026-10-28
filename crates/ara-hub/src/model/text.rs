//! Display formatting shared by the views: relative times, paths and names.
use std::path::Path;

/// "just now", "5 min ago", "yesterday", "3 days ago", then a `YYYY-MM-DD` date.
pub fn relative(then_ms: Option<u64>, now_ms: u64) -> String {
    let Some(then) = then_ms else {
        return "Never opened".into();
    };
    let minutes = now_ms.saturating_sub(then) / 60_000;
    match minutes {
        0 => "Just now".into(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{} h ago", minutes / 60),
        1440..2880 => "Yesterday".into(),
        2880..43_200 => format!("{} days ago", minutes / 1440),
        _ => date(then),
    }
}

/// Civil UTC date of a Unix-millisecond instant (Howard Hinnant's algorithm).
fn date(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// A path for people: no `\\?\` verbatim prefix.
pub fn path(path: &Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\")
        .map_or(text.clone(), str::to_owned)
}

/// Display an executable's directory across fixture platforms; never use this for I/O.
pub fn executable_directory(executable: &Path) -> String {
    let text = path(executable);
    match text.rfind(['/', '\\']) {
        Some(0) => text[..1].into(),
        Some(2) if text.as_bytes().get(1) == Some(&b':') => text[..3].into(),
        Some(index) => text[..index].into(),
        None => text,
    }
}

/// Shorten to at most `max` characters, keeping both ends (`C:\Users\…\Outpost`).
pub fn middle(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max || max < 5 {
        return text.to_owned();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start}…{end}")
}

/// Shorten to at most `max` characters, keeping the start.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Up to two initials for a thumbnail fallback (`Outpost` → `O`, `My game` → `MG`).
pub fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

/// Stable palette slot 1–6 for a fallback tile, from the project ID.
pub fn tint(id: &str) -> usize {
    id.bytes()
        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b))) as usize
        % 6
        + 1
}

/// The first `base`, `base 2`, `base 3`… accepted by `taken`.
pub fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    (1..)
        .map(|n| {
            if n == 1 {
                base.to_owned()
            } else {
                format!("{base} {n}")
            }
        })
        .find(|name| !taken(name))
        .unwrap_or_else(|| base.to_owned())
}

/// A folder name for a project name: Windows-reserved characters become `-`.
pub fn folder(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    cleaned.trim_end_matches(['.', ' ']).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times_read_like_a_launcher() {
        let now = 1_700_000_000_000;
        assert_eq!(relative(None, now), "Never opened");
        assert_eq!(relative(Some(now - 30_000), now), "Just now");
        assert_eq!(relative(Some(now - 5 * 60_000), now), "5 min ago");
        assert_eq!(relative(Some(now - 3 * 3_600_000), now), "3 h ago");
        assert_eq!(relative(Some(now - 30 * 3_600_000), now), "Yesterday");
        assert_eq!(relative(Some(now - 4 * 86_400_000), now), "4 days ago");
        assert_eq!(relative(Some(1_600_000_000_000), now), "2020-09-13");
    }

    #[test]
    fn paths_and_names_stay_readable() {
        assert_eq!(
            path(Path::new(r"\\?\C:\Games\Outpost")),
            r"C:\Games\Outpost"
        );
        assert_eq!(middle("abcdefghij", 7), "abc…hij");
        assert_eq!(initials("my  game project"), "MG");
        assert_eq!(folder(" Space: Odyssey? "), "Space- Odyssey-");
        assert_eq!(unique("My project", |n| n == "My project"), "My project 2");
    }

    #[test]
    fn executable_directories_are_visible_across_capture_platforms() {
        assert_eq!(
            executable_directory(Path::new(r"C:\Users\tester\Ara\0.0.2\ara.exe")),
            r"C:\Users\tester\Ara\0.0.2"
        );
        assert_eq!(
            executable_directory(Path::new("/opt/ara/0.0.2/ara")),
            "/opt/ara/0.0.2"
        );
        assert_eq!(executable_directory(Path::new(r"C:\ara.exe")), r"C:\");
        assert_eq!(executable_directory(Path::new("/ara")), "/");
    }
}
