//! Lightweight, presentation-independent identifiers for in-book media.
use crate::{Inline, InlineRole, LinkRole, Rgba, TextRun};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{ops::Range, sync::LazyLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaKind {
    Figure,
    Table,
    Equation,
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
        // Classifiers after a Chinese numeral belong to prose, e.g. 代表一个,
        // rather than the label 表一. Keep real labels such as 表一中 and 图五所示.
        if text[start..end]
            .chars()
            .any(|c| "一二三四五六七八九十百零〇".contains(c))
            && text[end..].starts_with(['个', '種', '种', '次', '些', '般', '旦'])
        {
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
    out.extend(equation_references(text));
    out.sort_by_key(|r| r.range.start);
    out
}

fn equation_references(text: &str) -> Vec<MediaReference> {
    static LABEL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r"(?i)(?:\bequations?\s+|\beqs?(?:\.\s*|\s+)|公式\s*|式\s*)[（(]?\s*({NUMBER})\s*[）)]?"
        ))
        .unwrap()
    });
    static NEXT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r"(?i)^\s*(?:、|,|，|and\b|及|和|与)\s*[（(]\s*({NUMBER})\s*[）)]"
        ))
        .unwrap()
    });
    let mut result = Vec::new();
    for capture in LABEL.captures_iter(text) {
        let whole = capture.get(0).unwrap();
        if text[..whole.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || "方式形模格样".contains(c))
        {
            continue;
        }
        let n = capture.get(1).unwrap();
        if text[n.end()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        {
            continue;
        }
        result.push(MediaReference {
            range: whole.range(),
            key: MediaKey {
                kind: MediaKind::Equation,
                number: number(n.as_str()),
            },
        });
        let mut end = whole.end();
        while let Some(next) = NEXT.captures(&text[end..]) {
            let n = next.get(1).unwrap();
            // Include parentheses in equation continuations, as in Eqs. (3.9) and (3.10).
            let all = next.get(0).unwrap();
            let start = text[end..end + n.start()].rfind(['(', '（']).unwrap();
            result.push(MediaReference {
                range: end + start..end + all.end(),
                key: MediaKey {
                    kind: MediaKind::Equation,
                    number: number(n.as_str()),
                },
            });
            end += all.end();
        }
    }
    result
}

/// A bare number is eligible only in a structural equation row or a verified authored link.
pub fn equation_number(text: &str) -> Option<MediaKey> {
    static LABEL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(&format!(r"^\s*[（(]\s*({NUMBER})\s*[）)]\s*$")).unwrap());
    let capture = LABEL.captures(text)?;
    Some(MediaKey {
        kind: MediaKind::Equation,
        number: number(capture.get(1)?.as_str()),
    })
}

