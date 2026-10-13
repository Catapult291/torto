use crate::reader::*;

#[test]
#[ignore = "requires TORTO_PERF_BOOKS and TORTO_PERF_PROBE_ROOT; local saved-position audit"]
fn local_current_unit_layout_probe() {
    use rebook_layout::{LayoutEngine, LineBreakStrategy, timing::{TimingScope, TimingStage}};
    let books: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var("TORTO_PERF_BOOKS").unwrap()).unwrap(),
    ).unwrap();
    let root = PathBuf::from(std::env::var("TORTO_PERF_PROBE_ROOT").unwrap());
    let library = PathBuf::from("C:/Users/Chris/AppData/Local/Rebook/Rebook/data/library");
    let fonts = crate::fonts::embedded_reader_fonts();
    let viewport = LayoutViewport::new(1200, 800).unwrap();
    let mut output = Vec::new();
    for book in books.as_array().unwrap() {
        let id = book["id"].as_str().unwrap();
        let database = root.join(format!("{}-layout.sqlite3", &id[..8]));
        std::fs::copy(PathBuf::from("F:/projects/ebook/tmp/book-open-perf").join(format!("{}-warm-5.sqlite3", &id[..8])), &database).unwrap();
        let store = SyncStore::open_at(database, "anomaly-layout-probe").unwrap();
        let shelf = BookDisplayMetadata {
            id: id.into(), title: book["title"].as_str().unwrap().into(),
            authors: serde_json::from_value(book["authors"].clone()).unwrap(),
        };
        let cover = std::fs::read(library.join("covers").join(book["cover_name"].as_str().unwrap())).ok();
        let mut reader = open_reader_in_viewport(&library.join("books").join(book["storage_name"].as_str().unwrap()), fonts.clone(), Some(shelf), cover, store, viewport).unwrap();
        reader.progress_store = None;
        let content = reader.reader.current_reading_unit_content().unwrap();
        let paragraphs = content.iter().flat_map(|(_, section)| &section.blocks).filter_map(|block| {
            match block { Block::Text(text) => Some(text), _ => None }
        }).collect::<Vec<_>>();
        let characters = paragraphs.iter().map(|text| crate::plugins::text_block_text(text).chars().count()).sum::<usize>();
        let mut samples = Vec::new();
        for sample in 0..5 {
            for optimized in if sample % 2 == 0 { [true, false] } else { [false, true] } {
                let mut engine = LayoutEngine::with_fonts(fonts.iter().cloned());
                engine.set_deferred_images(true);
                let mut style = reader.reader.style();
                style.typesetting.line_break_strategy = if optimized { LineBreakStrategy::Optimized } else { LineBreakStrategy::Greedy };
                let scope = TimingScope::start().unwrap();
                let pages = content.iter().map(|(_, section)| engine.layout_section(reader.source.as_ref(), section, viewport, &style).unwrap().pages.len()).sum::<usize>();
                let timings = scope.finish();
                samples.push(serde_json::json!({"optimized":optimized,"pages":pages,
                    "total_ms":timings.total.as_secs_f64()*1000.0,
                    "glyph_ms":timings.duration(TimingStage::GlyphShape).as_secs_f64()*1000.0,
                    "glyph_calls":timings.calls(TimingStage::GlyphShape),
                    "hyphen_ms":timings.duration(TimingStage::Hyphenation).as_secs_f64()*1000.0,
                    "hyphen_calls":timings.calls(TimingStage::Hyphenation),
                    "break_ms":timings.duration(TimingStage::LineBreak).as_secs_f64()*1000.0,
                    "display_ms":timings.duration(TimingStage::DisplayList).as_secs_f64()*1000.0}));
            }
        }
        output.push(serde_json::json!({"book":book["title"],"profile":"dev","paragraphs":paragraphs.len(),"text_characters":characters,"samples":samples}));
    }
    std::fs::write(root.join("unit-layout-phases.json"), serde_json::to_vec_pretty(&output).unwrap()).unwrap();
}

