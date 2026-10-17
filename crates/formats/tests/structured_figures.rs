use rebook_publication::{Block, Inline, TextBlockKind};

#[test]
#[ignore = "requires TORTO_BARRIERS_BOOK local EPUB"]
fn local_barriers_text_figure_surface() {
    use anyrender::ImageRenderer;
    use rebook_layout::{LayoutEngine, LayoutViewport, PageItem, ReaderFontBlob, ReaderStyle, ReaderTypesetting, SpreadMode};
    let opened = rebook_formats::open_file(std::env::var("TORTO_BARRIERS_BOOK").unwrap()).unwrap();
    let source = opened.source();
    let index = source.book().sections.iter().position(|s| s.href.to_string().ends_with("chapter-4.xhtml")).unwrap();
    let section = source.parse_section(index).unwrap();
    let figure = section.blocks.iter().find(|block| matches!(block, Block::Figure(f) if f.caption_blocks().any(|t| {
        let text = t.content.iter().flat_map(Inline::text_runs).map(|r| r.text.as_str()).collect::<String>();
        text.starts_with("Figure 4.3")
    }))).unwrap();
    let style = ReaderStyle { typesetting: ReaderTypesetting::unified(), spread: SpreadMode::Single, ..Default::default() };
    let mut engine = LayoutEngine::with_fonts([ReaderFontBlob::new(std::sync::Arc::new(include_bytes!("../../../assets/fonts/Literata-opsz-wght.ttf").as_slice().to_vec()))]);
    let layout = engine.layout_blocks(source.as_ref(), std::slice::from_ref(figure), LayoutViewport::new(1000, 800).unwrap(), &style).unwrap();
    assert_eq!(layout.pages.len(), 1);
    let page = &layout.pages[0];
    let panel = page.items.iter().find_map(|i| match i { PageItem::FigureBody(p) => Some(p), _ => None }).unwrap();
    assert!((panel.x + panel.width * 0.5 - 500.0).abs() < 0.1);
    assert!(panel.width < rebook_layout::reading_content_width(1000.0, &style));
    let display = rebook_renderer::DisplayListCompiler.compile(page);
    for text in page.items.iter().filter_map(|i| match i { PageItem::Text(t) => Some(t), _ => None }) {
        if let Some(range) = &text.source { assert!(display.contains_source_anchor(&range.start)); }
    }
    let mut renderer = anyrender_vello_cpu::VelloCpuImageRenderer::new(1000, 800);
    let mut pixels = Vec::new();
    renderer.render_to_vec(|scene| display.paint(scene), &mut pixels);
    let image = image::RgbaImage::from_raw(1000, 800, pixels).unwrap();
    assert_eq!(image.get_pixel(panel.x as u32 + 1, panel.y as u32 + 1).0, [255,255,255,255]);
    assert_ne!(image.get_pixel(panel.x as u32 - 1, panel.y as u32 + 1).0, [255,255,255,255]);
    if let Ok(path) = std::env::var("TORTO_FIGURE_PAGE_PNG") { image.save(path).unwrap(); }
    println!("body bounds: x={} y={} width={} height={}", panel.x, panel.y, panel.width, panel.height);
}

