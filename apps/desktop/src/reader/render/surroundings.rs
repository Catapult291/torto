use kurbo::{Affine, BezPath, Rect, Shape};
use peniko::{Color, Fill};
use vello::Scene;

use crate::preferences::FocusStyle;
use crate::reader::{DesktopReader, ScrollViewportState, focus_unit_contains_source_range};

const INACTIVE_OPACITY: f64 = 0.45;

// Disjoint rectangles describe the complement of the clear regions. Unlike
// even-odd holes, overlapping active/selected blocks never dim their overlap.
fn surrounding_regions(bounds: Rect, clear: impl IntoIterator<Item = Rect>) -> Vec<Rect> {
    let mut regions = vec![bounds];
    for clear in clear {
        let mut next = Vec::new();
        for region in regions {
            let cut = region.intersect(clear);
            if cut.area() <= 0.0 {
                next.push(region);
                continue;
            }
            for remainder in [
                Rect::new(region.x0, region.y0, region.x1, cut.y0),
                Rect::new(region.x0, cut.y1, region.x1, region.y1),
                Rect::new(region.x0, cut.y0, cut.x0, cut.y1),
                Rect::new(cut.x1, cut.y0, region.x1, cut.y1),
            ] {
                if remainder.area() > 0.0 {
                    next.push(remainder);
                }
            }
        }
        regions = next;
    }
    regions
}

impl DesktopReader {
    pub(in crate::reader) fn focus_background_highlight(&self) -> bool {
        self.is_focus_mode() && self.focus_style == FocusStyle::BackgroundHighlight
    }

