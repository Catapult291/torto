use super::*;
use rebook_publication::SourceAnchor;

pub(super) fn source_spans(block: &TextBlock, spans: &[TextSourceSpan]) -> Vec<TextSourceSpan> {
    let bases = if spans.is_empty() {
        block.source.as_ref().map(|source| vec![TextSourceSpan { range: 0..block.content.len(), source: source.clone() }]).unwrap_or_default()
    } else { spans.to_vec() };
    let units = |range: Range<usize>| block.content[range].iter().map(|i| match i {
        Inline::Text(t) => t.text.chars().count(),
        Inline::Ruby(r) => r.source_char_len(),
        Inline::Math(m) => m.source_char_len(),
        Inline::Break => 1,
        Inline::Image(_) => 0,
    }).sum::<usize>();
    let mut result = Vec::new();
    for base in bases {
        if base.source.start.spine != base.source.end.spine || base.source.start.node != base.source.end.node {
            result.push(base);
            continue;
        }
        let total = units(base.range.clone());
        let original_length = base.source.end.text_offset.saturating_sub(base.source.start.text_offset);
        let offset = |at: usize| {
            let count = units(base.range.start..at);
            base.source.start.text_offset + if total == 0 { 0 } else { (count as u128 * original_length as u128 / total as u128) as u64 }
        };
        let segment = |start: usize, end: usize| TextSourceSpan {
            range: start..end,
            source: SourceRange {
                start: SourceAnchor { text_offset: offset(start), ..base.source.start.clone() },
                end: SourceAnchor { text_offset: offset(end), ..base.source.end.clone() },
            },
        };
        let mut start = base.range.start;
        for index in base.range.clone() {
            if matches!(&block.content[index],Inline::Image(r) if r.image.ocr_text.is_some()) {
                if start < index { result.push(segment(start, index)); }
                result.push(segment(index, index + 1));
                start = index + 1;
            }
        }
        if start < base.range.end { result.push(segment(start, base.range.end)); }
    }
    result
}

pub(super) fn style(block: &TextBlock, index: usize, scale: f32, color: Rgba) -> TextStyle {
    let mut style = block.content[..index].iter().rev().find_map(|i| if let Inline::Text(r) = i { Some(r.style) } else { None }).unwrap_or_default();
    style.color = color;
    style.size_scale = scale;
    style.inline_citation = 0;
    style.inline_role = InlineRole::Normal;
    style.link_role = LinkRole::Normal;
    style.footnote_number = 0;
    style.display_writing_system = Some(WritingSystem::Unknown);
    style.baseline = TextBaseline::Normal;
    style
}
