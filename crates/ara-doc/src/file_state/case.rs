//! Filesystem comparison only: paths in documents and ops keep their original spelling.

#[cfg(windows)]
pub(super) fn key(path: &str) -> String {
    windows_key(path)
}

// Conservatively combine Unicode simple uppercase and non-Turkic simple folding.
// Uppercasing includes ordinal aliases such as dotless i/I; simple folding also
// includes sigma/final sigma and capital sharp S. Expansions and normalization
// are deliberately absent, as in NTFS's UTF-16 upcase comparisons.
#[cfg(any(windows, test))]
fn windows_key(path: &str) -> String {
    use unicode_casefold::{Locale, UnicodeCaseFold, Variant};
    path.chars()
        .map(|ch| {
            let mut upper = ch.to_uppercase();
            let first = upper.next().expect("uppercase mapping is nonempty");
            if upper.next().is_none() { first } else { ch }
        })
        .case_fold_with(Variant::Simple, Locale::NonTurkic)
        .collect()
}

#[cfg(target_os = "macos")]
pub(super) fn key(path: &str) -> String {
    apfs_key(path)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn key(path: &str) -> String {
    path.to_owned()
}

// APFS uses Unicode 9 case folding and canonical equivalence. Using these keys
// conservatively on case-sensitive macOS volumes also refuses potential aliases.
#[cfg(any(target_os = "macos", test))]
fn apfs_key(path: &str) -> String {
    use unicode_casefold::UnicodeCaseFold;
    use unicode_normalization::UnicodeNormalization;
    path.nfd().case_fold().nfd().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn apfs_folding_covers_sigma_expansions_and_canonical_equivalence() {
        assert_eq!(unicode_casefold::UNICODE_VERSION, (9, 0, 0));
        for (a, b) in [
            ("Σ", "ς"),
            ("σ", "ς"),
            ("ß", "SS"),
            ("é", "e\u{301}"),
            ("İ", "i\u{307}"),
        ] {
            assert_eq!(apfs_key(a), apfs_key(b), "{a} / {b}");
            assert_eq!(apfs_key(&apfs_key(a)), apfs_key(a));
        }
        assert_ne!(apfs_key("İ"), apfs_key("i"));
        assert_ne!(apfs_key("ı"), apfs_key("I"));
    }

    #[test]
    fn windows_simple_keys_are_idempotent_and_cover_every_simple_uppercase_pair() {
        use unicode_casefold::{Locale, UnicodeCaseFold, Variant};
        for ch in (0..=char::MAX as u32).filter_map(char::from_u32) {
            let original = ch.to_string();
            let key = windows_key(&original);
            assert_eq!(windows_key(&key), key, "U+{:04X}", ch as u32);
            let mut upper = ch.to_uppercase();
            let first = upper.next().expect("uppercase mapping");
            if upper.next().is_none() {
                assert_eq!(
                    windows_key(&first.to_string()),
                    key,
                    "upper U+{:04X}",
                    ch as u32
                );
            }
            let folded: String = ch
                .case_fold_with(Variant::Simple, Locale::NonTurkic)
                .collect();
            assert_eq!(windows_key(&folded), key, "fold U+{:04X}", ch as u32);
        }
    }
}
