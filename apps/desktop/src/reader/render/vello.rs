//! Adapter from the renderer-neutral retained display list to Vello.

use std::sync::Arc;

use anyrender::{Filter, NormalizedCoord, Paint, PaintRef, PaintScene, RenderContext};
use kurbo::{Affine, Diagonal2, Rect, Shape, Stroke, Vec2};
use peniko::{BlendMode, Color, Fill, FontData, StyleRef};
use vello::{FontEmbolden, Glyph, Scene};

pub struct VelloScene<'a> {
    scene: &'a mut Scene,
    measuring: bool,
    bounds: Option<Rect>,
    unbounded: bool,
}

impl<'a> VelloScene<'a> {
    pub fn new(scene: &'a mut Scene) -> Self {
        Self {
            scene,
            measuring: false,
            bounds: None,
            unbounded: false,
        }
    }

    pub(super) fn measuring(scene: &'a mut Scene) -> Self {
        Self {
            measuring: true,
            ..Self::new(scene)
        }
    }

    pub(super) fn painted_bounds(&self) -> Option<Rect> {
        (!self.unbounded).then_some(self.bounds).flatten()
    }

    fn include_bounds(&mut self, transform: Affine, shape: &impl Shape, padding: f64) {
        if self.measuring {
            let bounds =
                transform.transform_rect_bbox(shape.bounding_box().inflate(padding, padding));
            self.bounds = Some(
                self.bounds
                    .map_or(bounds, |previous| previous.union(bounds)),
            );
        }
    }
}

impl RenderContext for VelloScene<'_> {}

impl PaintScene for VelloScene<'_> {
    fn reset(&mut self) {
        self.scene.reset();
        self.bounds = None;
        self.unbounded = false;
    }

    fn push_layer(
        &mut self,
        fill: Fill,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl Shape,
        _filter: Option<Arc<Filter>>,
        _backdrop_filter: Option<Arc<Filter>>,
    ) {
        self.scene.push_layer(fill, blend, alpha, transform, clip);
    }

    fn push_clip_layer(&mut self, fill: Fill, transform: Affine, clip: &impl Shape) {
        self.scene.push_clip_layer(fill, transform, clip);
    }

    fn pop_layer(&mut self) {
        self.scene.pop_layer();
    }

    fn stroke<'a>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        paint: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        source_shape: &impl Shape,
    ) {
        self.include_bounds(
            transform,
            source_shape,
            style.width * style.miter_limit.max(1.0),
        );
        self.scene.stroke(
            style,
            transform,
            solid_brush(&paint.into()),
            brush_transform,
            source_shape,
        );
    }

    fn fill<'a>(
        &mut self,
        fill: Fill,
        transform: Affine,
        paint: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        source_shape: &impl Shape,
    ) {
        self.include_bounds(transform, source_shape, 0.0);
        match paint.into() {
            Paint::Solid(value) => {
                self.scene
                    .fill(fill, transform, value, brush_transform, source_shape);
            }
            Paint::Gradient(value) => {
                self.scene
                    .fill(fill, transform, value, brush_transform, source_shape);
            }
            Paint::Image(value) => {
                self.scene
                    .fill(fill, transform, value, brush_transform, source_shape);
            }
            Paint::Resource(_) | Paint::Custom(_) => {
                self.scene.fill(
                    fill,
                    transform,
                    Color::TRANSPARENT,
                    brush_transform,
                    source_shape,
                );
            }
        }
    }

    fn draw_glyphs<'a, 's: 'a>(
        &'s mut self,
        font: &'a FontData,
        font_size: f32,
        hint: bool,
        normalized_coords: &'a [NormalizedCoord],
        embolden: Vec2,
        style: impl Into<StyleRef<'a>>,
        paint: impl Into<PaintRef<'a>>,
        brush_alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = anyrender::Glyph>,
    ) {
        // Keep the safe page clip for commands whose painted bounds cannot be
        // determined cheaply; never guess glyph or filter extents.
        self.unbounded |= self.measuring;
        let paint = paint.into();
        let builder = self
            .scene
            .draw_glyphs(font)
            .font_size(font_size)
            .hint(hint)
            .normalized_coords(normalized_coords)
            .font_embolden(FontEmbolden::new(Diagonal2::new(embolden.x, embolden.y)))
            .brush(solid_brush(&paint))
            .brush_alpha(brush_alpha)
            .transform(transform)
            .glyph_transform(glyph_transform);
        let glyphs = glyphs.map(|glyph| Glyph {
            id: glyph.id,
            x: glyph.x,
            y: glyph.y,
        });
        match style.into() {
            StyleRef::Fill(fill) => builder.draw(fill, glyphs),
            StyleRef::Stroke(stroke) => builder.draw(stroke, glyphs),
        }
    }

    fn draw_box_shadow(
        &mut self,
        transform: Affine,
        rect: Rect,
        brush: Color,
        radius: f64,
        std_dev: f64,
    ) {
        self.unbounded |= self.measuring;
        self.scene
            .draw_blurred_rounded_rect(transform, rect, brush, radius, std_dev);
    }
}

fn solid_brush(paint: &PaintRef<'_>) -> Color {
    match paint {
        Paint::Solid(value) => *value,
        Paint::Gradient(_) | Paint::Image(_) | Paint::Resource(_) | Paint::Custom(_) => {
            Color::TRANSPARENT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interaction_bounds_follow_transforms_and_keep_unknown_extents_safe() {
        let mut scene = Scene::new();
        let mut bridge = VelloScene::measuring(&mut scene);
        bridge.fill(
            Fill::NonZero,
            Affine::translate((10.0, 20.0)),
            Color::BLACK,
            None,
            &Rect::new(1.0, 2.0, 3.0, 4.0),
        );
        assert_eq!(
            bridge.painted_bounds(),
            Some(Rect::new(11.0, 22.0, 13.0, 24.0))
        );
        bridge.draw_box_shadow(
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, 100.0, 100.0),
            Color::BLACK,
            4.0,
            8.0,
        );
        assert_eq!(
            bridge.painted_bounds(),
            None,
            "unknown/filter extents keep the full semantic clip"
        );
    }
}
