//! Authored local paragraph containers. Styles are deliberately not evidence.
use super::*;

fn wrapper(node: Node<'_, '_>) -> bool {
    node.is_element() && matches!(node.tag_name().name(), "div" | "section" | "article")
}

fn meaningful(node: Node<'_, '_>) -> bool {
    if node.is_text() {
        return node.text().is_some_and(|text| !text.trim().is_empty());
    }
    node.is_element()
        && !matches!(node.tag_name().name(), "script" | "style")
        && (node.tag_name().name() != "a" || node_has_visible_text(node))
}

pub(super) fn candidate(node: Node<'_, '_>) -> bool {
    if !wrapper(node) {
        return false;
    }
    let mut children = node.children().filter(|child| meaningful(*child));
    let mut count = 0;
    for child in &mut children {
        if !child.is_element() || !(child.tag_name().name() == "p" || wrapper(child)) {
            return false;
        }
        count += 1;
    }
    if count < 2 {
        return false;
    }
    // A sole-wrapper chain from body is the document's packaging, not a local group.
    let mut current = node;
    while let Some(parent) = current.parent_element() {
        if parent
            .children()
            .filter(|child| meaningful(*child))
            .take(2)
            .count()
            > 1
        {
            return true;
        }
        if !wrapper(parent) {
            return false;
        }
        current = parent;
    }
    false
}

pub(super) fn mark(blocks: &mut [Block], id: u64) {
    if blocks.len() < 2
        || !blocks.iter().all(|block| {
            matches!(block,
        Block::Text(t) if t.kind == TextBlockKind::Paragraph
            && t.source.is_some())
        })
    {
        return;
    }
    // A qualifying outer container owns its complete paragraph sequence,
    // including paragraphs already grouped by nested containers. Protected
    // semantic blocks above still prevent an outer group from absorbing them.
    for block in blocks {
        if let Block::Text(text) = block {
            text.style.paragraph_group = Some(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(body: &str) -> Section {
        let descriptor = SpineItem {
            id: SpineItemId::new("chapter").unwrap(),
            href: PublicationUrl::parse("chapter.xhtml").unwrap(),
            media_type: "application/xhtml+xml".into(),
            linear: true,
            properties: Vec::new(),
        };
        parse_section(
            &format!("<html><body>{body}</body></html>"),
            &descriptor,
            |_| None,
        )
        .unwrap()
    }

    #[test]
    fn stylesheet_important_alignment_reaches_every_container_member() {
        let descriptor = SpineItem {
            id: SpineItemId::new("chapter").unwrap(),
            href: PublicationUrl::parse("chapter.xhtml").unwrap(),
            media_type: "application/xhtml+xml".into(),
            linear: true,
            properties: vec![],
        };
        let s=parse_section("<html><head><style>.center_text {text-align:center !important}.Chart_Head,.ChartText {text-align:left}</style></head><body><p>Before.</p><div><p class='Chart_Head center_text'>End-of-life</p><p class='ChartText center_text'>Natural ingredients are not necessarily safer for wildlife.</p></div><p>After.</p></body></html>",&descriptor,|_|None).unwrap();
        let texts: Vec<_> = s
            .blocks
            .iter()
            .filter_map(|b| {
                if let Block::Text(t) = b {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        for t in &texts[1..3] {
            assert!(t.style.paragraph_group.is_some());
            assert_eq!(t.style.align, TextAlignment::Center);
            assert_eq!(t.style.authored_alignment, Some(TextAlignment::Center));
        }
    }

    #[test]
    fn unstyled_local_group_retains_individual_alignment_and_source_nodes() {
        let s = parse(
            "<p>Before.</p><div><p>A</p><p style='text-align:center'>B</p><p style='text-align:right'>C</p></div><p>After.</p>",
        );
        let texts: Vec<_> = s
            .blocks
            .iter()
            .filter_map(|b| {
                if let Block::Text(t) = b {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        let id = texts[1].style.paragraph_group.unwrap();
        assert!(
            texts[1..4]
                .iter()
                .all(|t| t.style.paragraph_group == Some(id))
        );
        assert!(
            texts[0].style.paragraph_group.is_none() && texts[4].style.paragraph_group.is_none()
        );
        assert_eq!(
            texts[2].style.authored_alignment,
            Some(TextAlignment::Center)
        );
        assert_eq!(texts[3].style.authored_alignment, Some(TextAlignment::End));
        assert!(
            texts[1..4]
                .windows(2)
                .all(|p| p[0].source.as_ref().unwrap().start.node
                    != p[1].source.as_ref().unwrap().start.node)
        );
    }

    #[test]
    fn chapter_wrappers_and_existing_semantics_are_not_new_groups() {
        for body in [
            "<div><p>A</p><p>B</p></div>",
            "<p>Before.</p><div><p>Intro.</p><ul><li>A</li><li>B</li></ul></div>",
            "<p>Before.</p><div><blockquote><p>A</p><p>B</p></blockquote><p>C</p></div>",
            "<p>Before.</p><div><h2>Heading</h2><p>A</p><p>B</p></div>",
        ] {
            let s = parse(body);
            assert!(
                !s.blocks
                    .iter()
                    .any(|b| matches!(b,Block::Text(t) if t.style.paragraph_group.is_some())),
                "{body}"
            );
        }
    }

    #[test]
    fn nested_paragraph_containers_share_the_outer_group_and_ignore_empty_anchors() {
        let s = parse(
            "<p>Before.</p><div><div><a id='anchor'/><p>A</p><p>B</p></div><div><p>C</p><p>D</p></div></div><p>After.</p>",
        );
        let ids: Vec<_> = s
            .blocks
            .iter()
            .filter_map(|b| {
                if let Block::Text(t) = b {
                    t.style.paragraph_group
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(ids.len(), 4);
        assert_eq!(ids[0], ids[1]);
        assert_eq!(ids[2], ids[3]);
        assert_eq!(ids[0], ids[2]);
    }

    #[test]
    fn outer_group_includes_direct_paragraphs_and_nested_groups_with_original_alignment() {
        let s = parse(
            "<p>Before.</p><div><p>Main question.</p><p>Main answer.</p><p/><div><p style='text-align:center'>Nested question.</p><p style='text-align:right'>Nested answer.</p></div></div><p>After.</p>",
        );
        let texts: Vec<_> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 6);
        let group = texts[1].style.paragraph_group.unwrap();
        assert!(
            texts[1..5]
                .iter()
                .all(|t| t.style.paragraph_group == Some(group))
        );
        assert!(texts[0].style.paragraph_group.is_none());
        assert!(texts[5].style.paragraph_group.is_none());
        assert_eq!(
            texts[3].style.authored_alignment,
            Some(TextAlignment::Center)
        );
        assert_eq!(texts[4].style.authored_alignment, Some(TextAlignment::End));
    }

    #[test]
    fn protected_blocks_keep_nested_paragraph_groups_separate() {
        let s = parse(
            "<p>Before.</p><div><div><p>A.</p><p>B.</p></div><figure><img src='image.png'/><figcaption>Figure 1.</figcaption></figure><div><p>C.</p><p>D.</p></div></div><p>After.</p>",
        );
        let ids: Vec<_> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => t.style.paragraph_group,
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 4);
        assert_eq!(ids[0], ids[1]);
        assert_eq!(ids[2], ids[3]);
        assert_ne!(ids[0], ids[2]);
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Figure(_))));
    }
}
