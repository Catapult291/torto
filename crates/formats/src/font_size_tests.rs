//! Font sizes across the actual EPUB parser and unified shaping pipeline.
use super::*;
use rebook_layout::{
    LayoutEngine, LayoutViewport, PageItem, ReaderFontBlob, ReaderStyle, ReaderTypesetting,
    SpreadMode,
};
use rebook_publication::{Block, TextBlock, TextBlockKind};

fn style() -> ReaderStyle {
    let mut style = ReaderStyle {
        typesetting: ReaderTypesetting::unified(),
        spread: SpreadMode::Single,
        ..Default::default()
    };
    style.typography.font_size = 20.0;
    style.typography.minimum_font_size = 8.0;
    style.typesetting.table_font_scale = 0.8;
    style
}

fn engine() -> LayoutEngine {
    LayoutEngine::with_fonts([ReaderFontBlob::new(Arc::new(
        include_bytes!("../../../assets/fonts/Literata-opsz-wght.ttf")
            .as_slice()
            .to_vec(),
    ))])
}

fn sizes(
    source: &dyn BookSource,
    block: &TextBlock,
    kind: TextBlockKind,
    unified: bool,
) -> Vec<f32> {
    let mut block = block.clone();
    block.kind = kind;
    let mut style = style();
    if !unified {
        style.typesetting = ReaderTypesetting::default();
    }
    let result = engine()
        .layout_blocks(
            source,
            &[Block::Text(block)],
            LayoutViewport::new(900, 800).unwrap(),
            &style,
        )
        .unwrap();
    result
        .pages
        .iter()
        .flat_map(|p| &p.items)
        .filter_map(|item| {
            if let PageItem::Text(text) = item {
                Some(text)
            } else {
                None
            }
        })
        .flat_map(|text| {
            text.layout.lines().flat_map(|line| {
                line.runs()
                    .filter(|run| !text.text[run.text_range()].trim().is_empty())
                    .map(|run| run.font_size())
            })
        })
        .collect()
}

fn assert_size(sizes: &[f32], expected: f32) {
    assert!(!sizes.is_empty());
    assert!(
        sizes.iter().all(|size| (size - expected).abs() < 0.01),
        "expected {expected}, got {sizes:?}"
    );
}

