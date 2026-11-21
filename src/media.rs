//! Media loading and export module (static images, GIF, and WebP animations).

use std::{
    fs::{self, File},
    io::{BufReader, Cursor},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage};

use crate::polar::{
    decode_polar_to_image, encode_image_to_polar_ext, encode_image_to_polar_full, PolarConfig,
    TransformParams,
};
use crate::{format::MAX_SUPPORTED_FRAMES, jpeg8c};

pub struct EncodedGifTimeline {
    pub frames: Vec<Vec<u8>>,
    pub duration_seconds: f32,
}

#[derive(Clone)]
pub struct PlaylistMediaPlan {
    pub path: PathBuf,
    pub duration_seconds: f32,
    pub output_frames: usize,
    pub gif_source: Option<Arc<[u8]>>,
}

/// Inspects the playlist duration before importing so a whole batch can be checked against
/// the container frame limit before any item is appended.
pub fn plan_playlist_media(path: &Path, fps: f32) -> Result<PlaylistMediaPlan> {
    plan_playlist_media_cancellable(path, fps, || false)
}

pub fn plan_playlist_media_cancellable(
    path: &Path,
    fps: f32,
    cancelled: impl Fn() -> bool,
) -> Result<PlaylistMediaPlan> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
    {
        let source: Arc<[u8]> = std::fs::read(path)
            .with_context(|| format!("Failed to read GIF source {}", path.display()))?
            .into();
        let duration_seconds = probe_gif_duration_cancellable(&source, &cancelled)?;
        let output_frames = output_frame_count(duration_seconds, fps);
        anyhow::ensure!(
            output_frames <= MAX_SUPPORTED_FRAMES,
            "GIF requires {output_frames} output frames; the format supports at most {MAX_SUPPORTED_FRAMES}"
        );
        Ok(PlaylistMediaPlan {
            path: path.to_path_buf(),
            duration_seconds,
            output_frames,
            gif_source: Some(source),
        })
    } else {
        Ok(PlaylistMediaPlan {
            path: path.to_path_buf(),
            duration_seconds: 5.0,
            output_frames: output_frame_count(5.0, fps),
            gif_source: None,
        })
    }
}

pub fn probe_gif_duration(source: &[u8]) -> Result<f32> {
    probe_gif_duration_cancellable(source, || false)
}

pub fn probe_gif_duration_cancellable(source: &[u8], cancelled: impl Fn() -> bool) -> Result<f32> {
    let decoder = GifDecoder::new(BufReader::new(Cursor::new(source)))
        .context("Failed to decode GIF source")?;
    let mut duration = 0.0f64;
    let mut frames = 0usize;
    for frame in decoder.into_frames() {
        if cancelled() {
            bail!("GIF inspection cancelled");
        }
        let frame = frame.context("Failed to decode GIF frame")?;
        duration += gif_frame_delay_seconds(frame.delay());
        frames += 1;
    }
    anyhow::ensure!(frames > 0, "GIF contains no frames");
    anyhow::ensure!(
        duration.is_finite() && duration > 0.0,
        "GIF has no usable duration"
    );
    Ok(duration as f32)
}

