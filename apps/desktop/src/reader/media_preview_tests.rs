use super::*;
use rebook_publication::{
    Book, Metadata, PublicationError, PublicationId, Resource, SpineItem, SpineItemId,
};
use std::sync::atomic::AtomicUsize;

#[test]
fn media_modal_rejects_stale_pixels_and_restores_parent_state_on_escape() {
    let (mut reader, _, _) = crate::reader::semantic_layout::tests::fixture();
    reader.ui.focus_footnotes_visible = true;
    let scroll_key = egui::Id::new("saved-footnote-scroll");
    reader
        .ui
        .focus_footnote_scroll_positions
        .insert(scroll_key, 123.0);
    let origin = rebook_publication::SourceRange {
        start: rebook_publication::SourceAnchor {
            spine: rebook_publication::SpineItemId::new("s").unwrap(),
            node: "p".into(),
            text_offset: 0,
        },
        end: rebook_publication::SourceAnchor {
            spine: rebook_publication::SpineItemId::new("s").unwrap(),
            node: "p".into(),
            text_offset: 10,
        },
    };
    let key = media_references::references("Figure 4.4")[0].key.clone();
    reader.media_preview.active = Some(Preview {
        origin,
        key,
        texture: None,
        captions: vec![],
        caption_textures: vec![],
        size: egui::Vec2::ZERO,
        zoom: 1.0,
        pan: egui::Vec2::ZERO,
        previous: None,
        next: None,
        error: None,
        loading: true,
    });
    reader.media_preview.restore_parent = true;
    let (tx, _jobs) = mpsc::sync_channel(1);
    let result = Arc::new(arc_swap::ArcSwapOption::empty());
    reader.media_preview.worker = Some(Worker {
        tx,
        pending: Arc::new(arc_swap::ArcSwapOption::empty()),
        result: result.clone(),
        generation: Arc::new(AtomicU64::new(7)),
    });
    let completed = |generation| Completion {
        generation,
        revision: reader.source.content_revision(),
        result: Ok(Ready {
            image: egui::ColorImage::filled([320, 200], egui::Color32::WHITE),
            size: [320, 200],
            captions: vec!["Figure 4.4 Caption".into()],
            caption_images: vec![egui::ColorImage::filled([280, 100], egui::Color32::WHITE)],
            previous: None,
            next: None,
            position: "1 / 1".into(),
        }),
    };
    result.store(Some(Arc::new(completed(6))));
    let current = completed(7);
    let ctx = egui::Context::default();
    let mut fonts = egui::FontDefinitions::default();
    fonts.families.insert(
        egui::FontFamily::Name(egui_commonmark_backend::STRONG_FONT_FAMILY.into()),
        fonts.families[&egui::FontFamily::Proportional].clone(),
    );
    ctx.set_fonts(fonts);
    let viewport = std::cell::Cell::new(egui::vec2(900.0, 700.0));
    let frame = |reader: &mut super::super::DesktopReader, events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    viewport.get(),
                )),
                events,
                ..Default::default()
            },
            |_ui| {
                reader.media_preview_overlay(&ctx);
            },
        );
        output.textures_delta.clear();
        output.shapes
    };
    let _ = frame(&mut reader, vec![]);
    assert!(
        reader
            .media_preview
            .active
            .as_ref()
            .unwrap()
            .texture
            .is_none()
    );
    result.store(Some(Arc::new(current)));
    let _ = frame(&mut reader, vec![]);
    assert!(
        reader
            .media_preview
            .active
            .as_ref()
            .unwrap()
            .texture
            .is_some()
    );
    assert_eq!(
        reader.media_preview.active.as_ref().unwrap().captions,
        ["Figure 4.4 Caption"]
    );
    let active = reader.media_preview.active.as_ref().unwrap();
    let image_id = active.texture.as_ref().unwrap().id();
    let caption_id = active.caption_textures[0].id();
    let geometry = |shapes: Vec<egui::epaint::ClippedShape>| {
        fn collect(shape: &egui::Shape, meshes: &mut Vec<(egui::TextureId, egui::Rect)>) {
            match shape {
                egui::Shape::Mesh(mesh) => meshes.push((mesh.texture_id, mesh.calc_bounds())),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, meshes);
                    }
                }
                egui::Shape::Text(_) => panic!("single-part previews must have no toolbar labels"),
                _ => {}
            }
        }
        let mut meshes = Vec::new();
        for clipped in shapes {
            collect(&clipped.shape, &mut meshes);
        }
        let image = meshes.iter().find(|(id, _)| *id == image_id).unwrap().1;
        let caption = meshes.iter().find(|(id, _)| *id == caption_id).unwrap().1;
        assert!((image.bottom() - caption.top()).abs() < 0.01, "caption must touch the image's paper");
        assert!((image.center().x - caption.center().x).abs() < 0.01);
        (image, caption)
    };
    let normal = geometry(frame(&mut reader, vec![]));
    let fitted = normal.0.union(normal.1).size();
    let available = crate::ui::overlay_rect(&ctx).shrink(32.0).size();
    assert!(normal.0.width() > 320.0, "small media should expand to the window");
    assert!(fitted.x <= available.x + 0.01 && fitted.y <= available.y + 0.01);
    assert!((fitted.x - available.x).abs() < 0.01 || (fitted.y - available.y).abs() < 0.01);
    reader.media_preview.active.as_mut().unwrap().zoom = 4.0;
    let zoomed = geometry(frame(&mut reader, vec![]));
    assert!((zoomed.0.width() / normal.0.width() - 4.0).abs() < 0.01);
    assert!((zoomed.1.width() / normal.1.width() - 4.0).abs() < 0.01);
    reader.media_preview.active.as_mut().unwrap().pan = egui::vec2(60.0, 40.0);
    let panned = geometry(frame(&mut reader, vec![]));
    assert!((panned.0.min - zoomed.0.min - egui::vec2(60.0, 40.0)).length() < 0.01);
    assert!((panned.1.min - zoomed.1.min - egui::vec2(60.0, 40.0)).length() < 0.01);
    let active = reader.media_preview.active.as_mut().unwrap();
    active.zoom = 1.0;
    active.pan = egui::Vec2::ZERO;
    viewport.set(egui::vec2(1200.0, 1000.0));
    let resized = geometry(frame(&mut reader, vec![]));
    let fitted = resized.0.union(resized.1).size();
    let available = crate::ui::overlay_rect(&ctx).shrink(32.0).size();
    assert!(resized.0.width() > normal.0.width());
    assert!(fitted.x <= available.x + 0.01 && fitted.y <= available.y + 0.01);
    assert!((fitted.x - available.x).abs() < 0.01 || (fitted.y - available.y).abs() < 0.01);
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |_ui| {
            assert!(reader.media_preview_keyboard(&ctx));
        },
    );
    output.textures_delta.clear();
    assert!(reader.media_preview.active.is_none());
    assert!(reader.ui.focus_footnotes_visible);
    assert_eq!(
        reader.ui.focus_footnote_scroll_positions[&scroll_key],
        123.0
    );
    assert!(reader.preserve_media_parent_popup());
    assert!(result.load_full().is_none());
    reader.ui.focus_footnotes_visible = false;
    assert!(!reader.preserve_media_parent_popup());
    reader.ui.focus_footnotes_visible = true;
    assert!(!reader.preserve_media_parent_popup(), "a later hover popup must regain its ordinary dismissal behavior");
}

