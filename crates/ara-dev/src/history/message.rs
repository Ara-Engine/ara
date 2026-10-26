//! Byte-preserving line removal: unchanged messages never get normalized.
use anyhow::{Result, bail};

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|v| v == needle)
}

fn word(text: &[u8], name: &[u8]) -> bool {
    text.windows(name.len()).enumerate().any(|(i, part)| {
        part == name
            && (i == 0 || !text[i - 1].is_ascii_alphanumeric())
            && text
                .get(i + name.len())
                .is_none_or(|v| !v.is_ascii_alphanumeric())
    })
}

fn attribution(line: &[u8]) -> Result<bool> {
    let lower = line.trim_ascii().to_ascii_lowercase();
    if lower.starts_with(b"co-authored-by:") {
        return Ok(word(&lower, b"claude") || word(&lower, b"anthropic"));
    }
    let generated = lower
        .strip_prefix("🤖".as_bytes())
        .unwrap_or(&lower)
        .trim_ascii_start();
    if let Some(tool) = generated.strip_prefix(b"generated with ")
        && (tool.starts_with(b"[claude code]")
            || tool
                .strip_prefix(b"claude code")
                .is_some_and(|rest| rest.first().is_none_or(|v| !v.is_ascii_alphanumeric())))
    {
        return Ok(true);
    }
    if contains(&lower, b"claude.ai/code") {
        let link = lower.strip_prefix(b"session: ").unwrap_or(&lower);
        let standalone = (link.starts_with(b"https://claude.ai/code")
            || link.starts_with(b"http://claude.ai/code")
            || link.starts_with(b"claude.ai/code"))
            && !link.iter().any(u8::is_ascii_whitespace);
        let markdown = link.starts_with(b"[")
            && link.ends_with(b")")
            && (contains(link, b"](https://claude.ai/code")
                || contains(link, b"](http://claude.ai/code"))
            && !contains(link, b") ");
        if !standalone && !markdown {
            bail!(
                "ambiguous Claude session link in substantive text; refusing to delete other message bytes"
            );
        }
        return Ok(true);
    }
    Ok(false)
}

/// Remove only complete attribution lines and trailing blank lines exposed by removal.
pub(super) fn clean(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut kept = Vec::new();
    let mut removed_footer = false;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if attribution(line)? {
            removed_footer = true;
        } else {
            if !line.trim_ascii().is_empty() {
                removed_footer = false;
            }
            kept.push(line);
        }
    }
    if removed_footer {
        while kept.last().is_some_and(|line| line.trim_ascii().is_empty()) {
            kept.pop();
        }
    }
    Ok(kept.concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_attribution_and_newly_trailing_blanks_change() {
        let before = b"Subject\r\n\r\nBody \xff\r\n\r\nCo-aUtHoReD-bY: CLAUDE <bot@ANTHROPIC.com>\r\n \t\r\n\r\n";
        assert_eq!(clean(before).unwrap(), b"Subject\r\n\r\nBody \xff\r\n");
        for keep in [
            b"A\n\n \t\n".as_slice(),
            b"no final newline",
            b"Co-Authored-By: Alice <a@b>\n",
            b"Co-Authored-By: Claudette <c@d>\n",
            b"Co-Authored-By: Codex <c@d>\n",
            b"Explain Generated with Claude Code in docs\n",
        ] {
            assert_eq!(clean(keep).unwrap(), keep);
        }
        assert_eq!(
            clean(b"Body\n\nCo-Authored-By: Claude\n\nNext\n").unwrap(),
            b"Body\n\n\nNext\n"
        );
        assert_eq!(
            clean(b"Body\nCo-Authored-By: Claude\nNext\n\n \t\n").unwrap(),
            b"Body\nNext\n\n \t\n"
        );
    }

    #[test]
    fn generated_footers_links_and_missing_final_newlines() {
        for line in [
            "🤖 Generated with [Claude Code](https://claude.ai/code)",
            "Generated with Claude Code",
            "generated with CLAUDE CODE",
            "https://claude.ai/code/session_123",
            "[Session](https://claude.ai/code/session_123)",
            "Session: https://claude.ai/code/session_123",
        ] {
            let before = format!("Body\n\n{line}");
            assert_eq!(clean(before.as_bytes()).unwrap(), b"Body\n");
            assert!(clean(line.as_bytes()).unwrap().is_empty());
        }
        assert!(clean(b"Fix real issue; see https://claude.ai/code/session_123").is_err());
    }
}
