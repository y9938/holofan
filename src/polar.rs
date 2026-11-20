//! Factory-compatible frame codec for the verified 112 × 450 LED display.
//!
//! Each angular line stores six MSB-first bitplanes from 112 palette-mapped BGR
//! samples. The 113,400-byte active payload is followed by zero padding to
//! 114,688 bytes. There is intentionally one supported profile: the supplied
//! vendor application and hardware evidence establish no alternate geometry.
//! The connected fan presents this profile rotated 90° clockwise, so encoding
//! compensates counterclockwise and decoded previews restore the physical view.

use anyhow::{bail, Result};
use image::{DynamicImage, GenericImageView, ImageBuffer, Rgb, Rgba, RgbaImage};

/// One immutable profile describing the verified factory frame format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PolarConfig;

impl PolarConfig {
    /// Clockwise rotation applied by the physical display to encoded frames.
    pub const DISPLAY_ROTATION_DEG: i32 = 90;
    pub const LED_COUNT: usize = 112;
    pub const LINE_COUNT: usize = 450;
    pub const IMAGE_SIZE: u32 = 450;
    pub const ACTIVE_LINE_SIZE: usize = 252;
    pub const ACTIVE_FRAME_SIZE: usize = 113_400;
    pub const FRAME_PADDED_SIZE: usize = 114_688;

    pub fn standard() -> Self {
        Self
    }

    pub fn led_count(&self) -> usize {
        Self::LED_COUNT
    }

    pub fn line_count(&self) -> usize {
        Self::LINE_COUNT
    }

    pub fn image_size(&self) -> u32 {
        Self::IMAGE_SIZE
    }

    pub fn active_frame_size(&self) -> usize {
        Self::ACTIVE_FRAME_SIZE
    }

    pub fn frame_padded_size(&self) -> usize {
        Self::FRAME_PADDED_SIZE
    }
}

/// Image fit mode for the circular POV display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FitMode {
    /// Fill the square canvas by scaling and center-cropping, matching factory import.
    #[default]
    Fill = 0,
    /// Fit the complete image inside the canvas, leaving black borders as needed.
    Fit = 1,
    /// Stretch across the canvas without preserving aspect ratio.
    Stretch = 2,
}

/// Image framing and orientation controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransformParams {
    pub fit_mode: FitMode,
    pub rotate_deg: i32,
    pub flip_v: bool,
    pub flip_h: bool,
    pub zoom: f32,
    pub offset_x: i32,
    pub offset_y: i32,
}

impl Default for TransformParams {
    fn default() -> Self {
        Self {
            fit_mode: FitMode::Fill,
            rotate_deg: 0,
            flip_v: false,
            flip_h: false,
            zoom: 1.0,
            offset_x: 0,
            offset_y: 0,
        }
    }
}

/// Applies orientation, fit, zoom, and pan controls using Qt-compatible fast scaling.
pub fn transform_image_to_canvas_full(
    img: &DynamicImage,
    target_size: u32,
    params: &TransformParams,
) -> DynamicImage {
    let mut prepared = img.clone();
    match params.rotate_deg.rem_euclid(360) {
        90 => prepared = prepared.rotate90(),
        180 => prepared = prepared.rotate180(),
        270 => prepared = prepared.rotate270(),
        _ => {}
    }
    if params.flip_v {
        prepared = prepared.flipv();
    }
    if params.flip_h {
        prepared = prepared.fliph();
    }

    let (orig_w, orig_h) = prepared.dimensions();
    if orig_w == 0 || orig_h == 0 || target_size == 0 {
        return prepared;
    }
    let zoom = params.zoom.clamp(0.2, 4.0) as f64;
    let scale_x = target_size as f64 / orig_w as f64;
    let scale_y = target_size as f64 / orig_h as f64;
    let (base_w, base_h) = match params.fit_mode {
        FitMode::Fill => (
            (orig_w as f64 * scale_y.max(scale_x) * zoom).round() as u32,
            (orig_h as f64 * scale_y.max(scale_x) * zoom).round() as u32,
        ),
        FitMode::Fit => (
            (orig_w as f64 * scale_y.min(scale_x) * zoom).round() as u32,
            (orig_h as f64 * scale_y.min(scale_x) * zoom).round() as u32,
        ),
        FitMode::Stretch => (
            (target_size as f64 * zoom).round() as u32,
            (target_size as f64 * zoom).round() as u32,
        ),
    };
    let base_w = base_w.max(1);
    let base_h = base_h.max(1);
    let resized = qt_fast_scale(&prepared, base_w, base_h);
    let mut canvas = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(
        target_size,
        target_size,
        Rgba([0, 0, 0, 255]),
    ));
    let x = (target_size as i64 - base_w as i64) / 2 + params.offset_x as i64;
    let y = (target_size as i64 - base_h as i64) / 2 + params.offset_y as i64;
    image::imageops::overlay(&mut canvas, &resized, x, y);
    canvas
}

