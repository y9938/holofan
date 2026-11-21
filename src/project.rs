//! Project playlist model and media management for holographic fan displays.
//!
//! Organizes sequences of items (images, GIFs, WebP animations),
//! handles item duration in seconds, rotation, scaling, mirroring,
//! item reordering, and direct export to removable storage media.

use anyhow::{bail, Context, Result};
use image::DynamicImage;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::format::{HologramContainer, PlaylistEntry, MAX_PLAYLIST_ENTRIES, MAX_SUPPORTED_FRAMES};
use crate::media::{self, MediaLoadOptions};
pub use crate::polar::TransformParams;
use crate::polar::{decode_polar_to_image, FitMode, PolarConfig};

/// Base frame rate used for duration calculation (15 FPS).
pub const DEFAULT_FPS: f32 = 15.0;
/// Default controller playback speed used for newly added media.
pub const DEFAULT_ITEM_SPEED: u8 = 50;

/// Project-owned source data used to regenerate encoded frames after edits.
#[derive(Debug, Clone)]
pub enum MediaSource {
    /// Raster images retained for still images and already-decoded containers.
    RasterFrames(Arc<[DynamicImage]>),
    /// Compressed GIF bytes; frames are decoded on demand instead of cached at full size.
    Gif(Arc<[u8]>),
}

/// Individual project item representing a static image or animation clip.
#[derive(Debug, Clone)]
pub struct ProjectItem {
    /// Unique item identifier within the project.
    pub id: usize,
    /// Display title (file name or label).
    pub title: String,
    /// Source file path on disk (if loaded from file).
    pub source_path: Option<PathBuf>,
    /// Global start frame offset (0-based).
    pub frame_start: usize,
    /// Number of polar frames allocated to this item.
    pub frame_count: usize,
    /// Controller playback speed parameter (1..255, default: 50).
    pub speed: u8,
    /// Playback mode flags (default: 1).
    pub mode: u8,

    // Interactive inspector and transformation parameters:
    /// Display duration in seconds (default: 5.0s).
    pub duration_seconds: f32,
    /// Rotation angle in degrees (0, 90, 180, 270).
    pub rotate_deg: i32,
    /// Vertical flip flag.
    pub flip_v: bool,
    /// Horizontal flip flag.
    pub flip_h: bool,
    /// Fit mode (Fit, Fill, Stretch).
    pub fit_mode: FitMode,
    /// Aspect ratio preservation flag (true if fit_mode != Stretch).
    pub preserve_aspect: bool,
    /// Zoom scale factor (1.0 = 100%, range 0.2..4.0).
    pub zoom: f32,
    /// Horizontal pan offset in pixels (-150..+150).
    pub offset_x: i32,
    /// Vertical pan offset in pixels (-150..+150).
    pub offset_y: i32,
    /// Project-owned source used to regenerate output after edits.
    pub media_source: MediaSource,
}

impl ProjectItem {
    /// Returns current transform parameters for this item.
    pub fn transform_params(&self) -> TransformParams {
        TransformParams {
            fit_mode: self.fit_mode,
            rotate_deg: self.rotate_deg,
            flip_v: self.flip_v,
            flip_h: self.flip_h,
            zoom: self.zoom,
            offset_x: self.offset_x,
            offset_y: self.offset_y,
        }
    }

    /// Generates polar frames for this item based on current inspector settings.
    pub fn generate_polar_frames(&self, config: &PolarConfig, fps: f32) -> Result<Vec<Vec<u8>>> {
        let params = self.transform_params();
        match &self.media_source {
            MediaSource::RasterFrames(raw_frames) => {
                if raw_frames.is_empty() {
                    return Ok(Vec::new());
                }
                let target_frames = ((self.duration_seconds * fps).round() as usize).max(1);
                let unique_polar: Vec<Vec<u8>> = raw_frames
                    .iter()
                    .map(|frame| crate::polar::encode_image_to_polar_full(frame, config, &params))
                    .collect();
                if unique_polar.len() == 1 {
                    Ok(vec![unique_polar[0].clone(); target_frames])
                } else {
                    Ok((0..target_frames)
                        .map(|i| unique_polar[i % unique_polar.len()].clone())
                        .collect())
                }
            }
            MediaSource::Gif(source) => media::encode_gif_timeline(
                source,
                config,
                &params,
                Some(self.duration_seconds),
                fps,
                || false,
            )
            .map(|rendered| rendered.frames),
        }
    }
}

