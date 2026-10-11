//! Retained raster strips plus a small, ordered interaction atlas. Only the
//! final composition visits the whole viewport on a normal scrolling frame.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use kurbo::{Affine, Rect};
use peniko::{Color, Fill, ImageData};
use vello::{AaConfig, RenderParams, Renderer, Scene};
use wgpu::util::DeviceExt;

use crate::reader::{LayeredReaderScene, ReaderFramePlan};

const STRIP_HEIGHT: i32 = 512;
const GUTTER: i32 = 2;
const CACHE_BUDGET: usize = 64 * 1024 * 1024;
const ATLAS_BUDGET: usize = 48 * 1024 * 1024;
const NEW_TILES_PER_FRAME: usize = 2;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct TileKey {
    identity: (u64, u8),
    scale: u32,
    strip: i32,
    clip: [u64; 4],
    images: Vec<u64>,
    phase: [u16; 2],
}

struct RasterTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    binding: wgpu::BindGroup,
    size: [u32; 2],
}

struct Tile {
    pixels: RasterTexture,
    used: u64,
}

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct Quad {
    rect: [f32; 4],
    uv: [f32; 4],
}

struct Slot {
    scene: Arc<Scene>,
    images: Vec<ImageData>,
    /// In physical document coordinates, including the anti-aliasing gutter.
    bounds: Rect,
    clip: Rect,
    atlas_y: u32,
    key: Option<TileKey>,
}

enum Pixels {
    Tile(TileKey),
    Atlas(usize),
}

struct Piece {
    pixels: Pixels,
    bounds: Rect,
    visible: Rect,
    offset: (f64, f64),
}

#[derive(Default, Debug)]
pub(crate) struct LayerStats {
    pub hits: usize,
    pub tiles: usize,
    pub rasterized_pixels: usize,
    pub image_bytes: usize,
    pub pending: bool,
    pub submit_ms: f32,
    pub images: Vec<ImageData>,
    pub scene_bytes: usize,
}

pub(super) struct LayerRenderer {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    tiles: HashMap<TileKey, Tile>,
    atlas: Option<RasterTexture>,
    epoch: u64,
    scene: Option<(u64, u32)>,
    bytes: usize,
    pending: bool,
}