/// Transforms a source image with the legacy simple fit controls.
pub fn transform_image_to_canvas(
    img: &DynamicImage,
    target_size: u32,
    preserve_aspect: bool,
    rotate_deg: i32,
    flip_v: bool,
    zoom: f32,
) -> DynamicImage {
    let params = TransformParams {
        fit_mode: if preserve_aspect {
            FitMode::Fill
        } else {
            FitMode::Stretch
        },
        rotate_deg,
        flip_v,
        flip_h: false,
        zoom,
        offset_x: 0,
        offset_y: 0,
    };
    transform_image_to_canvas_full(img, target_size, &params)
}

/// Fits an image to the factory canvas using the same fast scaler as the vendor app.
pub fn fit_image_to_canvas(
    img: &DynamicImage,
    target_size: u32,
    preserve_aspect: bool,
) -> DynamicImage {
    transform_image_to_canvas(img, target_size, preserve_aspect, 0, false, 1.0)
}

/// Encodes an image into a factory-compatible polar frame.
pub fn encode_image_to_polar(img: &DynamicImage, config: &PolarConfig) -> Vec<u8> {
    encode_image_to_polar_full(img, config, &TransformParams::default())
}

/// Encodes an image with framing and orientation controls.
pub fn encode_image_to_polar_full(
    img: &DynamicImage,
    _config: &PolarConfig,
    params: &TransformParams,
) -> Vec<u8> {
    let canvas = transform_image_to_canvas_full(img, PolarConfig::IMAGE_SIZE, params);
    // The encoder uses the factory coordinate system. Compensate for this
    // fan's clockwise physical display offset at the image-to-frame boundary,
    // leaving the verified low-level polar codec byte-compatible with factory
    // frames.
    let compensated = match PolarConfig::DISPLAY_ROTATION_DEG.rem_euclid(360) {
        90 => canvas.rotate270(),
        180 => canvas.rotate180(),
        270 => canvas.rotate90(),
        _ => canvas,
    };
    encode_prepared_canvas_to_polar(&compensated, _config)
}

/// Encodes an image with aspect, rotation, and vertical-flip controls.
pub fn encode_image_to_polar_ext(
    img: &DynamicImage,
    config: &PolarConfig,
    preserve_aspect: bool,
    rotate_deg: i32,
    flip_v: bool,
) -> Vec<u8> {
    let params = TransformParams {
        fit_mode: if preserve_aspect {
            FitMode::Fill
        } else {
            FitMode::Stretch
        },
        rotate_deg,
        flip_v,
        ..TransformParams::default()
    };
    encode_image_to_polar_full(img, config, &params)
}

/// Encodes an image with fit, rotation, flip, and zoom controls.
pub fn encode_transformed_image_to_polar(
    img: &DynamicImage,
    config: &PolarConfig,
    preserve_aspect: bool,
    rotate_deg: i32,
    flip_v: bool,
    zoom: f32,
) -> Vec<u8> {
    let params = TransformParams {
        fit_mode: if preserve_aspect {
            FitMode::Fill
        } else {
            FitMode::Stretch
        },
        rotate_deg,
        flip_v,
        zoom,
        ..TransformParams::default()
    };
    encode_image_to_polar_full(img, config, &params)
}

/// Encodes a 450×450 canvas using palette mapping and six bitplanes per line.
pub fn encode_prepared_canvas_to_polar(_canvas: &DynamicImage, _config: &PolarConfig) -> Vec<u8> {
    let mut frame = vec![0; PolarConfig::FRAME_PADDED_SIZE];
    let canvas = if _canvas.width() == PolarConfig::IMAGE_SIZE
        && _canvas.height() == PolarConfig::IMAGE_SIZE
    {
        _canvas.to_rgb8()
    } else {
        qt_fast_scale(_canvas, PolarConfig::IMAGE_SIZE, PolarConfig::IMAGE_SIZE).to_rgb8()
    };
    for line in 0..PolarConfig::LINE_COUNT {
        let angle = (90.0 + line as f64 * (360.0 / PolarConfig::LINE_COUNT as f64)).to_radians();
        let cos = angle.cos();
        let sin = angle.sin();
        let line_start = line * PolarConfig::ACTIVE_LINE_SIZE;

        for plane in 0..6 {
            let plane_start = line_start + plane * 42;
            for packed_bit in 0..(PolarConfig::LED_COUNT * 3) {
                let sample = packed_bit / 3;
                let channel = packed_bit % 3; // Vendor append order: B, G, R.
                let radius_ratio = (PolarConfig::LED_COUNT - 1 - sample) as f64
                    / (PolarConfig::LED_COUNT - 1) as f64;
                let x = (225.0 + radius_ratio * 224.0 * cos + 0.5)
                    .floor()
                    .clamp(0.0, 449.0) as u32;
                let y = (225.0 - radius_ratio * 224.0 * sin + 0.5)
                    .floor()
                    .clamp(0.0, 449.0) as u32;
                let pixel = canvas.get_pixel(x, y);
                let mapped = [
                    palette_gb(pixel[2]),
                    palette_gb(pixel[1]),
                    qt_palette_red(pixel[0]),
                ];
                let bit = (mapped[channel] >> (7 - plane)) & 1;
                let byte_index = plane_start + packed_bit / 8;
                frame[byte_index] |= bit << (7 - packed_bit % 8);
            }
        }
    }
    frame
}

