//! Media loading and export module (static images, GIF, and WebP animations).

use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;

use anyhow::{bail, Context, Result};
use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage};

use crate::jpeg8c;
use crate::polar::{decode_polar_to_image, encode_image_to_polar_ext, PolarConfig};

/// Options for transforming source media into polar frames.
#[derive(Debug, Clone, Copy)]
pub struct MediaLoadOptions {
    /// Preserve aspect ratio with centering on black background.
    pub preserve_aspect: bool,
    /// Image rotation angle in degrees (0, 90, 180, 270).
    pub rotate_deg: i32,
    /// Vertical flip flag.
    pub flip_v: bool,
    /// Frame repetitions for static images (e.g. 15 frames for ~1 second display).
    pub static_frame_count: usize,
}

impl Default for MediaLoadOptions {
    fn default() -> Self {
        Self {
            preserve_aspect: true,
            rotate_deg: 0,
            flip_v: false,
            static_frame_count: 15,
        }
    }
}

/// Loads raw image frames from media file (static images, GIF, or animated WebP).
pub fn load_media_raw_frames(path: &Path) -> Result<Vec<DynamicImage>> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "gif" => {
            let file = File::open(path)
                .with_context(|| format!("Failed to open GIF: {}", path.display()))?;
            let reader = BufReader::new(file);
            let decoder = GifDecoder::new(reader)
                .with_context(|| format!("Failed to decode GIF: {}", path.display()))?;
            let frames = decoder
                .into_frames()
                .collect_frames()
                .with_context(|| format!("Failed to extract GIF frames: {}", path.display()))?;
            if frames.is_empty() {
                bail!("GIF file contains no frames: {}", path.display());
            }
            Ok(frames
                .into_iter()
                .map(|f| DynamicImage::ImageRgba8(f.into_buffer()))
                .collect())
        }
        "webp" => {
            let file = File::open(path)
                .with_context(|| format!("Failed to open WebP: {}", path.display()))?;
            let reader = BufReader::new(file);
            let decoder = WebPDecoder::new(reader)
                .with_context(|| format!("Failed to decode WebP: {}", path.display()))?;

            if decoder.has_animation() {
                let frames = decoder.into_frames().collect_frames().with_context(|| {
                    format!("Failed to extract WebP frames: {}", path.display())
                })?;
                if frames.is_empty() {
                    bail!("WebP file contains no frames: {}", path.display());
                }
                Ok(frames
                    .into_iter()
                    .map(|f| DynamicImage::ImageRgba8(f.into_buffer()))
                    .collect())
            } else {
                let img = open_image(path)
                    .with_context(|| format!("Failed to open static WebP: {}", path.display()))?;
                Ok(vec![img])
            }
        }
        _ => {
            let img = open_image(path)
                .with_context(|| format!("Failed to open image: {}", path.display()))?;
            Ok(vec![img])
        }
    }
}

/// Loads media file (image, GIF, or WebP) and converts its frames into polar format.
pub fn load_media_to_polar_frames(path: &Path, config: &PolarConfig) -> Result<Vec<Vec<u8>>> {
    load_media_to_polar_frames_with_options(path, config, &MediaLoadOptions::default())
}

/// Loads media file with specified transformation options and static duration.
pub fn load_media_to_polar_frames_with_options(
    path: &Path,
    config: &PolarConfig,
    options: &MediaLoadOptions,
) -> Result<Vec<Vec<u8>>> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "gif" => load_gif_frames(path, config, options),
        "webp" => load_webp_frames(path, config, options),
        _ => {
            let img = open_image(path)
                .with_context(|| format!("Failed to open image: {}", path.display()))?;
            let polar_frame = encode_image_to_polar_ext(
                &img,
                config,
                options.preserve_aspect,
                options.rotate_deg,
                options.flip_v,
            );
            let repeat = options.static_frame_count.max(1);
            Ok(vec![polar_frame; repeat])
        }
    }
}

/// Loads and converts animated GIF frames.
fn load_gif_frames(
    path: &Path,
    config: &PolarConfig,
    options: &MediaLoadOptions,
) -> Result<Vec<Vec<u8>>> {
    let file =
        File::open(path).with_context(|| format!("Failed to open GIF: {}", path.display()))?;
    let reader = BufReader::new(file);
    let decoder = GifDecoder::new(reader)
        .with_context(|| format!("Failed to decode GIF: {}", path.display()))?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .with_context(|| format!("Failed to extract frames from GIF: {}", path.display()))?;

    let mut result = Vec::with_capacity(frames.len());
    for frame in frames {
        let dynamic_img = DynamicImage::ImageRgba8(frame.into_buffer());
        let polar_buf = encode_image_to_polar_ext(
            &dynamic_img,
            config,
            options.preserve_aspect,
            options.rotate_deg,
            options.flip_v,
        );
        result.push(polar_buf);
    }

    Ok(result)
}

/// Loads and converts animated or static WebP frames.
fn load_webp_frames(
    path: &Path,
    config: &PolarConfig,
    options: &MediaLoadOptions,
) -> Result<Vec<Vec<u8>>> {
    let file =
        File::open(path).with_context(|| format!("Failed to open WebP: {}", path.display()))?;
    let reader = BufReader::new(file);
    let decoder = WebPDecoder::new(reader)
        .with_context(|| format!("Failed to decode WebP: {}", path.display()))?;

    if decoder.has_animation() {
        let frames = decoder
            .into_frames()
            .collect_frames()
            .with_context(|| format!("Failed to extract frames from WebP: {}", path.display()))?;
        let mut result = Vec::with_capacity(frames.len());
        for frame in frames {
            let dynamic_img = DynamicImage::ImageRgba8(frame.into_buffer());
            let polar_buf = encode_image_to_polar_ext(
                &dynamic_img,
                config,
                options.preserve_aspect,
                options.rotate_deg,
                options.flip_v,
            );
            result.push(polar_buf);
        }
        Ok(result)
    } else {
        let img = open_image(path)
            .with_context(|| format!("Failed to open static WebP: {}", path.display()))?;
        let polar_buf = encode_image_to_polar_ext(
            &img,
            config,
            options.preserve_aspect,
            options.rotate_deg,
            options.flip_v,
        );
        let repeat = options.static_frame_count.max(1);
        Ok(vec![polar_buf; repeat])
    }
}

pub fn open_image(path: &Path) -> Result<DynamicImage> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => Ok(DynamicImage::ImageRgb8(jpeg8c::open(path)?)),
        _ => {
            image::open(path).with_context(|| format!("Failed to decode image: {}", path.display()))
        }
    }
}

/// Exports an array of polar frames to a directory as preview PNG images.
pub fn export_polar_frames_to_png(
    frames: &[Vec<u8>],
    output_dir: &Path,
    config: &PolarConfig,
) -> Result<usize> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    for (idx, frame_data) in frames.iter().enumerate() {
        let reconstructed = decode_polar_to_image(frame_data, config)?;
        let filename = format!("frame_{:04}.png", idx + 1);
        let out_path = output_dir.join(filename);
        reconstructed
            .save(&out_path)
            .with_context(|| format!("Failed to save frame: {}", out_path.display()))?;
    }

    Ok(frames.len())
}