#[test]
#[ignore = "requires TORTO_BARRIERS_BOOK local EPUB"]
fn local_barriers_unified_heading_spacing_and_hyphenation() {
    use rebook_layout::{LayoutEngine, LayoutViewport, PageItem, ReaderStyle, ReaderTypesetting, SpreadMode};
    let opened = rebook_formats::open_file(std::env::var("TORTO_BARRIERS_BOOK").unwrap()).unwrap();
    let source = opened.source();
    let index = source.book().sections.iter().position(|s| s.href.to_string().ends_with("chapter-4.xhtml")).unwrap();
    let section = source.parse_section(index).unwrap();
    let paragraphs: Vec<_> = section.blocks.iter().filter(|b| matches!(b, Block::Text(t) if t.kind == TextBlockKind::Paragraph)).take(20).cloned().collect();
    let prohibited = paragraphs.iter().flat_map(Block::text_blocks).flat_map(|t| &t.content).flat_map(Inline::text_runs)
        .filter(|r| r.style.hyphenation == rebook_publication::HyphenationMode::None).count();
    assert!(prohibited > 0, "fixture must retain the authored hyphen prohibition");
    let mut engine = LayoutEngine::new();
    let style = ReaderStyle { typesetting: ReaderTypesetting::unified(), spread: SpreadMode::Single, ..Default::default() };
    let mut total = 0;
    for width in [400, 600, 800] {
        let layout = engine.layout_blocks(source.as_ref(), &paragraphs, LayoutViewport::new(width, 1000).unwrap(), &style).unwrap();
        let count = layout.pages.iter().flat_map(|p| &p.items).filter(|i| matches!(i, PageItem::Text(t) if t.text.as_ref() == "\u{2010}")).count();
        println!("chapter4 width={width}: selected hyphens={count}");
        total += count;
    }
    assert!(total > 0);
    let headings: Vec<_> = section.blocks.iter().filter(|b| matches!(b, Block::Text(t) if t.kind.is_heading())).cloned().collect();
    let layout = engine.layout_blocks(source.as_ref(), &headings, LayoutViewport::new(1200, 1000).unwrap(), &style).unwrap();
    let title = layout.pages.iter().flat_map(|p| &p.items).find_map(|i| match i {
        PageItem::Text(t) if t.text.contains("Writing and editing") => Some(t.text.as_ref()), _ => None,
    }).unwrap();
    println!("chapter4 heading={title:?}; authored no-hyphen runs={prohibited}");
    assert_eq!(title.replace('\u{2060}', ""), "Chapter 4 Writing and editing");
}

#[test]
#[ignore = "requires TORTO_BARRIERS_BOOK local EPUB"]
fn local_barriers_structured_figures_sources_and_numbering() {
    let opened = rebook_formats::open_file(std::env::var("TORTO_BARRIERS_BOOK").unwrap()).unwrap();
    let source = opened.source();
    let mut labels = Vec::new();
    let mut sources = 0;
    let mut six = false;
    let mut alpha = false;
    for index in 0..source.book().sections.len() {
        let section = source.parse_section(index).unwrap();
        for block in &section.blocks {
            if let Block::Figure(figure) = block {
                sources += figure
                    .text_blocks()
                    .filter(|t| t.kind == TextBlockKind::QuoteAttribution)
                    .count();
                let caption = figure
                    .caption_blocks()
                    .map(|t| {
                        t.content
                            .iter()
                            .flat_map(Inline::text_runs)
                            .map(|r| r.text.as_str())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                if !figure.body.is_empty() {
                    println!("{}: {}", section.href, caption);
                    labels.push(caption.clone());
                    let number = figure.caption_blocks().find_map(|t| {
                        let text: String = t
                            .content
                            .iter()
                            .flat_map(Inline::text_runs)
                            .map(|r| r.text.as_str())
                            .collect();
                        rebook_publication::media_references::caption_label(&text)
                            .map(|label| label.key.number)
                    });
                    let body = figure
                        .text_blocks()
                        .filter(|t| {
                            t.kind != TextBlockKind::Caption
                                && t.kind != TextBlockKind::QuoteAttribution
                        })
                        .flat_map(|t| &t.content)
                        .flat_map(Inline::text_runs)
                        .map(|r| r.text.as_str())
                        .collect::<String>();
                    if number.as_deref() == Some("4.1") {
                        assert!(body.contains("stone fruit"));
                        assert!(!body.contains("One-half"));
                    }
                    if number.as_deref() == Some("4.3") {
                        assert!(body.contains("What are clouds?"));
                        assert!(!body.contains("One-half"));
                    }
                    if number.as_deref() == Some("4.2") {
                        assert!(body.contains("One-half"));
                        assert!(!body.contains("stone fruit"));
                        six = figure
                            .text_blocks()
                            .any(|t| matches!(t.kind, TextBlockKind::ListItem { ordinal: 6, .. }));
                        alpha = figure.text_blocks().any(|t| {
                            t.style.list_marker_format
                                == Some(rebook_publication::ListMarkerFormat::LowerAlpha)
                        });
                    }
                }
            }
        }
    }
    for label in ["4.1", "4.2", "4.3", "5.1", "5.2", "5.3", "11.14"] {
        assert!(
            labels.iter().any(|s| s.contains(label)),
            "missing figure {label}"
        );
    }
    assert!(six && alpha, "Figure 4.2 must keep 6, a, b");
    assert!(sources >= 40, "only {sources} sources retained");
    println!("structured figures={}, sources={sources}", labels.len());
}