    pub(super) fn paint_focus_surroundings(
        &self,
        scene: &mut Scene,
        viewport: ScrollViewportState,
        padding: f32,
    ) {
        if !self.is_focus_mode()
            || self.focus_style != FocusStyle::SurroundingDim
            || self.focus_units.get(self.focus_unit_index).is_none()
        {
            return;
        }
        let preview_x = f64::from(self.preview_offset_x(viewport.size.x));
        let bounds = Rect::new(
            -preview_x,
            0.0,
            f64::from(viewport.size.x) - preview_x,
            f64::from(viewport.size.y),
        );
        let translate = |rect: egui::Rect| {
            // Keep antialiased glyph edges and the adjacent reference icons clear.
            let rect = rect.expand2(egui::vec2(8.0, 2.0));
            Rect::new(
                f64::from(rect.left()),
                f64::from(rect.top() + padding - viewport.offset_y),
                f64::from(rect.right()),
                f64::from(rect.bottom() + padding - viewport.offset_y),
            )
        };
        let clear = self
            .focus_action_units()
            .iter()
            .map(|unit| translate(unit.rect));
        let selected = self
            .selection
            .as_ref()
            .filter(|_| self.focus_selection_anchor.is_none())
            .into_iter()
            .flat_map(|selection| {
                self.focus_units.iter().filter_map(move |unit| {
                    let rect = translate(unit.rect);
                    (rect.intersect(bounds).area() > 0.0
                        && selection
                            .ranges
                            .iter()
                            .any(|range| focus_unit_contains_source_range(unit, range)))
                    .then_some(rect)
                })
            });
        let mut path = BezPath::new();
        for region in surrounding_regions(bounds, clear.chain(selected)) {
            path.extend(region.path_elements(0.1));
        }
        let paper = self.reader.style().background;
        // Mixing the already-composited page with its own paper is equivalent
        // to 45% content opacity, including images, formulas and annotations.
        // Keep this single interaction layer independent of retained page tiles.
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgba8(
                paper.red,
                paper.green,
                paper.blue,
                ((1.0 - INACTIVE_OPACITY) * 255.0).round() as u8,
            ),
            None,
            &path,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_active_and_selected_regions_are_never_dimmed_twice() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);
        let regions = surrounding_regions(
            bounds,
            [
                Rect::new(10.0, 10.0, 70.0, 50.0),
                Rect::new(40.0, 30.0, 90.0, 80.0),
            ],
        );
        assert_eq!(regions.iter().map(|r| r.area()).sum::<f64>(), 5700.0);
        for (index, region) in regions.iter().enumerate() {
            assert!(!region.contains(kurbo::Point::new(50.0, 40.0)));
            assert!(
                regions[index + 1..]
                    .iter()
                    .all(|other| region.intersect(*other).area() <= 0.0)
            );
        }
    }

    #[test]
    fn offscreen_blocks_and_tall_blocks_preserve_viewport_clipping() {
        let bounds = Rect::new(-12.0, 0.0, 100.0, 80.0);
        assert_eq!(
            surrounding_regions(bounds, [Rect::new(10.0, 100.0, 90.0, 120.0)]),
            vec![bounds]
        );
        let regions = surrounding_regions(bounds, [Rect::new(10.0, -200.0, 90.0, 300.0)]);
        assert_eq!(
            regions,
            vec![
                Rect::new(-12.0, 0.0, 10.0, 80.0),
                Rect::new(90.0, 0.0, 100.0, 80.0)
            ]
        );
    }

    #[test]
    #[ignore = "requires a native GPU; verifies focus opacity and retained composition"]
    fn surrounding_dim_pixels_match_direct_and_retained_rendering() {
        use crate::reader::ReaderFramePlan;
        let mut audit = crate::platform::LayerAudit::new([800, 600]);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let (mut reader, _, _) = crate::reader::semantic_layout::tests::fixture();
            let mut style = reader.reader.style();
            crate::reader::apply_theme_colors(&mut style, theme);
            reader.reader.set_style(style).unwrap();
            reader.invalidate_page_scenes();
            let layout = reader.current_scroll_layout().unwrap();
            reader.rebuild_focus_units(&layout);
            reader.focus_unit_index = 1;
            let padding = reader.scroll_content_padding(600.0);
            let offset_y = reader.focus_units[1].rect.top() + padding - 180.0;
            reader.scroll_viewport = Some(ScrollViewportState {
                size: egui::vec2(800.0, 600.0),
                offset_y,
            });
            let paper = reader.reader.style().background;
            let mut plan = ReaderFramePlan {
                rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)),
                scene_id: reader.scene_id,
                scene_revision: reader.scene_revision,
                background: Color::from_rgb8(paper.red, paper.green, paper.blue),
                content_left: 0.0,
                preview_left: 0.0,
                reflow_pending: false,
                live_resize_preview: false,
            };
            reader.focus_style = FocusStyle::SurroundingDim;
            // Keep focus-mode padding and layout identical in the neutral frame.
            let units = std::mem::take(&mut reader.focus_units);
            audit.direct(&reader.page_scene(), plan, 1.0);
            let baseline = audit.pixels();
            reader.focus_units = units;
            let direct = reader.page_scene();
            audit.direct(&direct, plan, 1.0);
            let dimmed = audit.pixels();
            let active = reader.focus_units[1]
                .rect
                .expand2(egui::vec2(8.0, 2.0))
                .translate(egui::vec2(0.0, padding - offset_y));
            let mut clear_ink = 0;
            let mut dim_ink = 0;
            for y in 0..600_usize {
                for x in 0..800_usize {
                    let point = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                    // Avoid the antialiased boundary of the clear region.
                    if active.expand(2.0).contains(point) && !active.shrink(2.0).contains(point) {
                        continue;
                    }
                    let before = &baseline[(y * 800 + x) * 4..][..3];
                    let after = &dimmed[(y * 800 + x) * 4..][..3];
                    let ink = before
                        .iter()
                        .zip([paper.red, paper.green, paper.blue])
                        .any(|(v, paper)| v.abs_diff(paper) > 40);
                    if active.contains(point) {
                        assert!(
                            before.iter().zip(after).all(|(a, b)| a.abs_diff(*b) <= 2),
                            "active content changed at {x},{y}"
                        );
                        clear_ink += usize::from(ink);
                    } else {
                        for ((before, after), paper) in
                            before
                                .iter()
                                .zip(after)
                                .zip([paper.red, paper.green, paper.blue])
                        {
                            let expected = (f64::from(*before) * (115.0 / 255.0)
                                + f64::from(paper) * (140.0 / 255.0))
                                .round() as u8;
                            assert!(
                                after.abs_diff(expected) <= 3,
                                "surrounding opacity mismatch at {x},{y}: {after}, expected {expected}, theme={theme:?}"
                            );
                        }
                        dim_ink += usize::from(ink);
                    }
                }
            }
            assert!(
                clear_ink > 100 && dim_ink > 100,
                "must compare real ink in both regions"
            );
            let layers = reader.layered_scene().unwrap();
            let identities: Vec<_> = layers.layers.iter().filter_map(|l| l.identity).collect();
            let mut ready = false;
            for _ in 0..64 {
                if !audit.layered(&layers, plan, 1.0).0.pending {
                    ready = true;
                    break;
                }
            }
            assert!(ready);
            audit.parity(&dimmed, "surrounding-dim");
            reader.select_focus_unit(0);
            plan.scene_revision = reader.scene_revision;
            let moved = reader.layered_scene().unwrap();
            assert_eq!(
                identities,
                moved
                    .layers
                    .iter()
                    .filter_map(|l| l.identity)
                    .collect::<Vec<_>>()
            );
            audit.direct(&reader.page_scene(), plan, 1.0);
            let expected = audit.pixels();
            audit.layered(&moved, plan, 1.0);
            audit.parity(&expected, "surrounding-dim-next-block");
            println!(
                "SURROUNDING_DIM theme={theme:?} active_ink={clear_ink} inactive_ink={dim_ink} retained_layers={}",
                identities.len()
            );
        }
    }
}
