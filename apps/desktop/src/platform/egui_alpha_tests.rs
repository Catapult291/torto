//! Native GPU checks: UI shape alpha tests alone cannot detect color-space errors.
use egui::{Color32, Pos2, Rect, TextureId, Vec2};
use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};

pub(crate) fn run_alpha_audit() {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .unwrap();
    println!("ALPHA_GPU {:?}", adapter.get_info());
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let mut failures = Vec::new();
    for format in [
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8Unorm,
    ] {
        let paint_format = super::egui_surface::paint_format(format);
        let views = super::egui_surface::view_formats(format);
        for dithering in [false, true] {
            let mut renderer = Renderer::new(
                &device,
                paint_format,
                RendererOptions {
                    dithering,
                    ..Default::default()
                },
            );
            // Include transparent image pixels as well as painter/Area opacity.
            for (color, background_color) in [
                (Color32::WHITE, Color32::WHITE),
                (
                    Color32::from_rgb(128, 64, 200),
                    Color32::from_rgb(128, 64, 200),
                ),
                (Color32::from_gray(24), Color32::from_gray(24)),
                (Color32::WHITE, Color32::from_gray(24)),
                (Color32::WHITE, Color32::from_gray(200)),
            ] {
                let background = background_color.to_array();
                let foreground = color.to_array();
                for (texture_alpha, opacity) in [
                    (255, 0.0),
                    (255, 0.1),
                    (255, 0.5),
                    (255, 0.9),
                    (255, 1.0),
                    (128, 0.5),
                    (0, 0.5),
                ] {
                    let image_color = Color32::from_rgba_unmultiplied(
                        foreground[0],
                        foreground[1],
                        foreground[2],
                        texture_alpha,
                    );
                    renderer.update_texture(
                        &device,
                        &queue,
                        TextureId::Managed(1),
                        &egui::epaint::ImageDelta::full(
                            egui::ColorImage::filled([1, 1], Color32::WHITE),
                            egui::TextureOptions::NEAREST,
                        ),
                    );
                    renderer.update_texture(
                        &device,
                        &queue,
                        TextureId::Managed(2),
                        &egui::epaint::ImageDelta::full(
                            egui::ColorImage::filled([1, 1], image_color),
                            egui::TextureOptions::NEAREST,
                        ),
                    );
                    let target = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("egui-alpha-audit"),
                        size: wgpu::Extent3d {
                            width: 8,
                            height: 8,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &views,
                    });
                    let screen = ScreenDescriptor {
                        size_in_pixels: [8, 8],
                        pixels_per_point: 1.0,
                    };
                    let mut jobs = Vec::new();
                    for (id, x, tint) in [
                        (TextureId::Managed(1), 0.0, color),
                        (TextureId::Managed(2), 4.0, Color32::WHITE),
                    ] {
                        let mut mesh = egui::epaint::Mesh::with_texture(id);
                        mesh.add_rect_with_uv(
                            Rect::from_min_size(Pos2::new(x, 0.0), Vec2::new(4.0, 8.0)),
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            tint.gamma_multiply(opacity),
                        );
                        jobs.push(egui::ClippedPrimitive {
                            clip_rect: Rect::from_min_size(Pos2::ZERO, Vec2::splat(8.0)),
                            primitive: egui::epaint::Primitive::Mesh(mesh),
                        });
                    }
                    let mut encoder = device.create_command_encoder(&Default::default());
                    let callbacks =
                        renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
                    let channel = |value: u8| {
                        let gamma = f64::from(value) / 255.0;
                        if !paint_format.is_srgb() {
                            gamma
                        } else if gamma <= 0.04045 {
                            gamma / 12.92
                        } else {
                            ((gamma + 0.055) / 1.055).powf(2.4)
                        }
                    };
                    let view = target.create_view(&wgpu::TextureViewDescriptor {
                        format: Some(paint_format),
                        ..Default::default()
                    });
                    {
                        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("egui-alpha-audit"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: channel(background[0]),
                                        g: channel(background[1]),
                                        b: channel(background[2]),
                                        a: 1.0,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            ..Default::default()
                        });
                        renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
                    }
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("alpha-readback"),
                        size: 256 * 8,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    encoder.copy_texture_to_buffer(
                        target.as_image_copy(),
                        wgpu::TexelCopyBufferInfo {
                            buffer: &buffer,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(256),
                                rows_per_image: Some(8),
                            },
                        },
                        wgpu::Extent3d {
                            width: 8,
                            height: 8,
                            depth_or_array_layers: 1,
                        },
                    );
                    queue.submit(callbacks.into_iter().chain([encoder.finish()]));
                    let (send, recv) = std::sync::mpsc::channel();
                    buffer
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, move |result| {
                            send.send(result).unwrap();
                        });
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: Some(std::time::Duration::from_secs(30)),
                        })
                        .unwrap();
                    recv.recv().unwrap().unwrap();
                    let mapped = buffer.slice(..).get_mapped_range().unwrap();
                    for (kind, x) in [("surface", 2), ("image", 6)] {
                        let at = 256 * 4 + x * 4;
                        let mut pixel: [u8; 4] = mapped[at..at + 4].try_into().unwrap();
                        if format == wgpu::TextureFormat::Bgra8UnormSrgb {
                            pixel.swap(0, 2);
                        }
                        let alpha = if kind == "surface" {
                            opacity
                        } else {
                            opacity * f32::from(texture_alpha) / 255.0
                        };
                        let mut expected = [0, 0, 0, 255];
                        for channel in 0..3 {
                            expected[channel] = (f32::from(foreground[channel]) * alpha
                                + f32::from(background[channel]) * (1.0 - alpha))
                                .round() as u8;
                        }
                        println!(
                            "ALPHA_PIXEL format={format:?} dithering={dithering} kind={kind} opacity={opacity} texture_alpha={texture_alpha} expected={expected:?} actual={pixel:?}"
                        );
                        // Fading over the same color preserves it. White over dark paper
                        // must brighten with opacity instead of taking a gray detour.
                        if !pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2) {
                            failures.push(format!("{kind}, {format:?}, opacity={opacity}, texture_alpha={texture_alpha}, {pixel:?}"));
                        }
                    }
                    drop(mapped);
                    buffer.unmap();
                }
            }
            renderer.free_texture(&TextureId::Managed(1));
            renderer.free_texture(&TextureId::Managed(2));
        }
    }
    assert!(
        failures.is_empty(),
        "fade changes the content's color: {failures:?}"
    );
}

#[test]
#[ignore = "requires a native GPU; verifies actual popup/image fade colors on sRGB and gamma targets"]
fn popup_and_image_fades_preserve_color_on_gpu() {
    run_alpha_audit();
}