impl LayerRenderer {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reader-layer-texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("reader-layer-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("reader-layer-compositor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("reader_layers.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("reader-layer-pipeline"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("reader-layer-compositor"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Quad>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
                })],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            layout,
            sampler,
            pipeline,
            tiles: HashMap::new(),
            atlas: None,
            epoch: 0,
            scene: None,
            bytes: 0,
            pending: false,
        }
    }

    pub(super) fn clear(&mut self) {
        self.tiles.clear();
        self.atlas = None;
        self.bytes = 0;
        self.pending = false;
        self.scene = None;
    }

    pub(super) fn pending(&self) -> bool {
        self.pending
    }
    pub(super) fn cache_bytes(&self) -> usize {
        self.bytes
    }
    pub(super) fn atlas_bytes(&self) -> usize {
        self.atlas.as_ref().map_or(0, |a| texture_bytes(a.size))
    }

    fn texture(&self, device: &wgpu::Device, size: [u32; 2]) -> RasterTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reader-retained-raster"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reader-retained-raster"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        RasterTexture {
            texture,
            view,
            binding,
            size,
        }
    }

    /// Returns None for a viewport/device too large for bounded temporary
    /// storage. The caller keeps the direct path as a correctness fallback.
    pub(super) fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut Renderer,
        target: &wgpu::TextureView,
        size: [u32; 2],
        plan: ReaderFramePlan,
        scale: f32,
        layers: &LayeredReaderScene,
    ) -> Result<Option<LayerStats>, String> {
        let started = Instant::now();
        if self.scene != Some((plan.scene_id, scale.to_bits())) {
            self.clear();
            self.scene = Some((plan.scene_id, scale.to_bits()));
        }
        self.epoch += 1;
        let scale64 = f64::from(scale);
        let window = Rect::new(0.0, 0.0, f64::from(size[0]), f64::from(size[1]));
        let mut slots = Vec::new();
        let mut pieces = Vec::new();
        let mut stats = LayerStats::default();
        let mut active = HashSet::new();
        let mut selected = HashSet::new();
        let mut atlas_size = [1, 0];
        let mut visible_cache_bytes = 0;
        for layer in &layers.layers {
            let offset = (layer.offset.0 * scale64, layer.offset.1 * scale64);
            // Rasterize at the actual subpixel phase of this page's placement.
            // Whole-pixel scrolling then reuses sharp glyphs; fractional motion
            // never stretches a tile rasterized at a different glyph phase.
            let phase = [subpixel_phase(offset.0), subpixel_phase(offset.1)];
            let phase_offset =
                kurbo::Vec2::new(f64::from(phase[0]) / 256.0, f64::from(phase[1]) / 256.0);
            let clip = scale_rect(layer.clip, scale64);
            let visible = clip.intersect(window - kurbo::Vec2::new(offset.0, offset.1));
            if visible.is_zero_area() {
                continue;
            }
            let first = (visible.y0 / f64::from(STRIP_HEIGHT)).floor() as i32;
            let last = (visible.y1 / f64::from(STRIP_HEIGHT)).ceil() as i32;
            let strips = if layer.identity.is_some() {
                first..last
            } else {
                0..1
            };
            for strip in strips {
                let region = if layer.identity.is_some() {
                    Rect::new(
                        clip.x0,
                        f64::from(strip * STRIP_HEIGHT).max(clip.y0),
                        clip.x1,
                        f64::from((strip + 1) * STRIP_HEIGHT).min(clip.y1),
                    )
                } else {
                    visible
                };
                let part = region.intersect(visible);
                if part.is_zero_area() {
                    continue;
                }
                let bounds = padded_pixels(region + phase_offset) - phase_offset;
                let logical = scale_rect(bounds, 1.0 / scale64);
                let (scene, images) = layer.paint(logical);
                if scene.encoding().draw_tags.is_empty() {
                    continue;
                }
                let key = layer.identity.map(|identity| TileKey {
                    identity,
                    scale: scale.to_bits(),
                    strip,
                    clip: [
                        clip.x0.to_bits(),
                        clip.y0.to_bits(),
                        clip.x1.to_bits(),
                        clip.y1.to_bits(),
                    ],
                    images: images.iter().map(|i| i.data.id()).collect(),
                    phase,
                });
                if let Some(key) = &key {
                    active.insert(key.clone());
                    visible_cache_bytes += texture_bytes(rect_size(bounds));
                    if let Some(tile) = self.tiles.get_mut(key) {
                        tile.used = self.epoch;
                        stats.hits += 1;
                        pieces.push(Piece {
                            pixels: Pixels::Tile(key.clone()),
                            bounds,
                            visible: part,
                            offset,
                        });
                        continue;
                    }
                    if selected.contains(key) {
                        pieces.push(Piece {
                            pixels: Pixels::Tile(key.clone()),
                            bounds,
                            visible: part,
                            offset,
                        });
                        continue;
                    }
                }
                let cache_this = key.is_some()
                    && selected.len() < NEW_TILES_PER_FRAME
                    && visible_cache_bytes <= CACHE_BUDGET;
                let raster_bounds = if cache_this {
                    bounds
                } else {
                    padded_pixels(part + phase_offset) - phase_offset
                };
                let slot_size = rect_size(raster_bounds);
                let index = slots.len();
                let slot_key = if cache_this {
                    let key = key.unwrap();
                    selected.insert(key.clone());
                    Some(key)
                } else {
                    if key.is_some() && visible_cache_bytes <= CACHE_BUDGET {
                        stats.pending = true;
                    }
                    None
                };
                let pixels = slot_key
                    .as_ref()
                    .map_or(Pixels::Atlas(index), |key| Pixels::Tile(key.clone()));
                pieces.push(Piece {
                    pixels,
                    bounds: raster_bounds,
                    visible: part,
                    offset,
                });
                slots.push(Slot {
                    scene,
                    images,
                    bounds: raster_bounds,
                    clip,
                    atlas_y: atlas_size[1],
                    key: slot_key,
                });
                atlas_size[0] = atlas_size[0].max(slot_size[0]);
                atlas_size[1] += slot_size[1];
            }
        }
        // A visible set larger than the cache cannot converge. Fall back instead
        // of starting an endless cache-warming redraw loop at very high DPI.
        if visible_cache_bytes > CACHE_BUDGET {
            stats.pending = false;
        }
        atlas_size[1] = atlas_size[1].max(1);
        let limit = device.limits().max_texture_dimension_2d;
        if atlas_size.iter().any(|n| *n > limit) || texture_bytes(atlas_size) > ATLAS_BUDGET {
            self.pending = false;
            return Ok(None);
        }
        if !slots.is_empty() {
            // Grow in modest increments; shrinking every animation frame would
            // cause allocation churn. Book/DPI changes release the old atlas.
            let allocation = [
                atlas_size[0].div_ceil(64) * 64,
                atlas_size[1].div_ceil(256) * 256,
            ];
            if self
                .atlas
                .as_ref()
                .is_none_or(|a| a.size[0] < atlas_size[0] || a.size[1] < atlas_size[1])
            {
                self.atlas = Some(self.texture(device, allocation));
            }
            let atlas = self.atlas.as_ref().unwrap();
            let mut scene = Scene::new();
            let mut seen_images = HashSet::new();
            for slot in &slots {
                let slot_rect = Rect::new(
                    0.0,
                    f64::from(slot.atlas_y),
                    slot.bounds.width(),
                    f64::from(slot.atlas_y) + slot.bounds.height(),
                );
                let transform =
                    Affine::translate((-slot.bounds.x0, f64::from(slot.atlas_y) - slot.bounds.y0))
                        * Affine::scale(scale64);
                scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &slot_rect);
                scene.push_clip_layer(
                    Fill::NonZero,
                    transform,
                    &scale_rect(slot.clip, 1.0 / scale64),
                );
                scene.append(&slot.scene, Some(transform));
                scene.pop_layer();
                scene.pop_layer();
                for image in &slot.images {
                    if seen_images.insert(image.data.id()) {
                        renderer.mark_override_image_dirty(image);
                        stats.image_bytes += image.data.len();
                        stats.images.push(image.clone());
                    }
                }
            }
            stats.scene_bytes = crate::reader::render::scene_encoding_bytes(&scene);
            renderer
                .render_to_texture(
                    device,
                    queue,
                    &scene,
                    &atlas.view,
                    &RenderParams {
                        base_color: Color::TRANSPARENT,
                        width: atlas_size[0],
                        height: atlas_size[1],
                        antialiasing_method: AaConfig::Area,
                    },
                )
                .map_err(|e| e.to_string())?;
            stats.rasterized_pixels = atlas_size[0] as usize * atlas_size[1] as usize;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("reader-layer-compose"),
        });
        for slot in slots.iter().filter(|slot| slot.key.is_some()) {
            let key = slot.key.clone().unwrap();
            let pixels = self.texture(device, rect_size(slot.bounds));
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas.as_ref().unwrap().texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: slot.atlas_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                pixels.texture.as_image_copy(),
                wgpu::Extent3d {
                    width: pixels.size[0],
                    height: pixels.size[1],
                    depth_or_array_layers: 1,
                },
            );
            self.bytes += texture_bytes(pixels.size);
            self.tiles.insert(
                key,
                Tile {
                    pixels,
                    used: self.epoch,
                },
            );
            stats.tiles += 1;
        }
        let quads: Vec<_> = pieces
            .iter()
            .map(|piece| {
                let (texture_size, atlas_y) = match &piece.pixels {
                    Pixels::Tile(key) => (self.tiles[key].pixels.size, 0.0),
                    Pixels::Atlas(index) => (
                        self.atlas.as_ref().unwrap().size,
                        f64::from(slots[*index].atlas_y),
                    ),
                };
                quad(piece, size, texture_size, atlas_y)
            })
            .collect();
        let vertices = (!quads.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("reader-layer-quads"),
                contents: bytemuck::cast_slice(&quads),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        {
            let rgba = plan.background.to_rgba8();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("reader-layer-compose"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(rgba.r) / 255.0,
                            g: f64::from(rgba.g) / 255.0,
                            b: f64::from(rgba.b) / 255.0,
                            a: f64::from(rgba.a) / 255.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            if let Some(vertices) = &vertices {
                pass.set_vertex_buffer(0, vertices.slice(..));
                for (index, piece) in pieces.iter().enumerate() {
                    let binding = match &piece.pixels {
                        Pixels::Tile(key) => &self.tiles[key].pixels.binding,
                        Pixels::Atlas(_) => &self.atlas.as_ref().unwrap().binding,
                    };
                    pass.set_bind_group(0, binding, &[]);
                    pass.draw(0..6, index as u32..index as u32 + 1);
                }
            }
        }
        queue.submit([encoder.finish()]);
        self.trim(&active);
        self.pending = stats.pending;
        stats.submit_ms = started.elapsed().as_secs_f32() * 1000.0;
        Ok(Some(stats))
    }

    fn trim(&mut self, active: &HashSet<TileKey>) {
        while self.bytes > CACHE_BUDGET {
            let victim = self
                .tiles
                .iter()
                .min_by_key(|(key, tile)| (active.contains(*key), tile.used))
                .map(|(key, _)| key.clone());
            let Some(key) = victim else {
                break;
            };
            self.bytes -= texture_bytes(self.tiles.remove(&key).unwrap().pixels.size);
        }
    }
}

