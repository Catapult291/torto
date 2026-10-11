//! Ordered, document-coordinate layers. Scroll offsets never enter the content
//! identity: the GPU can move retained pixels without replaying their glyphs.
use std::sync::Arc;

#[cfg(test)]
use kurbo::Affine;
use kurbo::Rect;
use peniko::ImageData;
use rebook_renderer::PageDisplayList;
use vello::Scene;

use super::super::DesktopReader;
use super::vello::VelloScene;

pub(crate) enum LayerContent {
    Scene(Arc<Scene>),
    Images {
        page: Arc<PageDisplayList>,
        offset_x: f32,
    },
}

pub(crate) struct ReaderLayer {
    /// None denotes interaction geometry; retained layers use an allocation ID,
    /// never a pointer address which could be reused after an eviction.
    pub(crate) identity: Option<(u64, u8)>,
    pub(crate) content: LayerContent,
    pub(crate) clip: Rect,
    pub(crate) offset: (f64, f64),
}

pub(crate) struct LayeredReaderScene {
    pub(crate) layers: Vec<ReaderLayer>,
}

impl ReaderLayer {
    pub(crate) fn paint(&self, clip: Rect) -> (Arc<Scene>, Vec<ImageData>) {
        match &self.content {
            LayerContent::Scene(scene) => (Arc::clone(scene), Vec::new()),
            LayerContent::Images { page, offset_x } => {
                let mut scene = Scene::new();
                let images = page.paint_image_layer_in(
                    &mut VelloScene::new(&mut scene),
                    *offset_x,
                    Some(clip),
                );
                (Arc::new(scene), images)
            }
        }
    }
}

impl LayeredReaderScene {
    fn dynamic(&mut self, scene: Scene, clip: Rect, offset: (f64, f64)) {
        if !scene.encoding().draw_tags.is_empty() {
            self.layers.push(ReaderLayer {
                identity: None,
                content: LayerContent::Scene(Arc::new(scene)),
                clip,
                offset,
            });
        }
    }

