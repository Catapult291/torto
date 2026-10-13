/// egui's colors and textures use gamma-space premultiplied alpha. Blending
/// them through an sRGB attachment decodes the already premultiplied colors,
/// darkening translucent paper, images and text during Area/preview fades.
/// Keep the native surface format, but paint through its non-sRGB view. This
/// matches egui's preferred framebuffer and needs no extra texture or copy.
pub(super) fn paint_format(surface: wgpu::TextureFormat) -> wgpu::TextureFormat {
    surface.remove_srgb_suffix()
}

pub(super) fn view_formats(surface: wgpu::TextureFormat) -> Vec<wgpu::TextureFormat> {
    let paint = paint_format(surface);
    if paint == surface {
        vec![surface]
    } else {
        vec![surface, paint]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_surface_views_match_the_egui_pipeline_without_duplicate_formats() {
        use wgpu::TextureFormat::*;
        for (surface, paint) in [
            (Rgba8UnormSrgb, Rgba8Unorm),
            (Bgra8UnormSrgb, Bgra8Unorm),
            (Rgba8Unorm, Rgba8Unorm),
            (Bgra8Unorm, Bgra8Unorm),
        ] {
            assert_eq!(paint_format(surface), paint);
            assert!(!paint.is_srgb());
            let views = view_formats(surface);
            assert!(views.contains(&paint));
            assert_eq!(views.len(), if surface == paint { 1 } else { 2 });
        }
    }
}
