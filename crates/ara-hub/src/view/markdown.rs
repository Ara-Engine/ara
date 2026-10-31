//! Release notes are Markdown. The native text runs carry one style each, so blocks
//! (headings, paragraphs, lists, code) become styled rows and inline markers
//! (`**bold**`, `` `code` ``, `[text](url)`) are reduced to their text.
use super::Page;

/// Render Markdown `source` as blocks into `p`.
pub(super) fn render(p: &mut Page, source: &str) {
    p.panel("md", |p| {
        let mut paragraph = String::new();
        let mut code: Option<String> = None;
        let flush = |p: &mut Page, paragraph: &mut String| {
            if !paragraph.is_empty() {
                p.text("md-p", &inline(paragraph));
                paragraph.clear();
            }
        };
        for line in source.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                match code.take() {
                    Some(block) => p.text("md-code", block.trim_end()),
                    None => {
                        flush(p, &mut paragraph);
                        code = Some(String::new());
                    }
                }
                continue;
            }
            if let Some(block) = &mut code {
                block.push_str(line);
                block.push('\n');
                continue;
            }
            if trimmed.is_empty() {
                flush(p, &mut paragraph);
            } else if let Some((level, title)) = heading(trimmed) {
                flush(p, &mut paragraph);
                p.text(
                    ["md-h1", "md-h2", "md-h3"][level.min(3) - 1],
                    &inline(title),
                );
            } else if let Some((marker, item)) = list_item(trimmed) {
                flush(p, &mut paragraph);
                let indent = (line.len() - line.trim_start().len()).min(8) * 4;
                p.panel_styled("md-li", &format!("padding-left:{indent}px"), |p| {
                    p.text("md-bullet", &marker);
                    p.text("md-p md-li-text", &inline(item));
                });
            } else if trimmed.chars().all(|c| matches!(c, '-' | '*' | '_')) && trimmed.len() >= 3 {
                flush(p, &mut paragraph);
                p.leaf("md-rule");
            } else {
                if !paragraph.is_empty() {
                    paragraph.push(' ');
                }
                paragraph.push_str(trimmed.trim_start_matches('>').trim_start());
            }
        }
        if let Some(block) = code {
            p.text("md-code", block.trim_end());
        }
        flush(p, &mut paragraph);
    });
}

/// Markdown reduced to plain text, for one-line excerpts.
pub(super) fn plain(source: &str) -> String {
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("```"))
        .map(|line| {
            let line = heading(line).map_or(line, |(_, title)| title);
            inline(list_item(line).map_or(line, |(_, item)| item))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    (1..=6)
        .contains(&level)
        .then(|| line[level..].strip_prefix(' '))
        .flatten()
        .map(|title| (level, title.trim()))
}

fn list_item(line: &str) -> Option<(String, &str)> {
    if let Some(item) = ["- ", "* ", "+ "].iter().find_map(|m| line.strip_prefix(m)) {
        return Some(("•".into(), item));
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    (digits > 0)
        .then(|| line[digits..].strip_prefix(". "))
        .flatten()
        .map(|item| (format!("{}.", &line[..digits]), item))
}

/// Strip inline emphasis and code markers; links keep their text.
fn inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let (before, after) = rest.split_at(start);
        out.push_str(before);
        match after
            .find("](")
            .and_then(|mid| after[mid..].find(')').map(|end| (mid, mid + end)))
        {
            Some((mid, end)) => {
                out.push_str(&after[1..mid]);
                rest = &after[end + 1..];
            }
            None => {
                out.push('[');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "").replace("__", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_markers_reduce_to_text() {
        assert_eq!(
            inline("**New:** see [the docs](https://ara.dev) for `ara init`"),
            "New: see the docs for ara init"
        );
        assert_eq!(heading("## Fixes"), Some((2, "Fixes")));
        assert_eq!(heading("#hashtag"), None);
        assert_eq!(list_item("12. Twelve"), Some(("12.".into(), "Twelve")));
        assert_eq!(
            plain("# Ara 0.1\n- **Fast** builds\n"),
            "Ara 0.1 · Fast builds"
        );
    }
}