#[test]
fn epub_block_keywords_use_semantic_sizes_and_inline_changes_remain_local() {
    let mut entries = epub::tests::minimal_entries();
    entries[5] = ("OPS/Text/chapter.xhtml", br#"<html><head><style>body{font-size:large} p{font-size:small}</style></head><body>
        <p>Ordinary body.</p>
        <p>Before <span style="font-size:smaller">local</span> after.</p>
        <p>Before <sup>raised</sup> after.</p>
        <table><tr><td style="font-size:x-small"><p style="font-size:small">Cell body.</p><p>Cell <span style="font-size:larger">larger</span>.</p></td></tr></table>
    </body></html>"#, zip::CompressionMethod::Deflated);
    let source = epub::EpubPublication::open_bytes(epub::tests::zip_entries(&entries)).unwrap();
    let section = source.parse_section(0).unwrap();
    let body: Vec<_> = section
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
    assert_eq!(body.len(), 3);
    for (kind, expected) in [
        (TextBlockKind::Paragraph, 20.0),
        (TextBlockKind::Heading(2), 28.64),
        (TextBlockKind::Blockquote, 19.0),
        (TextBlockKind::QuoteAttribution, 17.6),
        (TextBlockKind::Caption, 17.6),
        (TextBlockKind::Preformatted, 18.0),
        (TextBlockKind::FootnoteDefinition, 20.0),
        (TextBlockKind::DefinitionTerm { depth: 0 }, 20.0),
        (
            TextBlockKind::ListItem {
                ordered: false,
                ordinal: 1,
                depth: 0,
                marker_visible: false,
            },
            20.0,
        ),
    ] {
        assert_size(&sizes(&source, body[0], kind, true), expected);
    }
    assert_size(
        &sizes(&source, body[0], TextBlockKind::Paragraph, false),
        20.0 * 8.0 / 9.0,
    );
    let local = sizes(&source, body[1], TextBlockKind::Paragraph, true);
    assert!(local.iter().any(|s| (*s - 20.0 / 1.2).abs() < 0.01));
    assert!(local.iter().any(|s| (*s - 20.0).abs() < 0.01));
    let raised = sizes(&source, body[2], TextBlockKind::Paragraph, true);
    assert!(raised.iter().any(|s| (*s - 14.0).abs() < 0.01));
    assert!(raised.iter().any(|s| (*s - 20.0).abs() < 0.01));
    let table = section
        .blocks
        .iter()
        .find(|b| matches!(b, Block::Table(_)))
        .unwrap();
    let layout = engine()
        .layout_blocks(
            &source,
            &[table.clone()],
            LayoutViewport::new(900, 800).unwrap(),
            &style(),
        )
        .unwrap();
    let cell_sizes: Vec<_> = layout
        .pages
        .iter()
        .flat_map(|p| &p.items)
        .filter_map(|item| {
            if let PageItem::Table(table) = item {
                Some(table)
            } else {
                None
            }
        })
        .flat_map(|table| &table.cells)
        .filter_map(|cell| cell.text.as_ref())
        .flat_map(|text| {
            text.layout.lines().flat_map(|line| {
                line.runs()
                    .filter(|run| !text.text[run.text_range()].trim().is_empty())
                    .map(|run| run.font_size())
            })
        })
        .collect();
    assert!(
        cell_sizes
            .iter()
            .all(|s| (*s - 16.0).abs() < 0.01 || (*s - 19.2).abs() < 0.01),
        "{cell_sizes:?}"
    );
    assert!(cell_sizes.iter().any(|s| (*s - 19.2).abs() < 0.01));
}

#[test]
#[ignore = "requires TORTO_FONT_AUDIT_BOOK; reads local EPUB without changing it"]
fn local_handbook_body_uses_configured_unified_size() {
    let path = std::env::var_os("TORTO_FONT_AUDIT_BOOK").expect("local EPUB path");
    let source = epub::EpubPublication::open_bytes(fs::read(path).unwrap()).unwrap();
    let mut checked = 0;
    for (index, descriptor) in source.book().sections.iter().enumerate() {
        if !["c01.xhtml", "c02.xhtml"]
            .iter()
            .any(|suffix| descriptor.href.path().ends_with(suffix))
        {
            continue;
        }
        let section = source.parse_section(index).unwrap();
        let mut chapter_checked = 0;
        for block in &section.blocks {
            let Block::Text(text) = block else { continue };
            if text.kind != TextBlockKind::Paragraph {
                continue;
            }
            let runs: Vec<_> = text
                .content
                .iter()
                .flat_map(|i| i.text_runs())
                .filter(|r| !r.text.trim().is_empty())
                .collect();
            if runs.is_empty()
                || !runs.iter().all(|r| {
                    r.style
                        .keyword_size_scale
                        .is_some_and(|s| (s - 8.0 / 9.0).abs() < 0.0001)
                        && r.style.inline_size_scale.is_none()
                        && r.style.baseline == rebook_publication::TextBaseline::Normal
                        && r.link.is_none()
                        && r.style.inline_citation == 0
                })
            {
                continue;
            }
            assert_size(&sizes(&source, text, TextBlockKind::Paragraph, true), 20.0);
            assert_size(
                &sizes(&source, text, TextBlockKind::Paragraph, false),
                20.0 * 8.0 / 9.0,
            );
            checked += 1;
            chapter_checked += 1;
            if chapter_checked == 4 {
                break;
            }
        }
        assert_eq!(chapter_checked, 4, "{}", descriptor.href.path());
    }
    assert_eq!(checked, 8);
    println!("HANDBOOK_FONT_AUDIT paragraphs={checked} authored=17.7778 unified=20");
}