/// Fan playlist project managing sequence of items, frames, and configuration.
#[derive(Debug, Clone)]
pub struct FanProject {
    /// Playlist items in presentation order.
    pub items: Vec<ProjectItem>,
    /// Global array of polar frames for the entire project.
    pub frames: Vec<Vec<u8>>,
    /// Immutable factory-compatible display profile (112 LEDs × 450 lines).
    pub config: PolarConfig,
    /// Target file name for export (defaults to hologram.bin).
    pub output_filename: String,
    /// Counter for unique item IDs.
    next_id: usize,
    /// Revision key for data derived from the global frame sequence.
    frame_revision: u64,
}

/// Media decoded and encoded away from the UI thread, ready to append atomically.
pub struct PreparedMedia {
    item: ProjectItem,
    polar_frames: Vec<Vec<u8>>,
}

/// Fully rendered transform result. It is committed only if its source item is still current.
pub struct PreparedItemTransform {
    source_id: usize,
    item: ProjectItem,
    polar_frames: Vec<Vec<u8>>,
}

impl Default for FanProject {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            frames: Vec::new(),
            config: PolarConfig::standard(),
            output_filename: "hologram.bin".to_string(),
            next_id: 1,
            frame_revision: 0,
        }
    }
}

impl FanProject {
    /// Creates a new empty project.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears all items and frames from the project.
    pub fn clear(&mut self) {
        self.items.clear();
        self.frames.clear();
        self.next_id = 1;
        self.frame_revision = self.frame_revision.wrapping_add(1);
    }

    pub fn frame_revision(&self) -> u64 {
        self.frame_revision
    }

    /// Total number of polar frames in the project.
    pub fn total_frames(&self) -> usize {
        self.frames.len()
    }

    /// Total project duration in seconds.
    pub fn total_duration_seconds(&self) -> f32 {
        self.frames.len() as f32 / DEFAULT_FPS
    }