/// Same reader worker and retained GPU renderer as the application. Does not
/// run reader UI/plugin task dispatch, save reading progress, or contact models.
#[test]
#[ignore = "requires TORTO_PERF_METADATA, TORTO_PERF_STORE and TORTO_PERF_OUTPUT; native GPU"]
fn local_book_open_breakdown() {
    let metadata: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var("TORTO_PERF_METADATA").unwrap()).unwrap(),
    ).unwrap();
    let root = PathBuf::from("C:/Users/Chris/AppData/Local/Rebook/Rebook/data/library");
    let path = root.join("books").join(metadata["storage_name"].as_str().unwrap());
    let shelf = BookDisplayMetadata {
        id: metadata["id"].as_str().unwrap().into(),
        title: metadata["title"].as_str().unwrap().into(),
        authors: serde_json::from_value(metadata["authors"].clone()).unwrap(),
    };
    let cover = std::fs::read(root.join("covers").join(metadata["cover_name"].as_str().unwrap())).ok();
    let store = SyncStore::open_at(PathBuf::from(std::env::var("TORTO_PERF_STORE").unwrap()), "open-performance").unwrap();
    let fonts = crate::fonts::embedded_reader_fonts();
    let viewport = LayoutViewport::new(1200, 800).unwrap();
    let mut gpu = crate::platform::LayerAudit::new([1200, 800]);
    rebook_layout::clear_raster_cache();
    crate::diagnostics::open_capture::start();
    let started = Instant::now();
    let mut reader = open_reader_in_viewport(&path, fonts, Some(shelf), cover, store, viewport).unwrap();
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;
    reader.progress_store = None;
    let started = Instant::now();
    let layout = reader.current_scroll_layout().unwrap();
    reader.scroll_viewport = Some(ScrollViewportState { size: egui::vec2(1200.0,800.0), offset_y: 0.0 });
    reader.rebuild_focus_units(&layout);
    let padding = reader.scroll_content_padding(800.0);
    let unit = &reader.focus_units[reader.focus_unit_index];
    reader.scroll_viewport.as_mut().unwrap().offset_y = (unit.rect.top() + padding - 120.0).max(0.0);
    let visible = kurbo::Rect::new(0.0, f64::from(reader.scroll_viewport.unwrap().offset_y), 1200.0,
        f64::from(reader.scroll_viewport.unwrap().offset_y) + 800.0);
    let mut requests = Vec::new();
    for (index, entry) in layout.pages.iter().enumerate() {
        let crop = kurbo::Rect::new(0.0, f64::from(layout.page_origins[index]), f64::from(entry.page.width()),
            f64::from(layout.page_origins[index] + layout.page_heights[index]));
        let dy = f64::from(layout.page_tops[index] + padding - layout.page_origins[index]);
        for (request, rect) in entry.page.deferred_rasters() {
            if ((rect.intersect(crop)) + kurbo::Vec2::new(0.0, dy)).intersect(visible).area() > 0.0 {
                requests.push(request.clone());
            }
        }
    }
    let context = egui::Context::default();
    let image_started = Instant::now();
    reader.prepare_visible_images(&context);
    let layers = reader.layered_scene().unwrap();
    let scene_ms = started.elapsed().as_secs_f64() * 1000.0;
    let paper = reader.reader.style().background;
    let plan = ReaderFramePlan {
        rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0,800.0)),
        scene_id: reader.scene_id, scene_revision: reader.scene_revision,
        background: peniko::Color::from_rgb8(paper.red,paper.green,paper.blue),
        content_left: 0.0, preview_left: reader.preview_offset_x(1200.0),
        reflow_pending: false, live_resize_preview: false,
    };
    let mut frames = Vec::new();
    let mut ready = false;
    for _ in 0..32 {
        let (stats, ms) = gpu.layered(&layers, plan, 1.0);
        frames.push(serde_json::json!({"ms":ms,"pending":stats.pending,"hits":stats.hits,"tiles":stats.tiles,"pixels":stats.rasterized_pixels}));
        if !stats.pending { ready = true; break; }
    }
    assert!(ready);
    let first_paint_gpu_ms: f64 = frames.iter().map(|frame| frame["ms"].as_f64().unwrap()).sum();
    let wait_started = Instant::now();
    while requests.iter().any(|r| r.ready().is_none()) {
        assert!(image_started.elapsed() < Duration::from_secs(30), "visible images did not finish");
        std::thread::sleep(Duration::from_millis(1));
        reader.prepare_visible_images(&context);
    }
    reader.prepare_visible_images(&context);
    let additional_image_wait_ms = wait_started.elapsed().as_secs_f64() * 1000.0;
    let images_ready_ms = image_started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let final_layers = reader.layered_scene().unwrap();
    let final_scene_ms = started.elapsed().as_secs_f64() * 1000.0;
    let final_plan = ReaderFramePlan { scene_revision: reader.scene_revision, ..plan };
    let mut final_gpu_ms = 0.0;
    let mut final_ready = false;
    for _ in 0..32 {
        let (stats, ms) = gpu.layered(&final_layers, final_plan, 1.0);
        final_gpu_ms += f64::from(ms);
        if !stats.pending { final_ready = true; break; }
    }
    assert!(final_ready);
    let events = crate::diagnostics::open_capture::finish();
    let (_, settled_gpu_ms) = gpu.layered(&final_layers, final_plan, 1.0);
    let stats = rebook_layout::raster_cache_stats();
    let source_section = reader.source.parse_section(reader.snapshot.location.section_index).unwrap();
    let focus = reader.focus_units[reader.focus_unit_index].text.chars().take(90).collect::<String>();
    let result = serde_json::json!({
        "book":metadata["title"],"version":env!("CARGO_PKG_VERSION"),"release":!cfg!(debug_assertions),
        "open_ms":open_ms,"scene_ms":scene_ms,"gpu_frames":frames,"settled_gpu_ms":settled_gpu_ms,
        "first_paint_gpu_ms":first_paint_gpu_ms,"visible_images":requests.len(),
        "additional_image_wait_ms":additional_image_wait_ms,"images_ready_ms":images_ready_ms,
        "final_scene_ms":final_scene_ms,"final_gpu_ms":final_gpu_ms,
        "events":events,"section":reader.snapshot.location.section_index,"page_count":reader.snapshot.location.page_count,
        "blocks":source_section.blocks.len(),"focus_units":reader.focus_units.len(),"focus":focus,
        "chapter":reader.current_chapter_title(),"file_bytes":std::fs::metadata(path).unwrap().len(),
        "raster_hits":stats.hits,"raster_misses":stats.misses,
    });
    std::fs::write(std::env::var("TORTO_PERF_OUTPUT").unwrap(), serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("BOOK_OPEN_PROFILE {result}");
}
