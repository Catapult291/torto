//! Source container boundaries shared by focus, AI batching and note numbering.
use rebook_publication::{Block, TextBlockKind};
use std::ops::Range;

pub fn semantic_paragraph_groups(blocks: &[&Block]) -> Vec<Range<usize>> {
    if !blocks.iter().any(|b| matches!(b, Block::Text(t) if t.style.paragraph_group.is_some())) {
        return Vec::new();
    }
    let lists = super::semantic_list_groups(blocks);
    let mut claimed = vec![false; blocks.len()];
    for list in lists {
        claimed[list].fill(true);
    }
    let paragraph = |i: usize| match blocks[i] {
        Block::Text(t)
            if t.kind == TextBlockKind::Paragraph
                && !claimed[i]
                && !super::is_display_formula(t) =>
        {
            t.style.paragraph_group.map(|id| (id, t))
        }
        _ => None,
    };
    let mut groups = Vec::new();
    let mut start = 0;
    while start < blocks.len() {
        let Some((id, first)) = paragraph(start) else {
            start += 1;
            continue;
        };
        let spine = first.source.as_ref().map(|s| &s.start.spine);
        let mut count = usize::from(first.source.is_some());
        let mut end = start + 1;
        while end < blocks.len() {
            let Some((next, text)) = paragraph(end) else {
                break;
            };
            if next != id
                || text
                    .source
                    .as_ref()
                    .is_some_and(|s| Some(&s.start.spine) != spine)
            {
                break;
            }
            count += usize::from(text.source.is_some());
            end += 1;
        }
        if count >= 2 {
            groups.push(start..end);
        }
        start = end;
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use rebook_publication::{SourceAnchor, SourceRange, SpineItemId, TextBlock};
    fn text(node: &str, group: Option<u64>) -> Block {
        let anchor = SourceAnchor {
            spine: SpineItemId::new("chapter").unwrap(),
            node: node.into(),
            text_offset: 0,
        };
        Block::Text(TextBlock {
            kind: TextBlockKind::Paragraph,
            content: vec![],
            source: Some(SourceRange {
                start: anchor.clone(),
                end: anchor,
            }),
            style: rebook_publication::BlockStyle {
                paragraph_group: group,
                ..Default::default()
            },
        })
    }
    #[test]
    fn companions_do_not_split_groups_and_existing_lists_keep_their_introduction() {
        let mut companion = text("translated", Some(1));
        let Block::Text(t) = &mut companion else {
            unreachable!()
        };
        t.source = None;
        let mut item = text("item", None);
        let Block::Text(t) = &mut item else {
            unreachable!()
        };
        t.kind = TextBlockKind::ListItem {
            ordered: false,
            ordinal: 1,
            depth: 0,
            marker_visible: true,
        };
        let blocks = [
            text("a", Some(1)),
            companion,
            text("b", Some(1)),
            text("intro", Some(2)),
            item,
        ];
        let refs = blocks.iter().collect::<Vec<_>>();
        assert_eq!(semantic_paragraph_groups(&refs), vec![0..3]);
    }
}