    /// Number of playlist items.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    pub fn prepare_item_transform(
        &self,
        item_index: usize,
        duration_seconds: f32,
        params: &TransformParams,
        cancelled: impl Fn() -> bool,
    ) -> Result<PreparedItemTransform> {
        let item = self
            .items
            .get(item_index)
            .with_context(|| format!("Playlist item index {item_index} is out of range"))?
            .clone();
        Self::prepare_item_transform_snapshot(
            item,
            self.config.clone(),
            duration_seconds,
            params,
            cancelled,
        )
    }

    pub fn prepare_item_transform_snapshot(
        mut item: ProjectItem,
        config: PolarConfig,
        duration_seconds: f32,
        params: &TransformParams,
        cancelled: impl Fn() -> bool,
    ) -> Result<PreparedItemTransform> {
        let source_id = item.id;
        item.duration_seconds = duration_seconds.max(0.2);
        item.rotate_deg = params.rotate_deg.rem_euclid(360);
        item.flip_v = params.flip_v;
        item.flip_h = params.flip_h;
        item.fit_mode = params.fit_mode;
        item.preserve_aspect = params.fit_mode != FitMode::Stretch;
        item.zoom = params.zoom.clamp(0.2, 4.0);
        item.offset_x = params.offset_x.clamp(-150, 150);
        item.offset_y = params.offset_y.clamp(-150, 150);
        let frames = match &item.media_source {
            MediaSource::Gif(source) => {
                media::encode_gif_timeline(
                    source,
                    &config,
                    params,
                    Some(item.duration_seconds),
                    DEFAULT_FPS,
                    cancelled,
                )?
                .frames
            }
            MediaSource::RasterFrames(raw_frames) => {
                anyhow::ensure!(!cancelled(), "Transform cancelled");
                let target = ((item.duration_seconds * DEFAULT_FPS).round() as usize).max(1);
                anyhow::ensure!(
                    target <= MAX_SUPPORTED_FRAMES,
                    "Transform requires {target} frames; the format supports at most {MAX_SUPPORTED_FRAMES}"
                );
                anyhow::ensure!(!raw_frames.is_empty(), "Raster source contains no frames");
                let encoded = raw_frames
                    .iter()
                    .map(|frame| {
                        anyhow::ensure!(!cancelled(), "Transform cancelled");
                        Ok(crate::polar::encode_image_to_polar_full(
                            frame, &config, params,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if encoded.len() == 1 {
                    vec![encoded[0].clone(); target]
                } else {
                    (0..target)
                        .map(|i| encoded[i % encoded.len()].clone())
                        .collect()
                }
            }
        };
        item.frame_count = frames.len();
        Ok(PreparedItemTransform {
            source_id,
            item,
            polar_frames: frames,
        })
    }

    pub fn commit_item_transform(&mut self, mut prepared: PreparedItemTransform) -> Result<()> {
        let current_index = self
            .items
            .iter()
            .position(|item| item.id == prepared.source_id)
            .context("Playlist item was removed while its transform was rendering")?;
        let old = &self.items[current_index];
        prepared.item.speed = old.speed;
        prepared.item.mode = old.mode;
        prepared.item.title = old.title.clone();
        prepared.item.source_path = old.source_path.clone();
        let start = old.frame_start;
        let count = old.frame_count;
        anyhow::ensure!(
            start
                .checked_add(count)
                .is_some_and(|end| end <= self.frames.len()),
            "Playlist item frame range is inconsistent with the project"
        );
        let total = self
            .frames
            .len()
            .checked_sub(count)
            .and_then(|n| n.checked_add(prepared.polar_frames.len()))
            .context("Project frame count overflow")?;
        anyhow::ensure!(
            total <= MAX_SUPPORTED_FRAMES,
            "Project would have {total} frames; the format supports at most {MAX_SUPPORTED_FRAMES}"
        );
        prepared.item.frame_start = start;
        prepared.item.frame_count = prepared.polar_frames.len();
        self.frames
            .splice(start..start + count, prepared.polar_frames);
        self.items[current_index] = prepared.item;
        let mut offset = 0;
        for item in &mut self.items {
            item.frame_start = offset;
            offset += item.frame_count;
        }
        self.frame_revision = self.frame_revision.wrapping_add(1);
        Ok(())
    }

    /// Populates project from an existing container.
    pub fn load_from_container(&mut self, container: HologramContainer) -> Result<()> {
        let decoded: Vec<DynamicImage> = container
            .frames
            .iter()
            .map(|frame| decode_polar_to_image(frame, &self.config).map(DynamicImage::ImageRgb8))
            .collect::<Result<_>>()?;
        let total = container.frames.len();
        let mut items = Vec::with_capacity(container.playlist.len().max(1));

        if container.playlist.is_empty() {
            if total > 0 {
                items.push(ProjectItem {
                    id: 1,
                    title: "Track 1".to_string(),
                    source_path: None,
                    frame_start: 0,
                    frame_count: total,
                    speed: DEFAULT_ITEM_SPEED,
                    mode: 1,
                    duration_seconds: (total as f32 / DEFAULT_FPS).max(1.0),
                    rotate_deg: 0,
                    flip_v: false,
                    flip_h: false,
                    fit_mode: FitMode::Fill,
                    preserve_aspect: true,
                    zoom: 1.0,
                    offset_x: 0,
                    offset_y: 0,
                    media_source: MediaSource::RasterFrames(decoded.into()),
                });
            }
        } else {
            for (idx, entry) in container.playlist.iter().enumerate() {
                let start = entry.start_frame.checked_sub(1).ok_or_else(|| {
                    anyhow::anyhow!("Playlist entry {} has a zero start frame", idx + 1)
                })? as usize;
                let end = entry.end_frame as usize;
                if start >= end || end > total {
                    bail!(
                        "Playlist entry {} references frames {}..{} but the container has {} frames",
                        idx + 1,
                        entry.start_frame,
                        entry.end_frame,
                        total
                    );
                }
                let count = end - start;
                items.push(ProjectItem {
                    id: idx + 1,
                    title: format!("Item #{}", idx + 1),
                    source_path: None,
                    frame_start: start,
                    frame_count: count,
                    speed: entry.speed.max(1),
                    mode: entry.mode.max(1),
                    duration_seconds: (count as f32 / DEFAULT_FPS).max(0.5),
                    rotate_deg: 0,
                    flip_v: false,
                    flip_h: false,
                    fit_mode: FitMode::Fill,
                    preserve_aspect: true,
                    zoom: 1.0,
                    offset_x: 0,
                    offset_y: 0,
                    media_source: MediaSource::RasterFrames(decoded[start..end].to_vec().into()),
                });
            }
        }

        self.clear();
        self.frames = container.frames;
        self.items = items;
        self.next_id = self.items.len() + 1;
        Ok(())
    }

    /// Alias for loading project from a container.
    pub fn load_from_hologram(&mut self, container: HologramContainer) -> Result<()> {
        self.load_from_container(container)
    }

    /// Internal helper to attach an item from decoded raw raster frames.
    fn add_item_with_raw_frames(
        &mut self,
        path: &Path,
        raw_frames: Vec<DynamicImage>,
        duration_seconds: f32,
        rotate_deg: i32,
        flip_v: bool,
        preserve_aspect: bool,
    ) -> Result<usize> {
        let expected_frames = ((duration_seconds * DEFAULT_FPS).round() as usize).max(1);
        self.ensure_capacity(expected_frames)?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Media".to_string());

        let mut item = ProjectItem {
            id: self.next_id,
            title: filename,
            source_path: Some(path.to_path_buf()),
            frame_start: self.frames.len(),
            frame_count: 0,
            speed: DEFAULT_ITEM_SPEED,
            mode: 1,
            duration_seconds,
            rotate_deg,
            flip_v,
            flip_h: false,
            fit_mode: if preserve_aspect {
                FitMode::Fill
            } else {
                FitMode::Stretch
            },
            preserve_aspect,
            zoom: 1.0,
            offset_x: 0,
            offset_y: 0,
            media_source: MediaSource::RasterFrames(raw_frames.into()),
        };
        self.next_id += 1;

        let new_polar = item.generate_polar_frames(&self.config, DEFAULT_FPS)?;
        item.frame_count = new_polar.len();
        self.append_prepared_media(PreparedMedia {
            item,
            polar_frames: new_polar,
        })
    }

    fn ensure_capacity(&self, incoming_frames: usize) -> Result<()> {
        if self.items.len() >= MAX_PLAYLIST_ENTRIES {
            bail!("Playlist already has the maximum of {MAX_PLAYLIST_ENTRIES} items");
        }
        let total_frames = self
            .frames
            .len()
            .checked_add(incoming_frames)
            .ok_or_else(|| anyhow::anyhow!("Project frame count overflow"))?;
        if total_frames > MAX_SUPPORTED_FRAMES {
            bail!(
                "Project would have {total_frames} frames; the format supports at most {MAX_SUPPORTED_FRAMES}"
            );
        }
        Ok(())
    }

    /// Decodes and encodes one media item without changing the project.
    pub fn prepare_media_file_with_duration(
        &self,
        path: &Path,
        duration_seconds: f32,
    ) -> Result<PreparedMedia> {
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
        {
            return self.prepare_gif_source(
                path,
                (duration_seconds > 0.0).then_some(duration_seconds),
                TransformParams::default(),
                || false,
            );
        }

        let raw_frames = media::load_media_raw_frames(path)?;
        if raw_frames.is_empty() {
            bail!("File contains no valid frames: {}", path.display());
        }

        let duration = if duration_seconds > 0.0 {
            duration_seconds
        } else if raw_frames.len() > 1 {
            raw_frames.len() as f32 / DEFAULT_FPS
        } else {
            5.0
        };
        let filename = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Media".to_owned());
        let mut item = ProjectItem {
            id: 0,
            title: filename,
            source_path: Some(path.to_path_buf()),
            frame_start: 0,
            frame_count: 0,
            speed: DEFAULT_ITEM_SPEED,
            mode: 1,
            duration_seconds: duration,
            rotate_deg: 0,
            flip_v: false,
            flip_h: false,
            fit_mode: FitMode::Fill,
            preserve_aspect: true,
            zoom: 1.0,
            offset_x: 0,
            offset_y: 0,
            media_source: MediaSource::RasterFrames(raw_frames.into()),
        };
        let expected_frames = ((duration * DEFAULT_FPS).round() as usize).max(1);
        self.ensure_capacity(expected_frames)?;
        let polar_frames = item.generate_polar_frames(&self.config, DEFAULT_FPS)?;
        item.frame_count = polar_frames.len();
        Ok(PreparedMedia { item, polar_frames })
    }

    /// Prepares a playlist item using the source GIF duration, or five seconds for a still image.
    pub fn prepare_media_file_for_playlist(&self, path: &Path) -> Result<PreparedMedia> {
        self.prepare_media_file_for_playlist_cancellable(path, || false)
    }

    pub fn prepare_media_file_for_playlist_cancellable(
        &self,
        path: &Path,
        cancelled: impl Fn() -> bool,
    ) -> Result<PreparedMedia> {
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
        {
            self.prepare_gif_source(path, None, TransformParams::default(), cancelled)
        } else {
            self.prepare_media_file_with_duration(path, 5.0)
        }
    }

    fn prepare_gif_source(
        &self,
        path: &Path,
        duration_seconds: Option<f32>,
        params: TransformParams,
        cancelled: impl Fn() -> bool,
    ) -> Result<PreparedMedia> {
        let source: Arc<[u8]> = std::fs::read(path)
            .with_context(|| format!("Failed to read GIF source {}", path.display()))?
            .into();
        self.prepare_gif_bytes(path, source, duration_seconds, params, cancelled)
    }

    pub fn prepare_media_plan_cancellable(
        &self,
        plan: media::PlaylistMediaPlan,
        cancelled: impl Fn() -> bool,
    ) -> Result<PreparedMedia> {
        if let Some(source) = plan.gif_source {
            return self.prepare_gif_bytes(
                &plan.path,
                source,
                Some(plan.duration_seconds),
                TransformParams::default(),
                cancelled,
            );
        }
        self.prepare_media_file_with_duration(&plan.path, plan.duration_seconds)
    }

    fn prepare_gif_bytes(
        &self,
        path: &Path,
        source: Arc<[u8]>,
        duration_seconds: Option<f32>,
        params: TransformParams,
        cancelled: impl Fn() -> bool,
    ) -> Result<PreparedMedia> {
        let timeline = media::encode_gif_timeline(
            &source,
            &self.config,
            &params,
            duration_seconds,
            DEFAULT_FPS,
            cancelled,
        )?;
        self.ensure_capacity(timeline.frames.len())?;
        let filename = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Media".to_owned());
        let item = ProjectItem {
            id: 0,
            title: filename,
            source_path: Some(path.to_path_buf()),
            frame_start: 0,
            frame_count: timeline.frames.len(),
            speed: DEFAULT_ITEM_SPEED,
            mode: 1,
            duration_seconds: timeline.duration_seconds,
            rotate_deg: params.rotate_deg,
            flip_v: params.flip_v,
            flip_h: params.flip_h,
            fit_mode: params.fit_mode,
            preserve_aspect: params.fit_mode != FitMode::Stretch,
            zoom: params.zoom,
            offset_x: params.offset_x,
            offset_y: params.offset_y,
            media_source: MediaSource::Gif(source),
        };
        Ok(PreparedMedia {
            item,
            polar_frames: timeline.frames,
        })
    }

    /// Appends a previously prepared item without doing image work on the caller thread.
    pub fn append_prepared_media(&mut self, mut prepared: PreparedMedia) -> Result<usize> {
        if self.items.len() >= MAX_PLAYLIST_ENTRIES {
            bail!("Playlist already has the maximum of {MAX_PLAYLIST_ENTRIES} items");
        }
        self.ensure_capacity(prepared.polar_frames.len())?;
        if prepared.polar_frames.is_empty() {
            bail!("Prepared media contains no output frames");
        }
        if prepared
            .polar_frames
            .iter()
            .any(|frame| frame.len() != crate::format::HOLOGRAM_FRAME_SIZE)
        {
            bail!("Prepared media contains a frame with an invalid size");
        }

        prepared.item.id = self.next_id;
        prepared.item.frame_start = self.frames.len();
        prepared.item.frame_count = prepared.polar_frames.len();
        self.next_id += 1;
        let count = prepared.item.frame_count;
        self.frames.extend(prepared.polar_frames);
        self.items.push(prepared.item);
        self.frame_revision = self.frame_revision.wrapping_add(1);
        Ok(count)
    }

    /// Adds a media file with explicitly specified playback duration in seconds.
    pub fn add_media_file_with_duration(
        &mut self,
        path: &Path,
        duration_seconds: f32,
    ) -> Result<usize> {
        let prepared = self.prepare_media_file_with_duration(path, duration_seconds)?;
        self.append_prepared_media(prepared)
    }

    /// Adds media with the playlist default: full source duration for GIF, five seconds for stills.
    pub fn add_media_file_for_playlist(&mut self, path: &Path) -> Result<usize> {
        let prepared = self.prepare_media_file_for_playlist(path)?;
        self.append_prepared_media(prepared)
    }

    /// Adds a media file with default repetition count for static images.
    pub fn add_media_file(&mut self, path: &Path, static_repeat_count: usize) -> Result<usize> {
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
        {
            let prepared =
                self.prepare_gif_source(path, None, TransformParams::default(), || false)?;
            return self.append_prepared_media(prepared);
        }
        let raw_frames = media::load_media_raw_frames(path)?;
        if raw_frames.is_empty() {
            bail!("File contains no valid frames: {}", path.display());
        }

        let duration = if raw_frames.len() > 1 {
            raw_frames.len() as f32 / DEFAULT_FPS
        } else {
            (static_repeat_count.max(1) as f32) / DEFAULT_FPS
        };

        self.add_item_with_raw_frames(path, raw_frames, duration, 0, false, true)
    }

    /// Adds a media file with custom load options.
    pub fn add_media_file_with_options(
        &mut self,
        path: &Path,
        options: &MediaLoadOptions,
    ) -> Result<usize> {
        let raw_frames = media::load_media_raw_frames(path)?;
        if raw_frames.is_empty() {
            bail!("File contains no valid frames: {}", path.display());
        }

        let duration = if raw_frames.len() > 1 {
            raw_frames.len() as f32 / DEFAULT_FPS
        } else {
            (options.static_frame_count.max(1) as f32) / DEFAULT_FPS
        };

        self.add_item_with_raw_frames(
            path,
            raw_frames,
            duration,
            options.rotate_deg,
            options.flip_v,
            options.preserve_aspect,
        )
    }

    /// Updates item geometry, positioning, framing presets, and duration.
    pub fn update_item_transform_full(
        &mut self,
        item_idx: usize,
        duration_seconds: f32,
        params: &TransformParams,
    ) -> Result<()> {
        if item_idx >= self.items.len() {
            bail!("Playlist item index {item_idx} is out of range");
        }
        let mut updated = self.items[item_idx].clone();
        updated.duration_seconds = duration_seconds.max(0.2);
        updated.rotate_deg = params.rotate_deg.rem_euclid(360);
        updated.flip_v = params.flip_v;
        updated.flip_h = params.flip_h;
        updated.fit_mode = params.fit_mode;
        updated.preserve_aspect = params.fit_mode != FitMode::Stretch;
        updated.zoom = params.zoom.clamp(0.2, 4.0);
        updated.offset_x = params.offset_x.clamp(-150, 150);
        updated.offset_y = params.offset_y.clamp(-150, 150);

        let new_polar = updated.generate_polar_frames(&self.config, DEFAULT_FPS)?;
        let old_start = updated.frame_start;
        let old_count = updated.frame_count;
        anyhow::ensure!(
            old_start
                .checked_add(old_count)
                .is_some_and(|end| end <= self.frames.len()),
            "Playlist item frame range is inconsistent with the project"
        );
        let new_total = self
            .frames
            .len()
            .checked_sub(old_count)
            .and_then(|remaining| remaining.checked_add(new_polar.len()))
            .context("Project frame count overflow")?;
        anyhow::ensure!(
            new_total <= MAX_SUPPORTED_FRAMES,
            "Project would have {new_total} frames; the format supports at most {MAX_SUPPORTED_FRAMES}"
        );

        updated.frame_count = new_polar.len();
        self.frames
            .splice(old_start..old_start + old_count, new_polar);
        self.items[item_idx] = updated;
        let mut offset = 0;
        for item in &mut self.items {
            item.frame_start = offset;
            offset += item.frame_count;
        }
        self.frame_revision = self.frame_revision.wrapping_add(1);
        Ok(())
    }

    /// Sets the controller playback speed for one playlist item (1..=255).
    pub fn set_item_speed(&mut self, item_idx: usize, speed: u8) {
        if let Some(item) = self.items.get_mut(item_idx) {
            item.speed = speed.max(1);
        }
    }

    /// Updates transform and duration parameters for an item.
    pub fn update_item_settings(
        &mut self,
        item_idx: usize,
        duration_seconds: f32,
        rotate_deg: i32,
        flip_v: bool,
        preserve_aspect: bool,
        zoom: f32,
    ) -> Result<()> {
        let fit_mode = if preserve_aspect {
            FitMode::Fill
        } else {
            FitMode::Stretch
        };
        let (flip_h, off_x, off_y) = if item_idx < self.items.len() {
            (
                self.items[item_idx].flip_h,
                self.items[item_idx].offset_x,
                self.items[item_idx].offset_y,
            )
        } else {
            (false, 0, 0)
        };
        let params = TransformParams {
            fit_mode,
            rotate_deg,
            flip_v,
            flip_h,
            zoom,
            offset_x: off_x,
            offset_y: off_y,
        };
        self.update_item_transform_full(item_idx, duration_seconds, &params)
    }

    /// Removes an item by index and drains its corresponding frame range.
    pub fn remove_item(&mut self, item_index: usize) -> bool {
        if item_index >= self.items.len() {
            return false;
        }

        let removed = self.items.remove(item_index);
        let start = removed.frame_start;
        let count = removed.frame_count;

        if start + count <= self.frames.len() {
            self.frames.drain(start..start + count);
        }

        // Recalculate start offsets for subsequent items
        let mut cur_offset = 0;
        for item in &mut self.items {
            item.frame_start = cur_offset;
            cur_offset += item.frame_count;
        }

        self.frame_revision = self.frame_revision.wrapping_add(1);

        true
    }

    /// Moves an item up in the playlist sequence.
    pub fn move_item_up(&mut self, item_index: usize) -> Result<bool> {
        if item_index == 0 || item_index >= self.items.len() {
            return Ok(false);
        }
        self.items.swap(item_index, item_index - 1);
        self.reorder_frames_to_match_items()?;
        Ok(true)
    }

    /// Moves an item down in the playlist sequence.
    pub fn move_item_down(&mut self, item_index: usize) -> Result<bool> {
        if item_index + 1 >= self.items.len() {
            return Ok(false);
        }
        self.items.swap(item_index, item_index + 1);
        self.reorder_frames_to_match_items()?;
        Ok(true)
    }

    fn reorder_frames_to_match_items(&mut self) -> Result<()> {
        let mut old_frames: Vec<Option<Vec<u8>>> = std::mem::take(&mut self.frames)
            .into_iter()
            .map(Some)
            .collect();
        let mut new_frames = Vec::with_capacity(old_frames.len());
        for item in &mut self.items {
            let start = item.frame_start;
            let end = start
                .checked_add(item.frame_count)
                .context("Playlist item frame range overflow")?;
            anyhow::ensure!(
                end <= old_frames.len(),
                "Playlist item frame range is invalid"
            );
            item.frame_start = new_frames.len();
            for frame in &mut old_frames[start..end] {
                new_frames.push(frame.take().context("Playlist frame was already moved")?);
            }
        }
        self.frames = new_frames;
        self.frame_revision = self.frame_revision.wrapping_add(1);
        Ok(())
    }

    /// Rebuilds global frame sequence based on item order and settings.
    pub fn rebuild_frames_from_items(&mut self) -> Result<()> {
        let mut new_frames = Vec::new();
        for item in &mut self.items {
            let item_frames = item.generate_polar_frames(&self.config, DEFAULT_FPS)?;
            item.frame_start = new_frames.len();
            item.frame_count = item_frames.len();
            new_frames.extend(item_frames);
            anyhow::ensure!(
                new_frames.len() <= MAX_SUPPORTED_FRAMES,
                "Project exceeds the {MAX_SUPPORTED_FRAMES}-frame format limit"
            );
        }
        self.frames = new_frames;
        self.frame_revision = self.frame_revision.wrapping_add(1);
        Ok(())
    }

    /// Exports the entire project to a binary container.
    pub fn export_container(&self) -> Result<HologramContainer> {
        if self.frames.is_empty() {
            bail!("Cannot export project: frame sequence is empty");
        }

        let mut playlist = Vec::with_capacity(self.items.len());
        for item in &self.items {
            let start = (item.frame_start + 1) as u16;
            let end = (item.frame_start + item.frame_count) as u16;
            playlist.push(PlaylistEntry {
                start_frame: start,
                end_frame: end,
                speed: item.speed,
                mode: item.mode,
            });
        }

        if playlist.is_empty() {
            playlist.push(PlaylistEntry {
                start_frame: 1,
                end_frame: self.frames.len() as u16,
                speed: DEFAULT_ITEM_SPEED,
                mode: 1,
            });
        }

        Ok(HologramContainer::new(self.frames.clone(), playlist))
    }

    /// Alias for exporting project to container.
    pub fn export_to_hologram(&self) -> Result<HologramContainer> {
        self.export_container()
    }

    /// Detects connected removable storage drives (USB / SD card).
    pub fn find_removable_drives() -> Vec<PathBuf> {
        let mut drives = Vec::new();

        #[cfg(windows)]
        unsafe {
            extern "system" {
                fn GetLogicalDrives() -> u32;
                fn GetDriveTypeW(lpRootPathName: *const u16) -> u32;
            }
            let mask = GetLogicalDrives();
            for i in 0..26 {
                if (mask & (1 << i)) != 0 {
                    let letter = (b'A' + i) as char;
                    let root_str = format!("{}:\\", letter);
                    let wide: Vec<u16> =
                        root_str.encode_utf16().chain(std::iter::once(0)).collect();
                    // DRIVE_REMOVABLE = 2
                    if GetDriveTypeW(wide.as_ptr()) == 2 {
                        drives.push(PathBuf::from(root_str));
                    }
                }
            }
        }

        #[cfg(unix)]
        {
            // Inspect mount points via /proc/mounts
            if let Ok(content) = std::fs::read_to_string("/proc/mounts") {
                for line in content.lines() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        let (_dev, mount_point, fstype) = (parts[0], parts[1], parts[2]);
                        if mount_point.starts_with("/run/media/")
                            || mount_point.starts_with("/media/")
                            || ((fstype == "vfat"
                                || fstype == "exfat"
                                || fstype == "ntfs"
                                || fstype == "msdos")
                                && !mount_point.starts_with("/boot")
                                && !mount_point.starts_with("/sys")
                                && !mount_point.starts_with("/etc"))
                        {
                            drives.push(PathBuf::from(mount_point));
                        }
                    }
                }
            }

            // Scan standard media mount hierarchies
            for base in &["/run/media", "/media"] {
                if let Ok(entries) = std::fs::read_dir(base) {
                    for entry in entries.flatten() {
                        let user_dir = entry.path();
                        if user_dir.is_dir() {
                            if let Ok(mounts) = std::fs::read_dir(&user_dir) {
                                for m in mounts.flatten() {
                                    if m.path().is_dir() {
                                        drives.push(m.path());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        drives.sort();
        drives.dedup();
        drives
    }
}