    fn retained(
        &mut self,
        identity: (u64, u8),
        content: LayerContent,
        clip: Rect,
        offset: (f64, f64),
    ) {
        self.layers.push(ReaderLayer {
            identity: Some(identity),
            content,
            clip,
            offset,
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::preferences::ReadingMode;
    use crate::reader::{ReaderFramePlan, ScrollViewportState};
    use rebook_layout::LayoutViewport;

    #[test]
    #[ignore = "requires TORTO_HIGHLIGHT_BOOK local EPUB and native GPU; checks cross-page highlight pixels"]
    fn local_cross_page_highlight_has_no_seam() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let store = crate::sync::SyncStore::open_at(
            std::env::temp_dir().join(format!("highlight-audit-{}.sqlite3", uuid::Uuid::new_v4())),
            "highlight-audit",
        )
        .unwrap();
        let mut reader = crate::reader::open_reader_in_viewport(
            &std::path::PathBuf::from(std::env::var_os("TORTO_HIGHLIGHT_BOOK").unwrap()),
            crate::fonts::embedded_reader_fonts(),
            None,
            None,
            store,
            LayoutViewport::new(900, 240).unwrap(),
        )
        .unwrap();
        reader.progress_store = None;
        let mut style = reader.reader.style().clone();
        style.spread = rebook_layout::SpreadMode::Scroll;
        style.typesetting = rebook_layout::ReaderTypesetting::unified();
        reader.reader.set_style(style).unwrap();
        let target = reader
            .reader
            .toc_items()
            .iter()
            .find(|item| item.label.contains("2.3 N-GRAM"))
            .unwrap()
            .target
            .clone()
            .unwrap();
        reader.reader.go_to_href(&target).unwrap();
        reader.reading_mode = ReadingMode::Focus;
        let layout = reader.current_scroll_layout().unwrap();
        reader.rebuild_focus_units(&layout);
        reader.focus_unit_index = reader
            .focus_units
            .iter()
            .position(|unit| unit.text.contains("The results are shown"))
            .unwrap();
        let ranges = reader.focus_units[reader.focus_unit_index]
            .paint_ranges
            .clone();
        let mut rects = Vec::new();
        let mut pages = Vec::new();
        for (index, entry) in layout.pages.iter().enumerate() {
            let local = entry.page.source_rects(&ranges);
            if local.is_empty() {
                continue;
            }
            pages.push(index);
            rects.extend(local.into_iter().map(|rect| {
                rect + kurbo::Vec2::new(
                    0.,
                    f64::from(layout.page_tops[index] - layout.page_origins[index]),
                )
            }));
        }
        assert!(
            pages.len() > 1,
            "fixture must split the active paragraph across logical pages"
        );
        let edge = layout.page_tops[pages[1]];
        println!("HIGHLIGHT_PAGE_EDGE {edge}, pages={pages:?}");
        let padding = reader.scroll_content_padding(240.);
        reader.scroll_viewport = Some(ScrollViewportState {
            offset_y: edge + padding - 100.325,
            size: egui::vec2(900., 240.),
        });
        let mut layers = reader.layered_scene().unwrap();
        layers.layers.retain(|layer| layer.identity.is_none());
        let plan = ReaderFramePlan {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900., 240.)),
            scene_id: 1,
            scene_revision: 1,
            background: peniko::Color::from_rgb8(212, 210, 206),
        };
        let mut audit = crate::platform::LayerAudit::new([1400, 400]);
        // Keep the former page-mask composition as a diagnostic reference.
        let mut page_masked = vello::Scene::new();
        let viewport = reader.scroll_viewport.unwrap();
        for index in &pages {
            let page = &layout.pages[*index].page;
            let origin = layout.page_origins[*index];
            let clip = kurbo::Rect::new(
                0.,
                f64::from(origin),
                f64::from(page.width()),
                f64::from(origin + layout.page_heights[*index]),
            );
            let mut fragment = vello::Scene::new();
            fragment.push_clip_layer(peniko::Fill::NonZero, kurbo::Affine::IDENTITY, &clip);
            reader.paint_page_overlays(page, &mut super::VelloScene::new(&mut fragment), 0.);
            fragment.pop_layer();
            page_masked.append(
                &fragment,
                Some(kurbo::Affine::translate((
                    0.,
                    f64::from(layout.page_tops[*index] + padding - viewport.offset_y - origin),
                ))),
            );
        }
        for scale in [0.925, 1., 1.25, 1.5] {
            let scene = crate::reader::ReaderScene {
                scene: std::sync::Arc::new(layers.flatten()),
                images: std::sync::Arc::from([]),
                refresh_image_atlas: false,
            };
            audit.direct(&scene, plan, scale);
            let pixels = audit.pixels();
            let pair = rects
                .windows(2)
                .find(|pair| {
                    (pair[0].y1 - f64::from(edge)).abs() < 1.
                        && (pair[1].y0 - f64::from(edge)).abs() < 1.
                })
                .unwrap();
            let x = ((pair[0].x0.max(pair[1].x0) + pair[0].x1.min(pair[1].x1))
                * 0.5
                * f64::from(scale)) as usize;
            let sample = |y: usize| &pixels[(y * 1400 + x) * 4..(y * 1400 + x) * 4 + 4];
            let boundary = (100.325 * f64::from(scale)) as usize;
            let expected = sample(boundary - 6);
            let legacy = crate::reader::ReaderScene {
                scene: std::sync::Arc::new(page_masked.clone()),
                images: std::sync::Arc::from([]),
                refresh_image_atlas: false,
            };
            audit.direct(&legacy, plan, scale);
            let legacy_pixels = audit.pixels();
            let legacy_sample =
                &legacy_pixels[(boundary * 1400 + x) * 4..(boundary * 1400 + x) * 4 + 4];
            println!(
                "HIGHLIGHT_PIXELS scale={scale} page_masked={legacy_sample:?} combined={:?} interior={expected:?}",
                sample(boundary)
            );
            for y in boundary - 1..=boundary + 1 {
                assert!(
                    sample(y)
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| a.abs_diff(*b) <= 1),
                    "scale={scale} boundary={boundary} y={y}: {:?} != {expected:?}",
                    sample(y)
                );
            }
            for _ in 0..64 {
                if !audit.layered(&layers, plan, scale).0.pending {
                    break;
                }
            }
            audit.parity(&pixels, &format!("highlight-seam-{scale}"));
        }
    }

    #[test]
    #[ignore = "requires a local publication and native GPU; verifies real-book pixels and scrolling"]
    fn local_book_layers_match_direct_rendering_and_reuse_scrolling_tiles() {
        let path = std::path::PathBuf::from(std::env::var_os("TORTO_LAYER_BOOK").unwrap());
        let store = crate::sync::SyncStore::open_at(
            std::env::temp_dir().join(format!("layer-audit-{}.sqlite3", uuid::Uuid::new_v4())),
            "layer-audit",
        )
        .unwrap();
        let mut reader = crate::reader::open_reader_in_viewport(
            &path,
            crate::fonts::embedded_reader_fonts(),
            None,
            None,
            store,
            LayoutViewport::new(1216, 789).unwrap(),
        )
        .unwrap();
        reader.progress_store = None;
        if reader.reader.style().spread != rebook_layout::SpreadMode::Scroll {
            let mut style = reader.reader.style().clone();
            style.spread = rebook_layout::SpreadMode::Scroll;
            reader.reader.set_style(style).unwrap();
            reader.invalidate_page_scenes();
        }
        if let Ok(label) = std::env::var("TORTO_LAYER_TOC") {
            let target = reader
                .reader
                .toc_items()
                .iter()
                .find(|item| item.label.contains(&label))
                .unwrap()
                .target
                .clone()
                .unwrap();
            reader.reader.go_to_href(&target).unwrap();
        }
        reader.reading_mode = ReadingMode::Focus;
        let layout = reader.current_scroll_layout().unwrap();
        reader.rebuild_focus_units(&layout);
        for entry in &layout.pages {
            for (request, _) in entry.page.deferred_rasters() {
                request.load(reader.source.as_ref()).unwrap();
            }
        }
        let size = egui::vec2(1216.0, 789.0);
        let plan = ReaderFramePlan {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, size),
            scene_id: reader.scene_id,
            scene_revision: reader.scene_revision,
            background: peniko::Color::from_rgb8(248, 247, 243),
        };
        let mut audit = crate::platform::LayerAudit::new([1216, 789]);
        let mut direct_times = Vec::new();
        let mut retained_times = Vec::new();
        let mut offsets = vec![0.0, 64.0, 128.0, 192.0, 128.0, 64.0, 0.0];
        // Also exercise actual pictures/formulas, rather than only the prose at
        // the beginning of a chapter. Decode is outside the measured paint step.
        for (index, entry) in layout.pages.iter().enumerate() {
            for (_, bounds) in entry.page.deferred_rasters() {
                let offset =
                    layout.page_tops[index] - layout.page_origins[index] + bounds.center().y as f32;
                offsets.push(offset.max(0.0));
                if offsets.len() >= 9 {
                    break;
                }
            }
            if offsets.len() >= 9 {
                break;
            }
        }
        for (index, offset) in offsets.into_iter().enumerate() {
            reader.scroll_viewport = Some(ScrollViewportState {
                offset_y: offset,
                size,
            });
            let direct = reader.page_scene();
            let elapsed = audit.direct(&direct, plan, 1.0);
            let baseline = audit.pixels();
            let layers = reader.layered_scene().unwrap();
            let mut stabilized = false;
            for frame in 0..64 {
                let (stats, _) = audit.layered(&layers, plan, 1.0);
                if frame == 0 {
                    audit.parity(&baseline, &format!("book-{index}-first"));
                }
                if !stats.pending {
                    stabilized = true;
                    break;
                }
            }
            assert!(stabilized, "cache warming must converge");
            audit.parity(&baseline, &format!("book-{index}"));
            let (stats, cached) = audit.layered(&layers, plan, 1.0);
            assert_eq!(stats.tiles, 0);
            assert!(!stats.pending);
            assert_eq!(
                stats.image_bytes, 0,
                "retained pictures must not be reuploaded"
            );
            direct_times.push(elapsed);
            retained_times.push(cached);
            println!(
                "LAYER_SCROLL index={index} offset={offset} direct_complete_ms={elapsed:.3} retained_complete_ms={cached:.3} cache_hits={} rasterized_pixels={} uploaded_image_bytes={}",
                stats.hits, stats.rasterized_pixels, stats.image_bytes
            );
        }
        direct_times.sort_by(f32::total_cmp);
        retained_times.sort_by(f32::total_cmp);
        println!(
            "LAYER_BENCH direct_complete_median_ms={:.3} retained_complete_median_ms={:.3}",
            direct_times[direct_times.len() / 2],
            retained_times[retained_times.len() / 2]
        );
        // Paginated modes share this path, including native PDF page images.
        reader.reading_mode = ReadingMode::Classic;
        let mut style = reader.reader.style().clone();
        style.spread = rebook_layout::SpreadMode::Single;
        reader.reader.set_style(style).unwrap();
        reader.invalidate_page_scenes();
        let direct = reader.page_scene();
        audit.direct(&direct, plan, 1.0);
        let baseline = audit.pixels();
        let layers = reader.layered_scene().unwrap();
        for _ in 0..64 {
            if !audit.layered(&layers, plan, 1.0).0.pending {
                break;
            }
        }
        audit.parity(&baseline, "paginated");
    }
}