/// Decodes the factory bitplanes to the quantized 450×450 preview raster.
pub fn decode_polar_to_image(
    data: &[u8],
    _config: &PolarConfig,
) -> Result<ImageBuffer<Rgb<u8>, Vec<u8>>> {
    if data.len() != PolarConfig::FRAME_PADDED_SIZE {
        bail!(
            "Invalid frame size: expected {} bytes, found {}",
            PolarConfig::FRAME_PADDED_SIZE,
            data.len()
        );
    }
    if data[PolarConfig::ACTIVE_FRAME_SIZE..]
        .iter()
        .any(|byte| *byte != 0)
    {
        bail!("Invalid frame padding: expected zero bytes after active payload");
    }

    // The encoded frame stores discrete LEDs along radial spokes. Scattering
    // one source sample into one canvas pixel leaves visible gaps between the
    // 0.8° spokes. Reconstruct the POV image per output pixel instead, blending
    // the four neighbouring angular/radial LED samples.
    let mut canvas = ImageBuffer::from_pixel(
        PolarConfig::IMAGE_SIZE,
        PolarConfig::IMAGE_SIZE,
        Rgb([0, 0, 0]),
    );
    let samples: Vec<[u8; 3]> = (0..PolarConfig::LINE_COUNT)
        .flat_map(|line| {
            (0..PolarConfig::LED_COUNT).map(move |sample| decode_sample(data, line, sample))
        })
        .collect();
    for y in 0..PolarConfig::IMAGE_SIZE {
        for x in 0..PolarConfig::IMAGE_SIZE {
            let dx = x as f64 - 225.0;
            let dy = 225.0 - y as f64;
            let radius = dx.hypot(dy);
            if radius > 224.0 {
                continue;
            }

            let angle = (dy.atan2(dx).to_degrees() - 90.0).rem_euclid(360.0)
                / (360.0 / PolarConfig::LINE_COUNT as f64);
            let line0 = angle.floor() as usize;
            let line1 = (line0 + 1) % PolarConfig::LINE_COUNT;
            let line_t = angle - line0 as f64;

            let sample = (PolarConfig::LED_COUNT - 1) as f64 * (1.0 - radius / 224.0);
            let sample0 = sample.floor() as usize;
            let sample1 = (sample0 + 1).min(PolarConfig::LED_COUNT - 1);
            let sample_t = sample - sample0 as f64;

            let a = samples[line0 * PolarConfig::LED_COUNT + sample0];
            let b = samples[line1 * PolarConfig::LED_COUNT + sample0];
            let c = samples[line0 * PolarConfig::LED_COUNT + sample1];
            let d = samples[line1 * PolarConfig::LED_COUNT + sample1];
            let mut rgb = [0; 3];
            for channel in 0..3 {
                let radial0 = a[channel] as f64 * (1.0 - line_t) + b[channel] as f64 * line_t;
                let radial1 = c[channel] as f64 * (1.0 - line_t) + d[channel] as f64 * line_t;
                rgb[channel] = (radial0 * (1.0 - sample_t) + radial1 * sample_t)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            canvas.put_pixel(x, y, Rgb(rgb));
        }
    }
    // Preview decoded frames in the orientation that the fan physically shows.
    let preview = match PolarConfig::DISPLAY_ROTATION_DEG.rem_euclid(360) {
        90 => image::imageops::rotate90(&canvas),
        180 => image::imageops::rotate180(&canvas),
        270 => image::imageops::rotate270(&canvas),
        _ => canvas,
    };
    Ok(preview)
}

fn decode_sample(data: &[u8], line: usize, sample: usize) -> [u8; 3] {
    let line_start = line * PolarConfig::ACTIVE_LINE_SIZE;
    let mut bgr = [0; 3];
    for plane in 0..6 {
        for (channel, value) in bgr.iter_mut().enumerate() {
            let bit_index = sample * 3 + channel;
            let byte = data[line_start + plane * 42 + bit_index / 8];
            *value = (*value << 1) | ((byte >> (7 - bit_index % 8)) & 1);
        }
    }
    [bgr[2] << 2, bgr[1] << 2, bgr[0] << 2]
}

fn qt_fast_scale(image: &DynamicImage, width: u32, height: u32) -> DynamicImage {
    let source = image.to_rgba8();
    if source.width() == 0 || source.height() == 0 {
        return DynamicImage::ImageRgba8(ImageBuffer::from_pixel(
            width,
            height,
            Rgba([0, 0, 0, 255]),
        ));
    }
    DynamicImage::ImageRgba8(RgbaImage::from_fn(width, height, |x, y| {
        let sx = qt_source_index(x, source.width(), width);
        let sy = qt_source_index(y, source.height(), height);
        *source.get_pixel(sx, sy)
    }))
}

fn qt_source_index(destination: u32, source_size: u32, destination_size: u32) -> u32 {
    let ratio = source_size as f64 / destination_size as f64;
    let source = ((destination as f64 + 0.5) * ratio - 1.0 / 65_536.0).floor();
    source.clamp(0.0, (source_size - 1) as f64) as u32
}

fn palette_key(component: u8) -> usize {
    (((component as usize + 7) / 8).max(1) - 1).min(31)
}

fn palette_gb(component: u8) -> u8 {
    const BLUE: [u8; 32] = [
        0, 1, 2, 3, 4, 5, 6, 7, 10, 12, 14, 16, 18, 20, 23, 26, 30, 34, 38, 42, 47, 52, 58, 64, 71,
        78, 86, 94, 104, 112, 120, 128,
    ];
    BLUE[palette_key(component)]
}

fn qt_palette_red(component: u8) -> u8 {
    const RED: [u8; 32] = [
        0, 0, 1, 3, 5, 8, 11, 15, 20, 25, 30, 36, 42, 48, 55, 63, 71, 79, 88, 97, 106, 116, 126,
        137, 150, 165, 180, 195, 210, 225, 240, 255,
    ];
    RED[palette_key(component)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_profile_has_verified_dimensions_and_exact_frame_size() {
        let config = PolarConfig::standard();
        assert_eq!(config.led_count(), 112);
        assert_eq!(config.line_count(), 450);
        assert_eq!(config.image_size(), 450);
        assert_eq!(config.active_frame_size(), 113_400);
        assert_eq!(config.frame_padded_size(), 114_688);
    }

    #[test]
    fn constant_red_frame_locks_channel_and_bitplane_order() {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(450, 450, Rgb([255, 0, 0])));
        let frame = encode_prepared_canvas_to_polar(&image, &PolarConfig::standard());
        assert_eq!(frame.len(), 114_688);
        let mut expected_plane = [0u8; 42];
        for packed_bit in 0..(PolarConfig::LED_COUNT * 3) {
            if packed_bit % 3 == 2 {
                expected_plane[packed_bit / 8] |= 1 << (7 - packed_bit % 8);
            }
        }
        for line in frame[..PolarConfig::ACTIVE_FRAME_SIZE].chunks_exact(252) {
            for plane in line.chunks_exact(42) {
                assert_eq!(plane, expected_plane);
            }
        }
        assert!(frame[PolarConfig::ACTIVE_FRAME_SIZE..]
            .iter()
            .all(|byte| *byte == 0));
    }

    #[test]
    fn decoder_rejects_wrong_frame_size_and_nonzero_padding() {
        let config = PolarConfig::standard();
        assert!(decode_polar_to_image(&[], &config).is_err());
        let mut frame = vec![0; PolarConfig::FRAME_PADDED_SIZE];
        frame[PolarConfig::ACTIVE_FRAME_SIZE] = 1;
        assert!(decode_polar_to_image(&frame, &config).is_err());
    }

    #[test]
    fn decoded_preview_interpolates_between_led_samples() {
        let config = PolarConfig::standard();
        let source = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(
            PolarConfig::IMAGE_SIZE,
            PolarConfig::IMAGE_SIZE,
            Rgb([255, 0, 0]),
        ));
        let frame = encode_prepared_canvas_to_polar(&source, &config);
        let preview = decode_polar_to_image(&frame, &config).unwrap();

        assert_eq!(preview.get_pixel(225, 1), &Rgb([252, 0, 0]));
        assert!(preview.get_pixel(226, 2)[0] > 0);
        assert_eq!(preview.get_pixel(0, 0), &Rgb([0, 0, 0]));
    }

    #[test]
    fn palette_quantization_uses_factory_lookup_and_ceiling_key() {
        assert_eq!(qt_palette_red(0), 0);
        assert_eq!(qt_palette_red(8), 0);
        assert_eq!(qt_palette_red(17), 1);
        assert_eq!(qt_palette_red(255), 255);
        assert_eq!(palette_gb(255), 128);
    }
}