/// Samples an animated GIF at the controller frame rate while preserving its frame delays.
/// `duration_seconds == None` uses one complete source animation; a longer explicit duration
/// loops the animation, and a shorter one trims its tail.
pub fn encode_gif_timeline(
    source: &[u8],
    config: &PolarConfig,
    params: &TransformParams,
    duration_seconds: Option<f32>,
    fps: f32,
    cancelled: impl Fn() -> bool,
) -> Result<EncodedGifTimeline> {
    anyhow::ensure!(fps.is_finite() && fps > 0.0, "Frame rate must be positive");
    if let Some(duration) = duration_seconds {
        anyhow::ensure!(
            duration.is_finite() && duration > 0.0,
            "GIF duration must be positive"
        );
    }

    let target_frames = duration_seconds.map(|duration| output_frame_count(duration, fps));
    if let Some(target) = target_frames {
        anyhow::ensure!(
            target <= MAX_SUPPORTED_FRAMES,
            "GIF duration requires {target} output frames; the format supports at most {MAX_SUPPORTED_FRAMES}"
        );
    }
    let mut output = Vec::with_capacity(target_frames.unwrap_or(0));
    let mut elapsed = 0.0f64;
    let mut sampler = GifTimelineSampler::new(fps as f64, target_frames);
    let mut last_polar: Option<Vec<u8>> = None;
    loop {
        if cancelled() {
            bail!("GIF import cancelled");
        }
        let decoder = GifDecoder::new(BufReader::new(Cursor::new(source)))
            .context("Failed to decode GIF source")?;
        let mut pass_frames = 0usize;
        for frame in decoder.into_frames() {
            if cancelled() {
                bail!("GIF import cancelled");
            }
            let frame = frame.context("Failed to decode GIF frame")?;
            let delay = gif_frame_delay_seconds(frame.delay());
            let copies = sampler.advance(delay);
            pass_frames += 1;
            elapsed += delay;

            if copies > 0 {
                let image = DynamicImage::ImageRgba8(frame.into_buffer());
                let polar = encode_image_to_polar_full(&image, config, params);
                last_polar = Some(polar.clone());
                for _ in 0..copies {
                    if cancelled() {
                        bail!("GIF import cancelled");
                    }
                    anyhow::ensure!(
                        output.len() < MAX_SUPPORTED_FRAMES,
                        "GIF duration exceeds the {MAX_SUPPORTED_FRAMES}-frame format limit"
                    );
                    output.push(polar.clone());
                }
            }
            if target_frames.is_some_and(|target| output.len() >= target) {
                break;
            }
        }

        anyhow::ensure!(pass_frames > 0, "GIF contains no frames");
        if target_frames.is_none() || target_frames.is_some_and(|target| output.len() >= target) {
            break;
        }
    }

    let duration = duration_seconds.unwrap_or(elapsed as f32);
    let expected_frames = output_frame_count(duration, fps);
    output.truncate(expected_frames);
    if output.len() < expected_frames {
        let last = last_polar.context("GIF produced no output frames")?;
        output.resize(expected_frames, last);
    }
    anyhow::ensure!(!output.is_empty(), "GIF produced no output frames");
    Ok(EncodedGifTimeline {
        frames: output,
        duration_seconds: duration,
    })
}

pub fn output_frame_count(duration_seconds: f32, fps: f32) -> usize {
    ((duration_seconds * fps).round() as usize).max(1)
}

fn gif_frame_delay_seconds(delay: image::Delay) -> f64 {
    let (numerator, denominator) = delay.numer_denom_ms();
    if denominator == 0 {
        0.01
    } else {
        (numerator as f64 / denominator as f64 / 1000.0).max(0.01)
    }
}

struct GifTimelineSampler {
    fps: f64,
    target_frames: Option<usize>,
    next_frame: usize,
    elapsed: f64,
}

impl GifTimelineSampler {
    fn new(fps: f64, target_frames: Option<usize>) -> Self {
        Self {
            fps,
            target_frames,
            next_frame: 0,
            elapsed: 0.0,
        }
    }

    /// Returns how many output samples fall inside this source frame's time interval.
    fn advance(&mut self, delay_seconds: f64) -> usize {
        let end = self.elapsed + delay_seconds;
        let mut count = 0;
        while self
            .target_frames
            .map_or(true, |target| self.next_frame < target)
            && (self.next_frame as f64 / self.fps) + 1e-9 < end
        {
            self.next_frame += 1;
            count += 1;
        }
        self.elapsed = end;
        count
    }
}

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

#[cfg(test)]
mod gif_timeline_tests {
    use super::*;

    #[test]
    fn sampler_preserves_source_timing_and_trims_at_requested_duration() {
        let mut sampler = GifTimelineSampler::new(10.0, Some(3));
        assert_eq!(sampler.advance(0.1), 1);
        assert_eq!(sampler.advance(0.1), 1);
        assert_eq!(sampler.advance(0.1), 1);
        assert_eq!(sampler.advance(0.1), 0);
        assert_eq!(sampler.next_frame, 3);
    }

    #[test]
    fn sampler_keeps_loop_timeline_continuous_until_selected_duration() {
        let mut sampler = GifTimelineSampler::new(10.0, Some(5));
        let first_loop: Vec<_> = (0..2).map(|_| sampler.advance(0.1)).collect();
        let second_loop: Vec<_> = (0..2).map(|_| sampler.advance(0.1)).collect();
        let last_loop: Vec<_> = (0..2).map(|_| sampler.advance(0.1)).collect();
        assert_eq!(first_loop, [1, 1]);
        assert_eq!(second_loop, [1, 1]);
        assert_eq!(last_loop, [1, 0]);
        assert_eq!(sampler.next_frame, 5);
    }

    #[test]
    fn frame_count_uses_controller_rate_and_rejects_no_positive_duration() {
        assert_eq!(output_frame_count(28.34, 15.0), 425);
        assert_eq!(output_frame_count(5.0, 15.0), 75);
        assert_eq!(output_frame_count(0.01, 15.0), 1);
    }
}