fn texture_bytes(size: [u32; 2]) -> usize {
    size[0] as usize * size[1] as usize * 4
}

fn subpixel_phase(offset: f64) -> u16 {
    ((offset.rem_euclid(1.0) * 256.0).round() as u16) % 256
}
fn rect_size(rect: Rect) -> [u32; 2] {
    [rect.width() as u32, rect.height() as u32]
}
fn scale_rect(rect: Rect, scale: f64) -> Rect {
    Rect::new(
        rect.x0 * scale,
        rect.y0 * scale,
        rect.x1 * scale,
        rect.y1 * scale,
    )
}
fn padded_pixels(rect: Rect) -> Rect {
    Rect::new(
        rect.x0.floor() - f64::from(GUTTER),
        rect.y0.floor() - f64::from(GUTTER),
        rect.x1.ceil() + f64::from(GUTTER),
        rect.y1.ceil() + f64::from(GUTTER),
    )
}
fn quad(piece: &Piece, size: [u32; 2], texture: [u32; 2], atlas_y: f64) -> Quad {
    let dest = piece.visible + kurbo::Vec2::new(piece.offset.0, piece.offset.1);
    Quad {
        rect: [
            (dest.x0 / f64::from(size[0]) * 2.0 - 1.0) as f32,
            (1.0 - dest.y0 / f64::from(size[1]) * 2.0) as f32,
            (dest.x1 / f64::from(size[0]) * 2.0 - 1.0) as f32,
            (1.0 - dest.y1 / f64::from(size[1]) * 2.0) as f32,
        ],
        uv: [
            ((piece.visible.x0 - piece.bounds.x0) / f64::from(texture[0])) as f32,
            ((piece.visible.y0 - piece.bounds.y0 + atlas_y) / f64::from(texture[1])) as f32,
            ((piece.visible.x1 - piece.bounds.x0) / f64::from(texture[0])) as f32,
            ((piece.visible.y1 - piece.bounds.y0 + atlas_y) / f64::from(texture[1])) as f32,
        ],
    }
}

#[cfg(test)]
#[path = "reader_layers_tests.rs"]
pub(crate) mod tests;