/// A standalone mathematical object and its number, without relying on CSS class names.
pub fn equation_target(block: &crate::Block) -> Option<MediaKey> {
    match block {
        crate::Block::Image(image) => {
            image
                .formula
                .as_ref()?
                .equation_number
                .as_ref()
                .and_then(|n| {
                    equation_number(&format!("({})", n.trim_matches(['(', ')', '（', '）'])))
                })
        }
        crate::Block::Text(text) => {
            let mut objects = 0;
            let mut label = String::new();
            let mut embedded = None;
            for inline in &text.content {
                match inline {
                    Inline::Image(image) if image.image.ocr_text.is_none() => {
                        objects += 1;
                        embedded = image
                            .image
                            .formula
                            .as_ref()
                            .and_then(|f| f.equation_number.as_deref());
                    }
                    Inline::Math(_) => objects += 1,
                    Inline::Text(run) => label.push_str(&run.text),
                    Inline::Break => label.push(' '),
                    _ => return None,
                }
            }
            if objects != 1 {
                return None;
            }
            equation_number(&label).or_else(|| {
                label
                    .trim()
                    .is_empty()
                    .then_some(embedded)
                    .flatten()
                    .and_then(|n| {
                        equation_number(&format!("({})", n.trim_matches(['(', ')', '（', '）'])))
                    })
            })
        }
        _ => None,
    }
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

fn reference_text(content: &[Inline]) -> String {
    let mut text = String::new();
    for inline in content {
        match inline {
            Inline::Text(run)
                if !run.style.suppress_media_references
                    && run.style.inline_role == InlineRole::Normal
                    && run.style.link_role == LinkRole::Normal
                    && run.style.baseline == crate::TextBaseline::Normal
                    && !run
                        .link
                        .as_ref()
                        .is_some_and(|href| href.website_url().is_some()) =>
            {
                text.push_str(&run.text)
            }
            // Preserve byte offsets and prevent a note from extending 11.13.
            Inline::Text(run) => text.extend(std::iter::repeat_n('\0', run.text.len())),
            _ => text.push('\0'),
        }
    }
    text
}

/// Source byte ranges with note/image/external-link boundaries respected.
pub fn inline_references(content: &[Inline]) -> Vec<MediaReference> {
    let mut refs = link_references(&reference_text(content));
    let mut offset = 0;
    for inline in content {
        if let Inline::Text(run) = inline {
            if !run.style.suppress_media_references
                && run.style.inline_role == InlineRole::Normal
                && run.style.link_role == LinkRole::Normal
                && run.style.baseline == crate::TextBaseline::Normal
                && run
                    .link
                    .as_ref()
                    .is_some_and(|href| href.website_url().is_none())
                && let Some(key) = equation_number(&run.text)
                && !refs
                    .iter()
                    .any(|r| r.range.start <= offset && r.range.end >= offset + run.text.len())
            {
                refs.push(MediaReference {
                    range: offset..offset + run.text.len(),
                    key,
                });
            }
            offset += run.text.len();
        } else {
            offset += 1;
        }
    }
    refs.sort_by_key(|r| r.range.start);
    refs
}

/// Retain source-backed media references after translation without asking the
/// model for extra markup or scanning any other chapter/resource.
pub fn restrict_translated_links(content: &mut Vec<Inline>, original: &[Inline]) {
    let source_refs = inline_references(original);
    if source_refs.is_empty() {
        for inline in content {
            if let Inline::Text(run) = inline {
                run.style.suppress_media_references = true;
            }
        }
        return;
    }
    let refs: Vec<_> = inline_references(content)
        .into_iter()
        .filter(|r| source_refs.iter().any(|source| source.key == r.key))
        .collect();
    let mut offset = 0;
    let mut result = Vec::with_capacity(content.len());
    for inline in std::mem::take(content) {
        let Inline::Text(run) = inline else {
            offset += 1;
            result.push(inline);
            continue;
        };
        let len = run.text.len();
        let mut cuts = vec![0, len];
        for reference in &refs {
            for end in [reference.range.start, reference.range.end] {
                if end > offset && end < offset + len {
                    cuts.push(end - offset);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let mut part = run.clone();
            part.text = run.text[pair[0]..pair[1]].into();
            part.style.suppress_media_references = !refs
                .iter()
                .any(|r| r.range.start <= offset + pair[0] && r.range.end >= offset + pair[1]);
            result.push(Inline::Text(part));
        }
        offset += len;
    }
    *content = result;
}

/// Link appearance is applied only by unified layout, after caption normalization.
pub fn style_links(content: &mut Vec<Inline>, color: Rgba) {
    let refs = inline_references(content);
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
            || run.style.suppress_media_references
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
            if let Some(reference) = refs
                .iter()
                .find(|r| r.range.start <= offset + pair[0] && r.range.end >= offset + pair[1])
            {
                part.style.color = color;
                part.style.underline = false;
                part.style.media_reference = Some(reference.key.kind);
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

    fn content(text: &str) -> Vec<Inline> {
        vec![Inline::Text(TextRun {
            text: text.into(),
            style: Default::default(),
            link: None,
        })]
    }

    #[test]
    fn equations_need_labels_or_authored_links_and_preserve_parentheses() {
        let refs = references("Eqs. (3.9) and (3.10); Equation 8.1; 公式（4.2）与式（4.3）");
        assert_eq!(
            refs.iter()
                .map(|r| r.key.number.as_str())
                .collect::<Vec<_>>(),
            ["3.9", "3.10", "8.1", "4.2", "4.3"]
        );
        assert!(refs.iter().all(|r| r.key.kind == MediaKind::Equation));
        for text in [
            "EQ3 and EQ6 devices",
            "(2020)",
            "Section 3.1",
            "方式（3.1）",
            "(3.1)",
        ] {
            assert!(references(text).is_empty(), "{text}");
        }
        assert!(is_citation("（见公式（4.2））"));
        let mut linked = content("(3.10)");
        if let Inline::Text(run) = &mut linked[0] {
            run.link = Some(crate::PublicationUrl::parse("chapter.xhtml#formula10").unwrap());
        }
        assert_eq!(inline_references(&linked)[0].key.number, "3.10");
        style_links(&mut linked, Rgba::BLACK);
        assert_eq!(
            linked[0].text_runs()[0].style.media_reference,
            Some(MediaKind::Equation),
            "click semantics do not depend on link color"
        );
    }

    #[test]
    fn chinese_classifiers_do_not_create_table_or_figure_references() {
        for text in ["代表一个声音", "图一般而言", "见表一种结果", "代表一些字符"]
        {
            assert!(references(text).is_empty(), "{text}");
        }
        assert_eq!(references("表一中列出了数据")[0].key.number, "1");
        assert_eq!(references("图五所示")[0].key.number, "5");
    }

    #[test]
    fn adjacent_note_markers_do_not_extend_media_numbers() {
        let mut content = content("See Fig. 11.13");
        content.push(Inline::Text(TextRun {
            text: "2".into(),
            style: crate::TextStyle {
                link_role: LinkRole::FootnoteReference,
                baseline: crate::TextBaseline::Superscript,
                ..Default::default()
            },
            link: Some(crate::PublicationUrl::parse("chapter.xhtml#fn2").unwrap()),
        }));
        let references = inline_references(&content);
        assert_eq!(references[0].key.number, "11.13");
        style_links(
            &mut content,
            Rgba {
                red: 45,
                green: 104,
                blue: 210,
                alpha: 255,
            },
        );
        assert_eq!(inline_references(&content)[0].key.number, "11.13");
        assert_eq!(content.last().unwrap().text_runs()[0].text, "2");
    }

    #[test]
    fn translated_links_require_the_same_source_kind_and_number() {
        let original = content("See Figure 11.1 and Table 1.");
        let mut translated = content("代表一个声音。见图 11.1、表一，以及图 9.9 和表 11.1。");
        restrict_translated_links(&mut translated, &original);
        let found = inline_references(&translated);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].key.number, "11.1");
        assert_eq!(found[1].key.number, "1");
        style_links(
            &mut translated,
            Rgba {
                red: 45,
                green: 104,
                blue: 210,
                alpha: 255,
            },
        );
        let linked: String = translated
            .iter()
            .flat_map(Inline::text_runs)
            .filter(|run| run.style.color.blue > run.style.color.red)
            .map(|run| run.text.as_str())
            .collect();
        assert_eq!(linked, "图 11.1表一");
        assert_eq!(
            translated
                .iter()
                .flat_map(Inline::text_runs)
                .map(|run| run.text.as_str())
                .collect::<String>(),
            "代表一个声音。见图 11.1、表一，以及图 9.9 和表 11.1。"
        );
        let mut invented = content("见图 11.1 与表一。");
        restrict_translated_links(&mut invented, &content("There are two examples."));
        assert!(inline_references(&invented).is_empty());
    }
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
