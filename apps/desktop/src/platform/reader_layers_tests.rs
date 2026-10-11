use super::*;
use crate::reader::render::{LayerContent, ReaderLayer};

pub(crate) struct LayerAudit {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    retained: LayerRenderer,
    target: wgpu::Texture,
    size: [u32; 2],
}

impl LayerAudit {
    pub(crate) fn new(size: [u32; 2]) -> Self {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .unwrap();
        println!("LAYER_GPU {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .unwrap();
        let renderer = Renderer::new(
            &device,
            vello::RendererOptions {
                antialiasing_support: vello::AaSupport::area_only(),
                ..Default::default()
            },
        )
        .unwrap();
        let retained = LayerRenderer::new(&device);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer-audit-target"),
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
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            device,
            queue,
            renderer,
            retained,
            target,
            size,
        }
    }

    pub(crate) fn wait(&self) {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
    }

    pub(crate) fn direct(
        &mut self,
        scene: &crate::reader::ReaderScene,
        plan: ReaderFramePlan,
        scale: f32,
    ) -> f32 {
        let started = Instant::now();
        let mut physical = Scene::new();
        physical.append(&scene.scene, Some(Affine::scale(f64::from(scale))));
        for image in scene.images.iter() {
            self.renderer.mark_override_image_dirty(image);
        }
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                &physical,
                &self.target.create_view(&Default::default()),
                &RenderParams {
                    base_color: plan.background,
                    width: self.size[0],
                    height: self.size[1],
                    antialiasing_method: AaConfig::Area,
                },
            )
            .unwrap();
        self.wait();
        started.elapsed().as_secs_f32() * 1000.0
    }

    pub(crate) fn layered(
        &mut self,
        scene: &LayeredReaderScene,
        plan: ReaderFramePlan,
        scale: f32,
    ) -> (LayerStats, f32) {
        let started = Instant::now();
        let stats = self
            .retained
            .render(
                &self.device,
                &self.queue,
                &mut self.renderer,
                &self.target.create_view(&Default::default()),
                self.size,
                plan,
                scale,
                scene,
            )
            .unwrap()
            .unwrap();
        self.wait();
        (stats, started.elapsed().as_secs_f32() * 1000.0)
    }

    pub(crate) fn pixels(&self) -> Vec<u8> {
        let row = (self.size[0] * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layer-audit-readback"),
            size: u64::from(row * self.size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(self.size[1]),
                },
            },
            wgpu::Extent3d {
                width: self.size[0],
                height: self.size[1],
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let (send, recv) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                send.send(result).unwrap()
            });
        self.wait();
        recv.recv().unwrap().unwrap();
        let mapped = buffer.slice(..).get_mapped_range().unwrap();
        mapped
            .chunks_exact(row as usize)
            .flat_map(|row| row[..self.size[0] as usize * 4].iter().copied())
            .collect()
    }

    pub(crate) fn parity(&self, baseline: &[u8], label: &str) {
        let pixels = self.pixels();
        let changed = pixels
            .chunks_exact(4)
            .zip(baseline.chunks_exact(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 8))
            .count();
        let fraction = changed as f64 / (self.size[0] as f64 * self.size[1] as f64);
        println!("LAYER_PARITY label={label} changed_fraction={fraction:.6}");
        if let Some(out) = std::env::var_os("TORTO_LAYER_OUTPUT") {
            let out = std::path::PathBuf::from(out);
            std::fs::create_dir_all(&out).unwrap();
            for (name, data) in [("direct", baseline), ("layered", pixels.as_slice())] {
                image::save_buffer(
                    out.join(format!("{label}-{name}.png")),
                    data,
                    self.size[0],
                    self.size[1],
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
        assert!(
            fraction < 0.005,
            "layer ordering/clip/alpha changed fraction={fraction:.6}: {label}"
        );
    }
}

#[test]
fn composition_crops_keep_document_texture_coordinates() {
    let piece = Piece {
        pixels: Pixels::Atlas(0),
        bounds: Rect::new(-2.0, 510.0, 102.0, 1026.0),
        visible: Rect::new(0.0, 512.0, 100.0, 600.0),
        offset: (0.0, -500.0),
    };
    let result = quad(&piece, [100, 100], [128, 2048], 256.0);
    assert_eq!(result.rect, [-1.0, 0.76, 1.0, -1.0]);
    assert_eq!(
        result.uv,
        [2.0 / 128.0, 258.0 / 2048.0, 102.0 / 128.0, 346.0 / 2048.0]
    );
}

#[test]
#[ignore = "requires a native GPU; compares actual Vello pixels and retained cache behavior"]
fn retained_layers_preserve_masks_alpha_order_and_strip_seams() {
    let mut audit = LayerAudit::new([320, 640]);
    let plan = ReaderFramePlan {
        rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 640.0)),
        scene_id: 1,
        scene_revision: 1,
        background: Color::from_rgb8(248, 247, 243),
        content_left: 0.0,
        preview_left: 0.0,
        reflow_pending: false,
        live_resize_preview: false,
    };
    let mut base = Scene::new();
    base.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(240, 230, 210),
        None,
        &Rect::new(24.0, 0.0, 296.0, 1100.0),
    );
    let mut text = Scene::new();
    for y in (0..1100).step_by(24) {
        text.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgba8(20, 30, 40, 210),
            None,
            &Rect::new(40.0, y as f64, 280.0, y as f64 + 7.0),
        );
    }
    let mut overlay = Scene::new();
    overlay.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgba8(68, 137, 103, 72),
        None,
        &Rect::new(16.0, 490.0, 304.0, 550.0),
    );
    let clip = Rect::new(0.0, 0.0, 320.0, 1100.0);
    let mut layers = LayeredReaderScene {
        layers: vec![
            ReaderLayer {
                identity: Some((1, 0)),
                content: LayerContent::Scene(Arc::new(base)),
                clip,
                offset: (0.0, 0.0),
            },
            ReaderLayer {
                identity: None,
                content: LayerContent::Scene(Arc::new(overlay)),
                clip,
                offset: (0.0, 0.0),
            },
            ReaderLayer {
                identity: Some((1, 1)),
                content: LayerContent::Scene(Arc::new(text)),
                clip,
                offset: (0.0, 0.0),
            },
        ],
    };
    for (label, offset, scale) in [
        ("integer", 0.0, 1.0),
        ("scroll", 200.0, 1.0),
        ("fractional", 200.25, 1.0),
        ("dpi", 200.25, 1.25),
    ] {
        for layer in &mut layers.layers {
            layer.offset.1 = -offset;
        }
        let scene = crate::reader::ReaderScene {
            scene: Arc::new(layers.flatten()),
            images: Arc::from([]),
            refresh_image_atlas: false,
        };
        audit.direct(&scene, plan, scale);
        let baseline = audit.pixels();
        for frame in 0..8 {
            let (stats, _) = audit.layered(&layers, plan, scale);
            if frame == 0 {
                audit.parity(&baseline, &format!("{label}-first"));
            }
            if !stats.pending {
                break;
            }
        }
        audit.parity(&baseline, label);
        let (stats, _) = audit.layered(&layers, plan, scale);
        assert_eq!(stats.tiles, 0);
        assert_eq!(stats.image_bytes, 0);
        assert!(!stats.pending);
        assert!(stats.hits >= 4);
        assert!(audit.retained.cache_bytes() <= CACHE_BUDGET);
    }
    audit.retained.clear();
    assert_eq!(audit.retained.cache_bytes(), 0);
    assert_eq!(audit.retained.atlas_bytes(), 0);
    assert!(!audit.retained.pending());
}