struct Source {
    book: Book,
    sections: Vec<Section>,
    resources: AtomicUsize,
    parses: AtomicUsize,
}
impl BookSource for Source {
    fn book(&self) -> &Book {
        &self.book
    }
    fn parse_section(&self, index: usize) -> Result<Section, PublicationError> {
        self.parses.fetch_add(1, Ordering::Relaxed);
        Ok(self.sections[index].clone())
    }
    fn resource(&self, _: &PublicationUrl) -> Result<Resource, PublicationError> {
        self.resources.fetch_add(1, Ordering::Relaxed);
        Err(PublicationError::InvalidPublication(
            "No image should be loaded by the index".into(),
        ))
    }
}
fn source(html: &[&str]) -> Arc<Source> {
    let descriptors: Vec<_> = html
        .iter()
        .enumerate()
        .map(|(i, _)| SpineItem {
            id: SpineItemId::new(format!("s{i}")).unwrap(),
            href: PublicationUrl::parse(&format!("s{i}.xhtml")).unwrap(),
            media_type: "application/xhtml+xml".into(),
            linear: true,
            properties: vec![],
        })
        .collect();
    let sections = html
        .iter()
        .zip(&descriptors)
        .map(|(html, descriptor)| rebook_html::parse_section(html, descriptor, |_| None).unwrap())
        .collect();
    Arc::new(Source {
        book: Book {
            id: PublicationId::new("media-tests").unwrap(),
            metadata: Metadata::default(),
            cover: None,
            sections: descriptors,
            table_of_contents: vec![],
        },
        sections,
        resources: AtomicUsize::new(0),
        parses: AtomicUsize::new(0),
    })
}
fn job(source: Arc<dyn BookSource>, at: usize, key: &str) -> Job {
    let section = source.parse_section(at).unwrap();
    let origin = section
        .blocks
        .iter()
        .flat_map(text_blocks)
        .find_map(|t| t.source.clone())
        .unwrap();
    Job {
        generation: 1,
        source,
        origin,
        key: media_references::references(key)[0].key.clone(),
        cursor: Cursor::default(),
        style: ReaderStyle::default(),
        ctx: egui::Context::default(),
    }
}
#[test]
fn lookup_uses_links_and_local_scope_without_loading_images() {
    let source = source(&[
        "<html><body><p>See <a href='s1.xhtml#target'>Figure 1</a>.</p><figure><img src='a.png'/><figcaption>Figure 1 Local</figcaption></figure></body></html>",
        "<html><body><figure id='target'><img src='b.png'/><img src='c.png'/><figcaption>Figure 1 Linked complete group</figcaption></figure></body></html>",
    ]);
    let target = Index::default()
        .resolve(&job(source.clone(), 0, "Figure 1"), &|| true)
        .unwrap();
    assert_eq!(
        target.section, 1,
        "the authored link wins over the local duplicate"
    );
    assert_eq!(source.resources.load(Ordering::Relaxed), 0);
    let local = Index::default()
        .resolve(&job(source.clone(), 1, "Figure 1"), &|| true)
        .unwrap();
    assert_eq!(local.section, 1);
}
#[test]
fn ambiguous_global_numbers_and_list_of_figures_never_pick_a_target() {
    let source = source(&[
        "<html><body><p>See Figure 1.</p><p>Figure 1 list entry</p></body></html>",
        "<html><body><figure><img src='a.png'/><figcaption>Figure 1 First</figcaption></figure></body></html>",
        "<html><body><figure><img src='b.png'/><figcaption>Figure 1 Second</figcaption></figure></body></html>",
    ]);
    assert!(targets(&source.sections[0], 0).is_empty());
    assert!(
        Index::default()
            .resolve(&job(source.clone(), 0, "Figure 1"), &|| true)
            .unwrap_err()
            .contains("several")
    );
    assert_eq!(source.resources.load(Ordering::Relaxed), 0);
}
#[test]
fn long_table_chunks_keep_spanned_rows_together_and_render_all_segments() {
    let rows = (0..80)
        .map(|i| {
            format!(
                "<tr><td{}>Row {i}</td><td>value</td></tr>",
                if i == 23 { " rowspan='4'" } else { "" }
            )
        })
        .collect::<String>();
    let html = format!(
        "<html><body><p>Table 3.1 Values</p><table>{rows}</table><p class='table-note'>Note: Complete trailing note.</p></body></html>"
    );
    let source = source(&[&html]);
    let target = targets(&source.sections[0], 0).pop().unwrap();
    let Block::Table(table) = &source.sections[0].blocks[target.block] else {
        panic!("table")
    };
    let chunks = table_chunks(table).unwrap();
    assert_eq!(chunks[0], 0..27);
    assert_eq!(chunks.last().unwrap().end, 80);
    let mut job = job(source, 0, "Table 3.1");
    let mut seen = 0;
    loop {
        let ready = render(&job, target.clone(), &|| true).unwrap();
        assert!(ready.image.pixels.len() * 4 <= PIXEL_BUDGET);
        assert!(ready.captions.join(" ").contains("Complete trailing note"));
        seen += 1;
        if let Some(next) = ready.next {
            job.cursor = next;
        } else {
            break;
        }
        assert!(seen < 12);
    }
    assert!(seen >= chunks.len());
}
#[test]
fn wrapped_reference_geometry_keeps_source_and_follows_vertical_corrections() {
    let source =
        source(&["<html><body><p>See Figure 12.34 and Table 3.1 for details.</p></body></html>"]);
    let mut style = ReaderStyle::default();
    style.typesetting.mode = rebook_layout::TypesettingMode::Unified;
    style.horizontal_margin = 10.0;
    style.minimum_horizontal_margin = 0.0;
    style.minimum_content_width = 0.0;
    style.spread = SpreadMode::Single;
    let layout = LayoutEngine::with_fonts(crate::fonts::embedded_reader_fonts().iter().cloned())
        .layout_section(
            source.as_ref(),
            &source.sections[0],
            LayoutViewport::new(240, 400).unwrap(),
            &style,
        )
        .unwrap();
    let display = rebook_renderer::DisplayListCompiler.compile(&layout.pages[0]);
    let mut count = 0;
    for item in &layout.pages[0].items {
        if let rebook_layout::PageItem::Text(text) = item {
            for line in text.layout.lines() {
                for item in line.items() {
                    if let rebook_layout::text_layout::PositionedLayoutItem::GlyphRun(run) = item {
                        let brush = run.style().brush;
                        if brush.color.blue > brush.color.red {
                            assert!(!brush.underline, "clickable media links need no underline");
                        }
                    }
                }
            }
            for (rect, key) in rebook_renderer::text_media_reference_rects(text) {
                let at = rect.center();
                let hit = display
                    .media_reference_at(at.x as f32, at.y as f32)
                    .unwrap();
                assert_eq!(hit.key, key);
                assert_eq!(
                    hit.source.start.node,
                    text.source.as_ref().unwrap().start.node
                );
                let shifted = display.translate_source_text(&hit.source, 37.0);
                assert_eq!(
                    shifted
                        .media_reference_at(at.x as f32, at.y as f32 + 37.0)
                        .unwrap()
                        .source,
                    hit.source
                );
                count += 1;
            }
        }
    }
    assert!(count >= 2);
}
#[test]
fn cancelling_a_lookup_does_not_scan_or_decode_the_book() {
    let source = source(&[
        "<html><body><p>See Figure 9.</p></body></html>",
        "<html><body><p>Other chapter.</p></body></html>",
    ]);
    let request = job(source.clone(), 0, "Figure 9");
    let before = source.parses.load(Ordering::Relaxed);
    assert!(Index::default().resolve(&request, &|| false).is_err());
    assert_eq!(source.parses.load(Ordering::Relaxed) - before, 1);
    assert_eq!(source.resources.load(Ordering::Relaxed), 0);
}
#[test]
#[ignore = "requires TORTO_MEDIA_BOOK, optionally TORTO_MEDIA_KEY and TORTO_MEDIA_PNG"]
fn local_book_media_preview() {
    let publication =
        rebook_formats::open_file(std::env::var("TORTO_MEDIA_BOOK").unwrap()).unwrap();
    let source = publication.source();
    let wanted = media_references::references(
        &std::env::var("TORTO_MEDIA_KEY").unwrap_or("Figure 4.5".into()),
    )[0]
    .key
    .clone();
    let mut all = Vec::new();
    for at in 0..source.book().sections.len() {
        all.extend(targets(&source.parse_section(at).unwrap(), at));
    }
    println!("{} media targets", all.len());
    let selected = all
        .iter()
        .find(|t| t.key == wanted)
        .expect("requested media target")
        .clone();
    let section = source.parse_section(selected.section).unwrap();
    println!(
        "target: {selected:?}; block: {:?}",
        std::mem::discriminant(&section.blocks[selected.block])
    );
    let origin = section
        .blocks
        .iter()
        .flat_map(text_blocks)
        .find_map(|t| t.source.clone())
        .unwrap();
    let mut request = Job {
        generation: 1,
        source,
        origin,
        key: wanted,
        cursor: Cursor::default(),
        style: ReaderStyle::default(),
        ctx: egui::Context::default(),
    };
    let resolved = Index::default().resolve(&request, &|| true).unwrap();
    assert_eq!(resolved.block, selected.block);
    let mut count = 0;
    loop {
        let ready = render(&request, selected.clone(), &|| true).unwrap();
        println!(
            "page {}: {}x{} bytes={} captions={:?}",
            ready.position,
            ready.size[0],
            ready.size[1],
            ready.image.pixels.len() * 4,
            ready.captions
        );
        assert!(!ready.captions.is_empty());
        assert!(ready.image.pixels.len() * 4 <= PIXEL_BUDGET);
        if count == 0
            && let Ok(path) = std::env::var("TORTO_MEDIA_PNG")
        {
            image::RgbaImage::from_raw(
                ready.size[0] as u32,
                ready.size[1] as u32,
                ready
                    .image
                    .pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_array())
                    .collect(),
            )
            .unwrap()
            .save(&path)
            .unwrap();
            if let Some(caption) = ready.caption_images.first() {
                image::RgbaImage::from_raw(
                    caption.size[0] as u32,
                    caption.size[1] as u32,
                    caption
                        .pixels
                        .iter()
                        .flat_map(|pixel| pixel.to_array())
                        .collect(),
                )
                .unwrap()
                .save(format!("{path}.caption.png"))
                .unwrap();
            }
        }
        count += 1;
        if let Some(next) = ready.next {
            request.cursor = next;
        } else {
            break;
        }
        assert!(count < 100);
    }
    if let Block::Figure(figure) = &section.blocks[selected.block] {
        assert_eq!(
            count,
            figure.images.len(),
            "all authored media images remain accessible; inline caption formulas stay in the caption"
        );
    }
}
