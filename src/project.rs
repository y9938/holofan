//! Project playlist model and media management for holographic fan displays.
//!
//! Organizes sequences of items (images, GIFs, WebP animations),
//! handles item duration in seconds, rotation, scaling, mirroring,
//! item reordering, and direct export to removable storage media.

use anyhow::{bail, Result};
use image::DynamicImage;
use std::path::{Path, PathBuf};

use crate::format::{HologramContainer, PlaylistEntry};
use crate::media::{self, MediaLoadOptions};
pub use crate::polar::TransformParams;
use crate::polar::{decode_polar_to_image, FitMode, PolarConfig};

/// Base frame rate used for duration calculation (15 FPS).
pub const DEFAULT_FPS: f32 = 15.0;
/// Default controller playback speed used for newly added media.
pub const DEFAULT_ITEM_SPEED: u8 = 50;

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
    /// Cached source raster frames for instant re-encoding on adjustment.
    pub raw_frames: Vec<DynamicImage>,
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
    pub fn generate_polar_frames(&self, config: &PolarConfig, fps: f32) -> Vec<Vec<u8>> {
        if self.raw_frames.is_empty() {
            return Vec::new();
        }

        let target_frames = ((self.duration_seconds * fps).round() as usize).max(1);
        let n_raw = self.raw_frames.len();
        let params = self.transform_params();

        let unique_polar: Vec<Vec<u8>> = self
            .raw_frames
            .iter()
            .map(|frame| crate::polar::encode_image_to_polar_full(frame, config, &params))
            .collect();

        if unique_polar.len() == 1 {
            vec![unique_polar[0].clone(); target_frames]
        } else {
            (0..target_frames)
                .map(|i| unique_polar[i % n_raw].clone())
                .collect()
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
}

impl Default for FanProject {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            frames: Vec::new(),
            config: PolarConfig::standard(),
            output_filename: "hologram.bin".to_string(),
            next_id: 1,
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
                    raw_frames: decoded,
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
                    raw_frames: decoded[start..end].to_vec(),
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
            raw_frames,
        };
        self.next_id += 1;

        let new_polar = item.generate_polar_frames(&self.config, DEFAULT_FPS);
        item.frame_count = new_polar.len();
        self.frames.extend(new_polar);
        let count = item.frame_count;
        self.items.push(item);

        Ok(count)
    }

    /// Adds a media file with explicitly specified playback duration in seconds.
    pub fn add_media_file_with_duration(
        &mut self,
        path: &Path,
        duration_seconds: f32,
    ) -> Result<usize> {
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

        self.add_item_with_raw_frames(path, raw_frames, duration, 0, false, true)
    }

    /// Adds a media file with default repetition count for static images.
    pub fn add_media_file(&mut self, path: &Path, static_repeat_count: usize) -> Result<usize> {
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
    ) {
        if item_idx >= self.items.len() {
            return;
        }

        self.items[item_idx].duration_seconds = duration_seconds.max(0.2);
        self.items[item_idx].rotate_deg = params.rotate_deg.rem_euclid(360);
        self.items[item_idx].flip_v = params.flip_v;
        self.items[item_idx].flip_h = params.flip_h;
        self.items[item_idx].fit_mode = params.fit_mode;
        self.items[item_idx].preserve_aspect = params.fit_mode != FitMode::Stretch;
        self.items[item_idx].zoom = params.zoom.clamp(0.2, 4.0);
        self.items[item_idx].offset_x = params.offset_x.clamp(-150, 150);
        self.items[item_idx].offset_y = params.offset_y.clamp(-150, 150);

        let new_polar = self.items[item_idx].generate_polar_frames(&self.config, DEFAULT_FPS);
        let old_start = self.items[item_idx].frame_start;
        let old_count = self.items[item_idx].frame_count;

        if old_start + old_count <= self.frames.len() {
            self.frames
                .splice(old_start..old_start + old_count, new_polar);
        } else {
            self.rebuild_frames_from_items();
            return;
        }

        self.items[item_idx].frame_count =
            ((self.items[item_idx].duration_seconds * DEFAULT_FPS).round() as usize).max(1);

        // Recalculate frame offsets across the item sequence
        let mut cur = 0;
        for it in &mut self.items {
            it.frame_start = cur;
            cur += it.frame_count;
        }
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
    ) {
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
        self.update_item_transform_full(item_idx, duration_seconds, &params);
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

        true
    }

    /// Moves an item up in the playlist sequence.
    pub fn move_item_up(&mut self, item_index: usize) -> bool {
        if item_index == 0 || item_index >= self.items.len() {
            return false;
        }
        self.items.swap(item_index, item_index - 1);
        self.rebuild_frames_from_items();
        true
    }

    /// Moves an item down in the playlist sequence.
    pub fn move_item_down(&mut self, item_index: usize) -> bool {
        if item_index + 1 >= self.items.len() {
            return false;
        }
        self.items.swap(item_index, item_index + 1);
        self.rebuild_frames_from_items();
        true
    }

    /// Rebuilds global frame sequence based on item order and settings.
    pub fn rebuild_frames_from_items(&mut self) {
        let mut new_frames = Vec::new();
        for item in &mut self.items {
            let item_frames = item.generate_polar_frames(&self.config, DEFAULT_FPS);
            item.frame_start = new_frames.len();
            item.frame_count = item_frames.len();
            new_frames.extend(item_frames);
        }
        self.frames = new_frames;
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
