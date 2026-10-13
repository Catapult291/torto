use super::*;
use rebook_publication::{SpineItemId, TextRun, TextStyle};

#[test]
fn popup_removes_outer_brackets_from_media_and_author_citations() {
    let anchor = SourceAnchor { spine: SpineItemId::new("test").unwrap(), node: "p".into(), text_offset: 0 };
    let source = SourceRange { start: anchor.clone(), end: anchor };
    for (original, expected) in [
        ("（见图 14.5）", "见图 14.5"),
        ("(see Figure 4.4)", "see Figure 4.4"),
        ("［如表 3.1］", "如表 3.1"),
        ("[Table 3.1]", "Table 3.1"),
        (" (Smith, 2020) ", "Smith, 2020"),
        ("（见图 4.4（a））", "见图 4.4（a）"),
        ("（见图 4.4)", "（见图 4.4)"),
    ] {
        let mut note = FocusFootnote { stable_id: None, owner: None, number: 1, citation: Some((source.clone(), 1)), text: original.into() };
        assert_eq!(note.popup_text(), expected);
        assert_eq!(note.text, original, "source text remains intact");
        note.citation = None;
        assert_eq!(note.popup_text(), original, "ordinary footnotes retain punctuation");
    }
}

#[test]
fn popup_preserves_appearance_order_and_merges_styled_citation_parts() {
    let anchor = SourceAnchor {
        spine: SpineItemId::new("test").unwrap(),
        node: "p".into(),
        text_offset: 0,
    };
    let range = SourceRange {
        start: anchor.clone(),
        end: SourceAnchor {
            text_offset: 100,
            ..anchor
        },
    };
    let run = |text: &str, number, role| {
        Inline::Text(TextRun {
            text: text.into(),
            style: TextStyle {
                inline_citation: number,
                inline_role: role,
                ..Default::default()
            },
            link: None,
        })
    };
    let block = TextBlock {
        kind: TextBlockKind::Paragraph,
        style: Default::default(),
        source: Some(range.clone()),
        content: vec![
            run("Body ", 0, InlineRole::Normal),
            run("(Smith, ", 1, InlineRole::Normal),
            run("2020)", 1, InlineRole::Normal),
            run("Note content", 0, InlineRole::Footnote),
            run(" more ", 0, InlineRole::Normal),
            run("(Jones, 1990)", 2, InlineRole::Normal),
        ],
    };
    let items = text_block_focus_footnotes(&block);
    assert_eq!(items.len(), 3);
    assert!(
        matches!(&items[0],FocusFootnoteSource::Citation{text,source,number:1} if text=="(Smith, 2020)" && source==&range)
    );
    assert!(matches!(&items[1],FocusFootnoteSource::Inline(t, 1) if t=="Note content"));
    assert!(matches!(
        &items[2],
        FocusFootnoteSource::Citation { number: 2, .. }
    ));
}
