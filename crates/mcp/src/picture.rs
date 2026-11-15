use std::io::Cursor;

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader, RgbImage};

pub const MAX_SIDE: u32 = 1568;

pub const PASS_BYTES: usize = 3_600_000;

const QUALITY: u8 = 85;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub data: Vec<u8>,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
    pub shown: (u32, u32),
}

#[must_use]
pub fn is_image(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".webp", ".gif"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

fn mime_of(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::WebP => Some("image/webp"),
        ImageFormat::Gif => Some("image/gif"),
        _ => None,
    }
}

fn reader(bytes: &[u8]) -> Result<ImageReader<Cursor<&[u8]>>> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("Couldn't read the image")
}

pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    reader(bytes)?
        .into_dimensions()
        .context("Couldn't read the image size")
}

fn on_white(img: &DynamicImage) -> RgbImage {
    let rgba = img.to_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = u16::from(p[3]);
        let mix = |c: u8| u8::try_from((u16::from(c) * a + 255 * (255 - a)) / 255).unwrap_or(255);
        out.put_pixel(x, y, image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])]));
    }
    out
}

pub fn prepare(bytes: &[u8], max_side: u32, pass_bytes: usize) -> Result<Picture> {
    let r = reader(bytes)?;
    let format = r.format().context("Unknown image format")?;
    let mime = mime_of(format).context("Unsupported image format")?;
    let (width, height) = dimensions(bytes)?;
    if width.max(height) <= max_side && bytes.len() <= pass_bytes {
        return Ok(Picture {
            data: bytes.to_vec(),
            mime,
            width,
            height,
            shown: (width, height),
        });
    }
    let img = r.decode().context("Couldn't decode the image")?;
    let scaled = if width.max(height) > max_side {
        img.resize(max_side, max_side, FilterType::Triangle)
    } else {
        img
    };
    let mut data = Vec::new();
    JpegEncoder::new_with_quality(&mut data, QUALITY)
        .encode_image(&on_white(&scaled))
        .context("Couldn't encode the image")?;
    Ok(Picture {
        data,
        mime: "image/jpeg",
        width,
        height,
        shown: scaled.dimensions(),
    })
}

#[cfg(test)]
#[path = "picture_tests.rs"]
mod tests;