impl DesktopReader {
    pub(crate) fn layered_scene(&mut self) -> Option<LayeredReaderScene> {
        if self.completion.is_some() {
            return None;
        }
        let mut result = LayeredReaderScene { layers: Vec::new() };
        if self.is_scroll_mode() {
            let viewport = self.scroll_viewport?;
            let layout = self.current_scroll_layout().ok()?;
            let padding = self.scroll_content_padding(viewport.size.y);
            let window = Rect::new(
                0.0,
                0.0,
                f64::from(viewport.size.x),
                f64::from(viewport.size.y),
            );
            let mut backdrop = Scene::new();
            let bounds = self.paint_scroll_backdrop(&mut backdrop, viewport, padding);
            result.dynamic(backdrop, measured_clip(window, Some(bounds)), (0.0, 0.0));
            let mut text_layers = LayeredReaderScene { layers: Vec::new() };
            let mut foreground_layers = LayeredReaderScene { layers: Vec::new() };
            for (index, entry) in layout.pages.iter().enumerate() {
                let top = layout.page_tops[index] + padding;
                let bottom = top + layout.page_heights[index];
                if bottom <= viewport.offset_y || top >= viewport.offset_y + viewport.size.y {
                    continue;
                }
                let cached = self.scroll_page_layers(entry);
                let origin = layout.page_origins[index];
                let clip = Rect::new(
                    0.0,
                    f64::from(origin),
                    f64::from(entry.page.width()),
                    f64::from(origin + layout.page_heights[index]),
                );
                let offset = (0.0, f64::from(top - viewport.offset_y - origin));
                result.retained(
                    (cached.generation, 0),
                    LayerContent::Images {
                        page: Arc::clone(&entry.page),
                        offset_x: 0.0,
                    },
                    clip,
                    offset,
                );
                text_layers.retained(
                    (cached.generation, 1),
                    LayerContent::Scene(Arc::clone(&cached.content)),
                    clip,
                    offset,
                );
                let mut foreground = Scene::new();
                let mut bridge = VelloScene::measuring(&mut foreground);
                self.paint_page_foreground(&entry.page, &mut bridge, 0.0);
                let border_clip = Rect::new(clip.x0, clip.y0 - 4.0, clip.x1, clip.y1 + 4.0);
                let foreground_clip = measured_clip(border_clip, bridge.painted_bounds());
                foreground_layers.dynamic(foreground, foreground_clip, offset);
            }
            let mut overlays = Scene::new();
            let bounds = self.paint_scroll_overlays(&mut overlays, &layout, viewport, padding);
            result.dynamic(overlays, measured_clip(window, Some(bounds)), (0.0, 0.0));
            result.layers.extend(text_layers.layers);
            result.layers.extend(foreground_layers.layers);
            let mut bridges = Scene::new();
            let bounds = self.paint_scroll_bridges(&mut bridges, &layout, viewport, padding);
            result.dynamic(bridges, measured_clip(window, Some(bounds)), (0.0, 0.0));
        } else {
            let cached = self.page_scene_layers();
            let spread = self.reader.current_spread().ok()?;
            let width = (spread.primary_offset_x + spread.primary.width() as f32).max(
                spread
                    .secondary
                    .as_ref()
                    .map_or(0.0, |p| spread.secondary_offset_x + p.width() as f32),
            );
            let clip = Rect::new(
                0.0,
                0.0,
                f64::from(width),
                f64::from(spread.primary.height()),
            );
            result.retained(
                (cached.generation, 2),
                LayerContent::Scene(Arc::clone(&cached.underlay)),
                clip,
                (0.0, 0.0),
            );
            for (page, offset_x, kind) in [
                (Some(spread.primary.clone()), spread.primary_offset_x, 0),
                (spread.secondary.clone(), spread.secondary_offset_x, 3),
            ] {
                if let Some(page) = page {
                    result.retained(
                        (cached.generation, kind),
                        LayerContent::Images {
                            page: Arc::clone(&page),
                            offset_x,
                        },
                        clip,
                        (0.0, 0.0),
                    );
                    let mut overlays = Scene::new();
                    let mut bridge = VelloScene::measuring(&mut overlays);
                    self.paint_page_overlays(&page, &mut bridge, offset_x);
                    let overlay_clip = measured_clip(clip, bridge.painted_bounds());
                    result.dynamic(overlays, overlay_clip, (0.0, 0.0));
                }
            }
            result.retained(
                (cached.generation, 1),
                LayerContent::Scene(Arc::clone(&cached.content)),
                clip,
                (0.0, 0.0),
            );
            let mut foreground = Scene::new();
            let mut bridge = VelloScene::measuring(&mut foreground);
            self.paint_page_foreground(&spread.primary, &mut bridge, spread.primary_offset_x);
            if let Some(page) = spread.secondary {
                self.paint_page_foreground(&page, &mut bridge, spread.secondary_offset_x);
            }
            let foreground_clip = measured_clip(clip, bridge.painted_bounds());
            result.dynamic(foreground, foreground_clip, (0.0, 0.0));
        }
        Some(result)
    }
}

#[cfg(test)]
impl LayeredReaderScene {
    /// Same vector ordering as the direct renderer, useful for parity checks.
    pub(crate) fn flatten(&self) -> Scene {
        let mut result = Scene::new();
        for layer in &self.layers {
            let (scene, _) = layer.paint(layer.clip);
            let mut clipped = Scene::new();
            clipped.push_clip_layer(peniko::Fill::NonZero, Affine::IDENTITY, &layer.clip);
            clipped.append(&scene, None);
            clipped.pop_layer();
            result.append(&clipped, Some(Affine::translate(layer.offset)));
        }
        result
    }
}

fn measured_clip(original: Rect, bounds: Option<Rect>) -> Rect {
    bounds.map_or(original, |bounds| {
        original.intersect(bounds.inflate(2.0, 2.0))
    })
}
