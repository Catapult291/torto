use super::model::Screenshot;
use crate::async_task::TaskResult;
use base64::Engine;
use std::io::Cursor;

pub(super) const MAX_IMAGES: usize = 3;
const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub(super) enum Source {
    Clipboard,
    Pick,
    Restore(Vec<Screenshot>),
}

pub(crate) type Message = TaskResult<Vec<Prepared>>;
pub(crate) type PreviewMessage = TaskResult<egui::ColorImage>;
pub(crate) struct Prepared {
    pub(super) screenshot: Screenshot,
    pub(super) thumbnail: egui::ColorImage,
}

pub(super) async fn prepare(source: Source) -> Result<Vec<Prepared>, String> {
    if let Source::Pick = source {
        let paths = rfd::AsyncFileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp"])
            .pick_files()
            .await;
        return tokio::task::spawn_blocking(move || {
            let Some(paths) = paths else {
                return Ok(Vec::new());
            };
            if paths.len() > MAX_IMAGES {
                return Err("limit".into());
            }
            paths
                .into_iter()
                .map(|path| {
                    if std::fs::metadata(path.path()).map_err(|_| "image")?.len() > 20 * 1024 * 1024
                    {
                        return Err("image".into());
                    }
                    let mut reader = image::ImageReader::open(path.path())
                        .map_err(|_| "image")?
                        .with_guessed_format()
                        .map_err(|_| "image")?;
                    let mut limits = image::Limits::default();
                    limits.max_alloc = Some(64 * 1024 * 1024);
                    limits.max_image_width = Some(8192);
                    limits.max_image_height = Some(8192);
                    reader.limits(limits);
                    encode(reader.decode().map_err(|_| "image")?)
                })
                .collect()
        })
        .await
        .map_err(|_| "image".to_owned())?;
    }
    tokio::task::spawn_blocking(move || match source {
        Source::Clipboard => {
            let images = crate::platform::feedback_images();
            if images.len() > MAX_IMAGES {
                return Err("limit".into());
            }
            images
                .into_iter()
                .map(|pixels| {
                    if u64::from(pixels.width) * u64::from(pixels.height) > 16 * 1024 * 1024 {
                        return Err("image".into());
                    }
                    let image =
                        image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.rgba)
                            .ok_or("image")?;
                    encode(image::DynamicImage::ImageRgba8(image))
                })
                .collect()
        }
        Source::Restore(images) => {
            if images.len() > MAX_IMAGES {
                return Err("image".into());
            }
            images.into_iter().map(restore).collect()
        }
        Source::Pick => unreachable!(),
    })
    .await
    .map_err(|_| "image".to_owned())?
}

pub(super) fn restore(screenshot: Screenshot) -> Result<Prepared, String> {
    let image = decode(&screenshot)?;
    Ok(Prepared {
        screenshot,
        thumbnail: thumbnail(&image),
    })
}

pub(super) async fn preview(screenshot: Screenshot) -> Result<egui::ColorImage, String> {
    tokio::task::spawn_blocking(move || {
        let pixels = decode(&screenshot)?.to_rgba8();
        Ok(egui::ColorImage::from_rgba_unmultiplied(
            [pixels.width() as usize, pixels.height() as usize],
            pixels.as_raw(),
        ))
    })
    .await
    .map_err(|_| "image".to_owned())?
}

fn decode(screenshot: &Screenshot) -> Result<image::DynamicImage, String> {
    if screenshot.data.len() > MAX_BYTES.div_ceil(3) * 4 {
        return Err("image".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&screenshot.data)
        .map_err(|_| "image")?;
    let format = image::guess_format(&bytes).map_err(|_| "image")?;
    if !matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg) {
        return Err("image".into());
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(32 * 1024 * 1024);
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    reader.limits(limits);
    reader.decode().map_err(|_| "image".into())
}

pub(super) fn encode(image: image::DynamicImage) -> Result<Prepared, String> {
    // Clipboard images often have an alpha channel even when every pixel is opaque.
    let transparent = match &image {
        image::DynamicImage::ImageLumaA8(pixels) => pixels.pixels().any(|p| p[1] < 255),
        image::DynamicImage::ImageRgba8(pixels) => pixels.pixels().any(|p| p[3] < 255),
        image::DynamicImage::ImageLumaA16(pixels) => pixels.pixels().any(|p| p[1] < 65535),
        image::DynamicImage::ImageRgba16(pixels) => pixels.pixels().any(|p| p[3] < 65535),
        image::DynamicImage::ImageRgba32F(pixels) => pixels.pixels().any(|p| p[3] != 1.0),
        _ => image.color().has_alpha(),
    };
    let mut image = image.thumbnail(2048, 2048);
    if !transparent {
        image = image::DynamicImage::ImageRgb8(image.to_rgb8());
    }
    loop {
        let mut bytes = Cursor::new(Vec::new());
        if transparent {
            image.write_to(&mut bytes, image::ImageFormat::Png)
        } else {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 92).encode_image(&image)
        }
        .map_err(|_| "image")?;
        let bytes = bytes.into_inner();
        if bytes.len() <= MAX_BYTES {
            return Ok(Prepared {
                screenshot: Screenshot {
                    data: base64::engine::general_purpose::STANDARD.encode(bytes),
                },
                thumbnail: thumbnail(&image),
            });
        }
        if image.width().max(image.height()) <= 512 {
            return Err("image".into());
        }
        image = image.thumbnail(
            (image.width() * 3 / 4).max(1),
            (image.height() * 3 / 4).max(1),
        );
    }
}

fn thumbnail(image: &image::DynamicImage) -> egui::ColorImage {
    let pixels = image.thumbnail(160, 112).to_rgba8();
    egui::ColorImage::from_rgba_unmultiplied(
        [pixels.width() as usize, pixels.height() as usize],
        pixels.as_raw(),
    )
}
