//! Release notes are Markdown. Blocks (headings, paragraphs, lists, rules, code) become
//! styled rows; inline markers (`**bold**`, `` `code` ``, `[text](url)`) are reduced to
//! their text, since a text run carries one style.
use ara_kit::look::Look;
use stillkit::el::{Align, El, col, el, row, text};

use super::HubApp;

/// Markdown `source` as blocks.
pub(super) fn render(l: &Look, source: &str) -> El<HubApp> {
    let c = &l.c;
    let paragraph_el = |t: String| text(t).text_size(12.0).wrap(200).color(c.foreground.0);
    let mut out = col().gap(6.0);
    let mut paragraph = String::new();
    let mut code: Option<String> = None;
    let flush = |out: El<HubApp>, paragraph: &mut String| {
        if paragraph.is_empty() {
            return out;
        }
        out.child(paragraph_el(inline(&std::mem::take(paragraph))))
    };
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            match code.take() {
                Some(block) => out = out.child(code_block(l, block.trim_end())),
                None => {
                    out = flush(out, &mut paragraph);
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
            out = flush(out, &mut paragraph);
        } else if let Some((level, title)) = heading(trimmed) {
            out = flush(out, &mut paragraph);
            let size = [16.0, 14.0, 13.0][level.min(3) - 1];
            out = out.child(
                text(inline(title))
                    .text_size(size)
                    .weight(600)
                    .wrap(3)
                    .color(c.foreground.0),
            );
        } else if let Some((marker, item)) = list_item(trimmed) {
            out = flush(out, &mut paragraph);
            let indent = ((line.len() - line.trim_start().len()).min(8) * 4) as f32;
            out = out.child(
                row()
                    .gap(6.0)
                    .align(Align::Start)
                    .child(el().w(indent))
                    .child(text(marker).text_size(12.0).color(c.muted_foreground.0))
                    .child(paragraph_el(inline(item)).w(0.0).grow(1.0)),
            );
        } else if trimmed.len() >= 3 && trimmed.chars().all(|c| matches!(c, '-' | '*' | '_')) {
            out = flush(out, &mut paragraph);
            out = out.child(el().h(1.0).w_full().fill(c.border.0));
        } else {
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(trimmed.trim_start_matches('>').trim_start());
        }
    }
    if let Some(block) = code {
        out = out.child(code_block(l, block.trim_end()));
    }
    flush(out, &mut paragraph)
}

fn code_block(l: &Look, source: &str) -> El<HubApp> {
    col().pad(8.0).radius(l.radius.xs).fill(l.well()).child(
        text(source.to_owned())
            .text_size(11.5)
            .family(ara_kit::ui::mono())
            .wrap(200)
            .color(l.c.foreground.0),
    )
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
