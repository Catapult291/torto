//! Lightweight, presentation-independent identifiers for in-book media.
use crate::{Inline, InlineRole, LinkRole, Rgba, TextRun};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{ops::Range, sync::LazyLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaKind {
    Figure,
    Table,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaKey {
    pub kind: MediaKind,
    pub number: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaReference {
    pub range: Range<usize>,
    pub key: MediaKey,
}

const NUMBER: &str = r"(?:[A-Z][.-]?)?[0-9０-９]+(?:[.．\-‐‑–][0-9０-９]+)*(?:[a-zA-Z]|\([a-zA-Z]\))?|[一二三四五六七八九十百零〇]+[A-Za-z]?";

fn number(value: &str) -> String {
    let value: String = value
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap(),
            '．' => '.',
            '‐' | '‑' | '–' => '-',
            _ => c.to_ascii_lowercase(),
        })
        .collect();
    let mut total = 0;
    let mut digit = 0;
    let mut chinese = false;
    let mut suffix = String::new();
    for c in value.chars() {
        match c {
            '零' | '〇' => {
                digit = 0;
                chinese = true;
            }
            c if "一二三四五六七八九".contains(c) => {
                digit = "一二三四五六七八九".chars().position(|x| x == c).unwrap() + 1;
                chinese = true;
            }
            '十' => {
                total += digit.max(1) * 10;
                digit = 0;
                chinese = true;
            }
            '百' => {
                total += digit.max(1) * 100;
                digit = 0;
                chinese = true;
            }
            _ => suffix.push(c),
        }
    }
    if chinese {
        format!("{}{}", total + digit, suffix)
    } else {
        value.replace(['(', ')'], "")
    }
}

/// Byte ranges in the original UTF-8 text. Never rewrites source anchors.
pub fn references(text: &str) -> Vec<MediaReference> {
    static LABEL: LazyLock<Regex> = LazyLock::new(|| {
        // Rust regex intentionally has no lookaround; suffix boundaries are checked below.
        let num = NUMBER;
        Regex::new(&format!(
            r"(?i)(figures?|figs?\.?|tables?|tab\.?|图片|图表|插图|图版|表格|图|表)\s*({num})"
        ))
        .unwrap()
    });
    static NEXT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r"(?i)^\s*(?:、|,|，|and\b|及|和|与)\s*({})",
            NUMBER
        ))
        .unwrap()
    });
    let mut out = Vec::new();
    for capture in LABEL.captures_iter(text) {
        let whole = capture.get(0).unwrap();
        if text[..whole.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphabetic())
        {
            continue;
        }
        let mut end = whole.end();
        if text[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        {
            end -= 1;
        }
        let label = capture.get(1).unwrap().as_str().to_ascii_lowercase();
        let kind = if label.starts_with("tab") || label.starts_with('表') {
            MediaKind::Table
        } else {
            MediaKind::Figure
        };
        let start = capture.get(2).unwrap().start();
        if start >= end {
            continue;
        }
        out.push(MediaReference {
            range: whole.start()..end,
            key: MediaKey {
                kind,
                number: number(&text[start..end]),
            },
        });
        while let Some(next) = NEXT.captures(&text[end..]) {
            let n = next.get(1).unwrap();
            let all = next.get(0).unwrap();
            let at = end + n.start();
            let last = end + n.end();
            out.push(MediaReference {
                range: at..last,
                key: MediaKey {
                    kind,
                    number: number(n.as_str()),
                },
            });
            end += all.end();
        }
    }
    out
}

pub fn caption_label(text: &str) -> Option<MediaReference> {
    references(text)
        .into_iter()
        .next()
        .filter(|r| text[..r.range.start].trim().is_empty())
}

/// Media in an author/year citation belongs to the cited work, not necessarily this book.
pub fn link_references(text: &str) -> Vec<MediaReference> {
    static YEAR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b(?:1[0-9]{3}|20[0-9]{2})\b").unwrap());
    references(text)
        .into_iter()
        .filter(|reference| {
            let before = &text[..reference.range.start];
            let open = before.rfind(['(', '（']);
            let closed = before.rfind([')', '）']);
            let Some(open) = open.filter(|open| closed.is_none_or(|closed| *open > closed)) else {
                return true;
            };
            let Some(close) = text[reference.range.end..].find([')', '）']) else {
                return true;
            };
            let context = &text[open..reference.range.end + close];
            !YEAR.is_match(context) || is_citation(context)
        })
        .collect()
}

/// Only a short reference expression may collapse to [n], not explanatory prose.
pub fn is_citation(text: &str) -> bool {
    if text.len() > 512 {
        return false;
    }
    let refs = references(text);
    if refs.is_empty() {
        return false;
    }
    let mut rest = String::new();
    let mut at = 0;
    for reference in refs {
        if reference.range.start >= at {
            rest.push_str(&text[at..reference.range.start]);
            at = reference.range.end;
        }
    }
    rest.push_str(&text[at..]);
    static CUE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)如|参见|见|所示|以及|及|和|与|see|also|cf\.|and").unwrap()
    });
    CUE.replace_all(&rest, "")
        .chars()
        .all(|c| c.is_whitespace() || "()（）[]［］,，、;；.&".contains(c))
}

/// Link appearance is applied only by unified layout, after caption normalization.
pub fn style_links(content: &mut Vec<Inline>, color: Rgba) {
    let text: String = content
        .iter()
        .map(|i| match i {
            Inline::Text(r) => r.text.clone(),
            _ => "\0".into(),
        })
        .collect();
    let refs = link_references(&text);
    if refs.is_empty() {
        return;
    }
    let mut offset = 0;
    let mut result = Vec::with_capacity(content.len());
    for inline in std::mem::take(content) {
        let Inline::Text(run) = inline else {
            offset += 1;
            result.push(inline);
            continue;
        };
        let len = run.text.len();
        if run.style.inline_citation != 0
            || run.style.inline_role != InlineRole::Normal
            || run.style.link_role != LinkRole::Normal
            || run
                .link
                .as_ref()
                .is_some_and(|href| href.website_url().is_some())
        {
            offset += len;
            result.push(Inline::Text(run));
            continue;
        }
        let mut cuts = vec![0, len];
        for r in &refs {
            for end in [r.range.start, r.range.end] {
                if end > offset && end < offset + len {
                    cuts.push(end - offset);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let mut part: TextRun = run.clone();
            part.text = run.text[pair[0]..pair[1]].into();
            if refs
                .iter()
                .any(|r| r.range.start <= offset + pair[0] && r.range.end >= offset + pair[1])
            {
                part.style.color = color;
                part.style.underline = false;
            }
            result.push(Inline::Text(part));
        }
        offset += len;
    }
    *content = result;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multilingual_numbers_and_continuations() {
        let found = references("如图 ４.４，Fig. C.1; Figures 1.2a and 1.2b; 见图五、七E");
        assert_eq!(
            found
                .iter()
                .map(|r| r.key.number.as_str())
                .collect::<Vec<_>>(),
            ["4.4", "c.1", "1.2a", "1.2b", "5", "7e"]
        );
        assert_eq!(references("Figure 1 shows")[0].key.number, "1");
    }
    #[test]
    fn only_reference_parentheses_collapse() {
        for s in [
            "（如图 4.4）",
            "（如图8-28所示）",
            "(see Fig. C.1)",
            "（见图五、七E）",
        ] {
            assert!(is_citation(s), "{s}");
        }
        for s in [
            "(Smith, 2020, Fig. 4.4)",
            "（如图4.4所示，这是长篇解释）",
            "（第4.4节）",
        ] {
            assert!(!is_citation(s), "{s}");
        }
    }
}
