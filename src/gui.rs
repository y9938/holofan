//! Graphical user interface (egui / eframe).
//! Built for ease of use, stability, and responsive interaction:
//! - Arbitrary output .bin file name (defaults to hologram.bin).
//! - Media formats: PNG, JPG, BMP, animated GIF, and animated WebP.
//! - Direct item duration controls in seconds.
//! - Interactive media inspector under circular preview (fit mode, pan, zoom, rotation, mirroring).
//! - Playlist management (ordering, deletion, clearing).
//! - Direct export to detected SD card or USB flash drive with one click.
//! - Smooth POV fan circular preview and player controls.

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use formatjs_intl::format_message;

use crate::device::FanDevice;
use crate::format::HologramContainer;
use crate::media;
use crate::polar::{decode_polar_to_image, FitMode, TransformParams};
use crate::project::{FanProject, DEFAULT_FPS};

pub struct FanSoftwareApp {
    /// Playlist project and global polar frame sequence
    project: FanProject,
    /// Selected playlist item index
    selected_item_idx: Option<usize>,
    /// Current displayed frame index (0-based)
    current_frame_idx: usize,
    /// Cached preview texture
    preview_texture: Option<TextureHandle>,
    /// Frame index for which preview texture was generated
    texture_frame_idx: Option<usize>,
    /// Playback state flag
    is_playing: bool,
    /// Playback speed in frames per second
    playback_speed: f32,
    /// Timestamp of previous frame transition
    last_frame_time: Instant,
    /// Status bar notification text
    status_message: String,
    /// Hardware connection telemetry string
    device_status: String,
    /// Current project / container title
    project_title: String,
    /// Detected removable storage media paths
    detected_drives: Vec<PathBuf>,
    /// Interface language selection
    pub language: crate::i18n::Language,
}

impl Default for FanSoftwareApp {
    fn default() -> Self {
        let language = crate::i18n::Language::process_default();
        let intl = crate::i18n::for_language(language);
        let mut app = Self {
            project: FanProject::new(),
            selected_item_idx: None,
            current_frame_idx: 0,
            preview_texture: None,
            texture_frame_idx: None,
            is_playing: false,
            playback_speed: 15.0,
            last_frame_time: Instant::now(),
            status_message: format_message!(
                &intl,
                default_message: "Ready. Add an image, GIF, WebP, or open a container file (.bin).",
                description: "Initial status bar message on application startup"
            ),
            device_status: format_message!(
                &intl,
                default_message: "Device not connected",
                description: "Device status when fan is not connected via USB HID"
            ),
            project_title: format_message!(
                &intl,
                default_message: "New Project",
                description: "Default project title"
            ),
            detected_drives: FanProject::find_removable_drives(),
            language,
        };

        // Auto-load default container if present in current working directory
        let default_bin = PathBuf::from("hologram.bin");
        if default_bin.exists() {
            if let Err(error) = app.open_bin_file(&default_bin) {
                app.status_message = format!("Failed to open hologram.bin: {error:#}");
            }
        }

        app
    }
}

impl FanSoftwareApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    /// Creates application with pre-loaded command line file arguments
    pub fn with_initial_files(files: Vec<PathBuf>) -> Self {
        let mut app = Self::default();
        if !files.is_empty() {
            app.project.clear();
            for file in files {
                let ext = file
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if ext == "bin" {
                    if let Err(error) = app.open_bin_file(&file) {
                        app.status_message =
                            format!("Failed to open {}: {error:#}", file.display());
                    }
                } else {
                    if let Err(error) = app.add_media_file(&file) {
                        app.status_message = format!("Failed to add {}: {error:#}", file.display());
                    }
                }
            }
        }
        app
    }

    /// Opens an existing binary container (.bin)
    pub fn open_bin_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let container = HologramContainer::read_from_file(path)?;
        let count = container.frames.len();
        self.project.load_from_container(container)?;
        self.current_frame_idx = 0;
        self.texture_frame_idx = None;
        self.preview_texture = None;
        self.selected_item_idx = if self.project.item_count() > 0 {
            Some(0)
        } else {
            None
        };
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "hologram.bin".to_string());
        self.project.output_filename = filename.clone();
        self.project_title = filename;

        let intl = crate::i18n::for_language(self.language);
        self.status_message = format_message!(
            &intl,
            default_message: "File loaded: {title} (frames: {frames}, items: {items})",
            description: "Status message after loading a binary container file",
            values: {
                title: self.project_title.clone(),
                frames: count.to_string(),
                items: self.project.item_count().to_string(),
            }
        );
        Ok(())
    }

    /// Adds a media file (image, GIF, WebP) with default 5.0 second display duration
    pub fn add_media_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let count = self.project.add_media_file_with_duration(path, 5.0)?;
        let new_item_idx = self.project.item_count().saturating_sub(1);
        self.selected_item_idx = Some(new_item_idx);
        if let Some(item) = self.project.items.get(new_item_idx) {
            self.current_frame_idx = item.frame_start;
            self.texture_frame_idx = None;
            self.preview_texture = None;
        }

        if self.project.item_count() == 1 {
            if let Some(stem) = path.file_stem() {
                self.project.output_filename = format!("{}.bin", stem.to_string_lossy());
            }
        }

        let intl = crate::i18n::for_language(self.language);
        self.status_message = format_message!(
            &intl,
            default_message: "Added \"{filename}\": {duration} s (+{frames} frames). Total in project: {total_duration} s",
            description: "Status message after adding a media file to the playlist",
            values: {
                filename: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                duration: "5.0".to_string(),
                frames: count.to_string(),
                total_duration: format!("{:.1}", self.project.total_duration_seconds()),
            }
        );
        Ok(())
    }

    /// Updates preview texture for the active frame index
    fn update_preview_texture(&mut self, ctx: &egui::Context) {
        let total = self.project.total_frames();
        if total == 0 {
            self.preview_texture = None;
            self.texture_frame_idx = None;
            return;
        }

        if self.current_frame_idx >= total {
            self.current_frame_idx = 0;
        }

        if self.texture_frame_idx != Some(self.current_frame_idx) || self.preview_texture.is_none()
        {
            let frame_data = &self.project.frames[self.current_frame_idx];
            let reconstructed = match decode_polar_to_image(frame_data, &self.project.config) {
                Ok(image) => image,
                Err(error) => {
                    self.preview_texture = None;
                    self.texture_frame_idx = None;
                    self.status_message = format!("Preview decode failed: {error:#}");
                    return;
                }
            };
            let width = reconstructed.width() as usize;
            let height = reconstructed.height() as usize;

            let mut pixels = Vec::with_capacity(width * height);
            for p in reconstructed.pixels() {
                pixels.push(Color32::from_rgb(p[0], p[1], p[2]));
            }

            let color_image = ColorImage {
                size: [width, height],
                pixels,
            };

            self.preview_texture =
                Some(ctx.load_texture("fan_preview_tex", color_image, TextureOptions::LINEAR));
            self.texture_frame_idx = Some(self.current_frame_idx);
        }
    }

    /// Writes project to the selected removable storage mount point
    fn write_to_removable_drive(&mut self, drive_path: &Path) {
        let intl = crate::i18n::for_language(self.language);
        match self.project.export_container() {
            Ok(container) => {
                let target = drive_path.join(&self.project.output_filename);
                match container.write_to_file(&target) {
                    Ok(_) => {
                        self.status_message = format_message!(
                            &intl,
                            default_message: "✅ Successfully written to SD/USB: {target} (frames: {frames})",
                            description: "Status message when project is saved to removable media",
                            values: {
                                target: target.display().to_string(),
                                frames: container.total_frames.to_string(),
                            }
                        );
                    }
                    Err(e) => {
                        self.status_message = format_message!(
                            &intl,
                            default_message: "❌ Error writing to disk: {error}",
                            description: "Status message when writing to removable media fails",
                            values: {
                                error: e.to_string(),
                            }
                        );
                    }
                }
            }
            Err(e) => {
                self.status_message = format_message!(
                    &intl,
                    default_message: "❌ Project preparation error: {error}",
                    description: "Status message when container export preparation fails",
                    values: {
                        error: e.to_string(),
                    }
                );
            }
        }
    }
}

impl eframe::App for FanSoftwareApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let intl = crate::i18n::for_language(self.language);

        // Playback animation ticker
        if self.is_playing && self.project.total_frames() > 0 {
            let interval = 1.0 / self.playback_speed.max(1.0);
            if self.last_frame_time.elapsed().as_secs_f32() >= interval {
                self.current_frame_idx = (self.current_frame_idx + 1) % self.project.total_frames();
                self.last_frame_time = Instant::now();

                // Automatically sync selected playlist item with the active frame
                for (idx, item) in self.project.items.iter().enumerate() {
                    if self.current_frame_idx >= item.frame_start
                        && self.current_frame_idx < item.frame_start + item.frame_count
                    {
                        self.selected_item_idx = Some(idx);
                        break;
                    }
                }
            }
            ctx.request_repaint();
        }

        // Handle Drag & Drop files directly onto the window
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            if let Some(path) = file.path {
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if ext == "bin" {
                    if let Err(e) = self.open_bin_file(&path) {
                        self.status_message = format_message!(
                            &intl,
                            default_message: "Error opening {path}: {error}",
                            description: "Status message on failed drag-and-drop bin open",
                            values: {
                                path: path.display().to_string(),
                                error: e.to_string(),
                            }
                        );
                    }
                } else {
                    if let Err(e) = self.add_media_file(&path) {
                        self.status_message = format_message!(
                            &intl,
                            default_message: "Error adding {path}: {error}",
                            description: "Status message on failed drag-and-drop media add",
                            values: {
                                path: path.display().to_string(),
                                error: e.to_string(),
                            }
                        );
                    }
                }
            }
        }

        self.update_preview_texture(ctx);

        // Top action toolbar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.heading("🌀 Fan Software");
                ui.separator();

                // Add media file action
                let btn_add_media = format_message!(
                    &intl,
                    default_message: "➕ Add media (PNG/JPG/GIF/WebP)",
                    description: "Button to open file picker for images or animations"
                );
                if ui.button(btn_add_media).clicked() {
                    let filter_media = format_message!(
                        &intl,
                        default_message: "Images and animations",
                        description: "File filter name for media files"
                    );
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter(&filter_media, &["png", "jpg", "jpeg", "bmp", "gif", "webp"])
                        .pick_file()
                    {
                        if let Err(e) = self.add_media_file(&path) {
                            self.status_message = format_message!(
                                &intl,
                                default_message: "Error adding: {error}",
                                description: "Status message when adding media file from dialog fails",
                                values: { error: e.to_string() }
                            );
                        }
                    }
                }

                // Open existing container action
                let btn_open_bin = format_message!(
                    &intl,
                    default_message: "📂 Open .bin",
                    description: "Button to open existing .bin container file"
                );
                if ui.button(btn_open_bin).clicked() {
                    let filter_fan_bin = format_message!(
                        &intl,
                        default_message: "Hologram fan files (*.bin)",
                        description: "File filter name for .bin containers"
                    );
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter(&filter_fan_bin, &["bin"])
                        .pick_file()
                    {
                        if let Err(e) = self.open_bin_file(&path) {
                            self.status_message = format_message!(
                                &intl,
                                default_message: "Error reading .bin: {error}",
                                description: "Status message when opening .bin fails",
                                values: { error: e.to_string() }
                            );
                        }
                    }
                }

                // Save container action
                let btn_save_bin = format_message!(
                    &intl,
                    default_message: "💾 Save .bin...",
                    description: "Button to save container with custom filename"
                );
                if ui.button(btn_save_bin).clicked() {
                    match self.project.export_container() {
                        Ok(container) => {
                            let filter_save_bin = format_message!(
                                &intl,
                                default_message: "Hologram fan file (*.bin)",
                                description: "File filter name for saving .bin container"
                            );
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name(&self.project.output_filename)
                                .add_filter(&filter_save_bin, &["bin"])
                                .save_file()
                            {
                                match container.write_to_file(&path) {
                                    Ok(_) => {
                                        if let Some(name) = path.file_name() {
                                            self.project.output_filename = name.to_string_lossy().to_string();
                                        }
                                        self.status_message = format_message!(
                                            &intl,
                                            default_message: "✅ File saved successfully: {path}",
                                            description: "Status message when container is saved successfully",
                                            values: { path: path.display().to_string() }
                                        );
                                    }
                                    Err(e) => {
                                        self.status_message = format_message!(
                                            &intl,
                                            default_message: "❌ Save error: {error}",
                                            description: "Status message when saving container fails",
                                            values: { error: e.to_string() }
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            self.status_message = format_message!(
                                &intl,
                                default_message: "Error: {error}",
                                description: "Generic error status message",
                                values: { error: e.to_string() }
                            );
                        }
                    }
                }

                ui.separator();

                // Output filename input field
                let lbl_filename = format_message!(
                    &intl,
                    default_message: "File name:",
                    description: "Label for the output filename text input"
                );
                ui.label(lbl_filename);
                ui.add(egui::TextEdit::singleline(&mut self.project.output_filename).desired_width(110.0));

                // Removable drive export action
                self.detected_drives = FanProject::find_removable_drives();
                let has_drives = !self.detected_drives.is_empty();

                if has_drives {
                    let drive = self.detected_drives[0].clone();
                    let drive_name = drive.file_name().unwrap_or_default().to_string_lossy();
                    let btn_text = format_message!(
                        &intl,
                        default_message: "⚡ Write to \"{drive}\"",
                        description: "Button to write project to detected removable drive",
                        values: { drive: drive_name.to_string() }
                    );
                    if ui.button(btn_text).clicked() {
                        self.write_to_removable_drive(&drive);
                    }
                } else {
                    let btn_text = format_message!(
                        &intl,
                        default_message: "⚡ Write to SD card...",
                        description: "Button to select SD card root and save project"
                    );
                    if ui.button(btn_text).clicked() {
                        let dialog_title = format_message!(
                            &intl,
                            default_message: "Select SD card root directory",
                            description: "Title of directory picker dialog for SD card export"
                        );
                        if let Some(folder) = rfd::FileDialog::new().set_title(&dialog_title).pick_folder() {
                            self.write_to_removable_drive(&folder);
                        }
                    }
                }

                ui.separator();

                // PNG export action
                let btn_export_png = format_message!(
                    &intl,
                    default_message: "🖼 Export frames to PNG",
                    description: "Button to export polar frames as PNG images"
                );
                if ui.button(btn_export_png).clicked() {
                    if self.project.total_frames() == 0 {
                        self.status_message = format_message!(
                            &intl,
                            default_message: "No frames to export!",
                            description: "Status message when PNG export has no frames"
                        );
                    } else if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        match media::export_polar_frames_to_png(&self.project.frames, &folder, &self.project.config) {
                            Ok(n) => {
                                self.status_message = format_message!(
                                    &intl,
                                    default_message: "✅ Exported {count} frames to {path}",
                                    description: "Status message on successful PNG export",
                                    values: {
                                        count: n.to_string(),
                                        path: folder.display().to_string(),
                                    }
                                );
                            }
                            Err(e) => {
                                self.status_message = format_message!(
                                    &intl,
                                    default_message: "❌ Export error: {error}",
                                    description: "Status message when PNG export fails",
                                    values: { error: e.to_string() }
                                );
                            }
                        }
                    }
                }

                // USB HID
                let btn_hid = format_message!(
                    &intl,
                    default_message: "🔌 HID connect",
                    description: "Button to test USB HID connection with fan device"
                );
                if ui.button(btn_hid).clicked() {
                    match FanDevice::connect() {
                        Ok(dev) => match dev.query_status() {
                            Ok(_) => {
                                self.device_status = format_message!(
                                    &intl,
                                    default_message: "Connected (VID: 0x0483, PID: 0x5750)",
                                    description: "Device status label when USB HID device is connected"
                                );
                                self.status_message = format_message!(
                                    &intl,
                                    default_message: "Hologram fan detected via USB HID and ready for commands.",
                                    description: "Status message when fan device responds to status query"
                                );
                            }
                            Err(e) => {
                                self.device_status = format_message!(
                                    &intl,
                                    default_message: "Reply: {error}",
                                    description: "Device status when HID query returns an error",
                                    values: { error: e.to_string() }
                                );
                            }
                        },
                        Err(e) => {
                            self.device_status = format_message!(
                                &intl,
                                default_message: "Not connected",
                                description: "Device status when USB HID connection fails"
                            );
                            self.status_message = format_message!(
                                &intl,
                                default_message: "USB status: {error}",
                                description: "Status message reporting USB connection error",
                                values: { error: e.to_string() }
                            );
                        }
                    }
                }

                ui.separator();

                let previous_lang = self.language;
                egui::ComboBox::from_id_salt("language_switcher")
                    .selected_text(format!(
                        "{} {}",
                        self.language.flag().unwrap_or(""),
                        self.language.display_name()
                    ))
                    .show_ui(ui, |ui| {
                        for lang in crate::i18n::Language::available() {
                            let label = format!(
                                "{} {}",
                                lang.flag().unwrap_or(""),
                                lang.display_name()
                            );
                            ui.selectable_value(&mut self.language, lang, label);
                        }
                    });
                if self.language != previous_lang {
                    ctx.request_repaint();
                }
            });
            ui.add_space(4.0);
        });

        // Status bar at the bottom
        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let status_lbl = format_message!(
                    &intl,
                    default_message: "Status: {message}",
                    description: "Status bar prefix for general status messages",
                    values: { message: self.status_message.clone() }
                );
                ui.label(status_lbl);
                ui.separator();
                let drive_info = if self.detected_drives.is_empty() {
                    format_message!(
                        &intl,
                        default_message: "Removable drives: none detected",
                        description: "Status bar message when no removable drives are found"
                    )
                } else {
                    format_message!(
                        &intl,
                        default_message: "Removable drive: {path}",
                        description: "Status bar message showing first detected removable drive",
                        values: { path: format!("{:?}", self.detected_drives[0]) }
                    )
                };
                ui.label(drive_info);
                ui.separator();
                let hid_lbl = format_message!(
                    &intl,
                    default_message: "USB HID: {status}",
                    description: "Status bar prefix for USB HID hardware status",
                    values: { status: self.device_status.clone() }
                );
                ui.label(hid_lbl);
            });
            ui.add_space(2.0);
        });

        // Central area
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                // Left column: Playlist and fan display mode
                cols[0].vertical(|ui| {
                    ui.group(|ui| {
                        let playlist_heading = format_message!(
                            &intl,
                            default_message: "📋 Project playlist",
                            description: "Heading for playlist management section"
                        );
                        ui.heading(playlist_heading);

                        let summary_lbl = format_message!(
                            &intl,
                            default_message: "Total: {items} items | ⏱ {duration} s ({frames} frames)",
                            description: "Playlist summary displaying total items, duration, and frame count",
                            values: {
                                items: self.project.item_count().to_string(),
                                duration: format!("{:.1}", self.project.total_duration_seconds()),
                                frames: self.project.total_frames().to_string(),
                            }
                        );
                        ui.label(summary_lbl);

                        ui.add_space(4.0);

                        // Playlist items grid
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            egui::Grid::new("items_grid")
                                .striped(true)
                                .min_col_width(50.0)
                                .show(ui, |ui| {
                                    let col_num = format_message!(
                                        &intl,
                                        default_message: "#",
                                        description: "Column header: row number"
                                    );
                                    let col_filename = format_message!(
                                        &intl,
                                        default_message: "File name",
                                        description: "Column header: media file name"
                                    );
                                    let col_duration = format_message!(
                                        &intl,
                                        default_message: "Duration",
                                        description: "Column header: item duration in seconds"
                                    );
                                    let col_frames = format_message!(
                                        &intl,
                                        default_message: "Frames",
                                        description: "Column header: frame range and count"
                                    );
                                    let col_rotation = format_message!(
                                        &intl,
                                        default_message: "Rotation",
                                        description: "Column header: rotation angle in degrees"
                                    );
                                    ui.label(col_num);
                                    ui.label(col_filename);
                                    ui.label(col_duration);
                                    ui.label(col_frames);
                                    ui.label(col_rotation);
                                    ui.end_row();

                                    let mut select_item = None;
                                    for (idx, item) in self.project.items.iter().enumerate() {
                                        let is_selected = self.selected_item_idx == Some(idx);
                                        if ui.selectable_label(is_selected, format!("#{}", idx + 1)).clicked() {
                                            select_item = Some(idx);
                                        }
                                        ui.label(&item.title);
                                        let duration_cell = format_message!(
                                            &intl,
                                            default_message: "{seconds} s",
                                            description: "Seconds abbreviation in playlist table",
                                            values: { seconds: format!("{:.1}", item.duration_seconds) }
                                        );
                                        ui.label(duration_cell);
                                        ui.label(format!("{}-{} ({})", item.frame_start + 1, item.frame_start + item.frame_count, item.frame_count));
                                        ui.label(format!("{}°", item.rotate_deg));
                                        ui.end_row();
                                    }

                                    if let Some(idx) = select_item {
                                        self.selected_item_idx = Some(idx);
                                        if let Some(item) = self.project.items.get(idx) {
                                            self.current_frame_idx = item.frame_start;
                                            self.texture_frame_idx = None;
                                        }
                                    }
                                });
                        });

                        ui.add_space(6.0);

                        // Ordering and deletion buttons
                        ui.horizontal(|ui| {
                            let has_sel = self.selected_item_idx.is_some();

                            let btn_del = format_message!(
                                &intl,
                                default_message: "Delete",
                                description: "Button to delete selected playlist item"
                            );
                            if ui.add_enabled(has_sel, egui::Button::new(btn_del)).clicked() {
                                if let Some(idx) = self.selected_item_idx {
                                    self.project.remove_item(idx);
                                    if self.project.item_count() == 0 {
                                        self.selected_item_idx = None;
                                        self.current_frame_idx = 0;
                                    } else {
                                        self.selected_item_idx = Some(idx.min(self.project.item_count() - 1));
                                    }
                                    self.texture_frame_idx = None;
                                    self.status_message = format_message!(
                                        &intl,
                                        default_message: "Item removed from playlist",
                                        description: "Status message when an item is deleted"
                                    );
                                }
                            }

                            let btn_up = format_message!(
                                &intl,
                                default_message: "Up",
                                description: "Button to move selected playlist item up"
                            );
                            if ui.add_enabled(has_sel, egui::Button::new(btn_up)).clicked() {
                                if let Some(idx) = self.selected_item_idx {
                                    if self.project.move_item_up(idx) {
                                        self.selected_item_idx = Some(idx - 1);
                                        self.texture_frame_idx = None;
                                    }
                                }
                            }

                            let btn_down = format_message!(
                                &intl,
                                default_message: "Down",
                                description: "Button to move selected playlist item down"
                            );
                            if ui.add_enabled(has_sel, egui::Button::new(btn_down)).clicked() {
                                if let Some(idx) = self.selected_item_idx {
                                    if self.project.move_item_down(idx) {
                                        self.selected_item_idx = Some(idx + 1);
                                        self.texture_frame_idx = None;
                                    }
                                }
                            }

                            let btn_clear = format_message!(
                                &intl,
                                default_message: "Clear all",
                                description: "Button to clear all playlist items"
                            );
                            if ui.button(btn_clear).clicked() {
                                self.project.clear();
                                self.selected_item_idx = None;
                                self.current_frame_idx = 0;
                                self.texture_frame_idx = None;
                                self.status_message = format_message!(
                                    &intl,
                                    default_message: "Playlist cleared completely",
                                    description: "Status message when entire playlist is cleared"
                                );
                            }
                        });

                        ui.add_space(8.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            let profile_heading = format_message!(
                                &intl,
                                default_message: "⚙ Controller format",
                                description: "Heading for the verified controller frame format"
                            );
                            ui.heading(profile_heading);
                        });

                        let profile_description = format_message!(
                            &intl,
                                id: "9eUNvMvjnj",
                            default_message: "112 radial LEDs · 450 angular lines · 114,688 bytes per frame",
                                description: "Verified controller geometry and encoded frame size"
                        );
                        ui.label(
                            egui::RichText::new(profile_description)
                                .small()
                                .color(Color32::from_rgb(180, 210, 255))
                        );
                    });
                });

                // Right column: POV preview and media inspector
                cols[1].vertical(|ui| {
                    ui.group(|ui| {
                        let prev_heading = format_message!(
                            &intl,
                            default_message: "🌀 POV display preview",
                            description: "Heading for circular POV display preview area"
                        );
                        ui.heading(prev_heading);

                        let preview_size = egui::vec2(280.0, 280.0);
                        if let Some(texture) = &self.preview_texture {
                            ui.image((texture.id(), preview_size));
                        } else {
                            let (rect, _) = ui.allocate_exact_size(preview_size, egui::Sense::hover());
                            ui.painter().rect_filled(rect, 4.0, Color32::from_rgb(10, 10, 15));
                            let placeholder = format_message!(
                                &intl,
                                default_message: "No frames to display\nAdd media files or a .bin file",
                                description: "Placeholder text inside empty preview circle"
                            );
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                placeholder,
                                egui::FontId::proportional(15.0),
                                Color32::GRAY,
                            );
                        }

                        ui.add_space(6.0);

                        // Playback controls
                        ui.horizontal(|ui| {
                            let play_btn_text = if self.is_playing {
                                format_message!(
                                    &intl,
                                    default_message: "⏸ Pause",
                                    description: "Button to pause animation playback"
                                )
                            } else {
                                format_message!(
                                    &intl,
                                    default_message: "▶ Play",
                                    description: "Button to start animation playback"
                                )
                            };
                            if ui.button(play_btn_text).clicked() {
                                self.is_playing = !self.is_playing;
                            }

                            if ui.button("⏮").clicked() && self.project.total_frames() > 0 {
                                self.current_frame_idx = 0;
                            }

                            if ui.button("◀").clicked()
                                && self.project.total_frames() > 0
                                && self.current_frame_idx > 0
                            {
                                self.current_frame_idx -= 1;
                            }

                            if ui.button("▶").clicked()
                                && self.project.total_frames() > 0
                                && self.current_frame_idx + 1 < self.project.total_frames()
                            {
                                self.current_frame_idx += 1;
                            }

                            if ui.button("⏭").clicked() && self.project.total_frames() > 0 {
                                self.current_frame_idx = self.project.total_frames().saturating_sub(1);
                            }

                            let lbl_fps = format_message!(
                                &intl,
                                default_message: "FPS:",
                                description: "Label for playback speed slider"
                            );
                            ui.label(lbl_fps);
                            ui.add(egui::Slider::new(&mut self.playback_speed, 1.0..=60.0).text(""));
                        });

                        let total = self.project.total_frames();
                        if total > 0 {
                            let mut frame_num = self.current_frame_idx + 1;
                            ui.horizontal(|ui| {
                                let lbl_frame = format_message!(
                                    &intl,
                                    default_message: "Frame:",
                                    description: "Label for current frame number slider"
                                );
                                ui.label(lbl_frame);
                                if ui.add(egui::Slider::new(&mut frame_num, 1..=total).text("")).changed() {
                                    self.current_frame_idx = frame_num.saturating_sub(1);
                                }
                                let progress_text = format_message!(
                                    &intl,
                                    default_message: "of {total} ({time} s)",
                                    description: "Frame progress indicator showing total frames and timestamp",
                                    values: {
                                        total: total.to_string(),
                                        time: format!("{:.2}", (self.current_frame_idx as f32 / DEFAULT_FPS)),
                                    }
                                );
                                ui.label(progress_text);
                            });
                        }

                        ui.add_space(6.0);
                        ui.separator();

                        // Item inspector
                        if let Some(sel_idx) = self.selected_item_idx {
                            if sel_idx < self.project.items.len() {
                                let item = &self.project.items[sel_idx];
                                let heading_text = format_message!(
                                    &intl,
                                    default_message: "🛠 Item #{index} settings: \"{title}\"",
                                    description: "Heading for media item transform inspector",
                                    values: {
                                        index: (sel_idx + 1).to_string(),
                                        title: item.title.clone(),
                                    }
                                );
                                ui.heading(heading_text);

                                let mut cur_duration = item.duration_seconds;
                                let mut cur_speed = item.speed;
                                let original_speed = item.speed;
                                let mut cur_rot = item.rotate_deg;
                                let mut cur_flip_v = item.flip_v;
                                let mut cur_flip_h = item.flip_h;
                                let mut cur_fit = item.fit_mode;
                                let mut cur_zoom = item.zoom;
                                let mut cur_off_x = item.offset_x;
                                let mut cur_off_y = item.offset_y;
                                let mut changed = false;

                                // Playback duration
                                ui.horizontal(|ui| {
                                    let lbl_dur = format_message!(
                                        &intl,
                                        default_message: "⏱ Duration:",
                                        description: "Label for item display duration controls"
                                    );
                                    ui.label(lbl_dur);
                                    let btn_minus = format_message!(
                                        &intl,
                                        default_message: "- 0.5s",
                                        description: "Button to decrease item duration by 0.5 seconds"
                                    );
                                    if ui.button(btn_minus).clicked() {
                                        cur_duration = (cur_duration - 0.5).max(0.5);
                                        changed = true;
                                    }
                                    let mut dur_display = cur_duration;
                                    let sec_suffix = format_message!(
                                        &intl,
                                        default_message: " sec",
                                        description: "Suffix for duration drag value"
                                    );
                                    if ui.add(egui::DragValue::new(&mut dur_display).speed(0.1).range(0.2..=60.0).suffix(&sec_suffix)).changed() {
                                        cur_duration = dur_display;
                                        changed = true;
                                    }
                                    let btn_plus = format_message!(
                                        &intl,
                                        default_message: "+ 0.5s",
                                        description: "Button to increase item duration by 0.5 seconds"
                                    );
                                    if ui.button(btn_plus).clicked() {
                                        cur_duration += 0.5;
                                        changed = true;
                                    }

                                    // Presets
                                    let lbl_pres = format_message!(
                                        &intl,
                                        default_message: "Presets:",
                                        description: "Label for quick duration preset buttons"
                                    );
                                    ui.label(lbl_pres);
                                    let btn_1s = format_message!(
                                        &intl,
                                        default_message: "1s",
                                        description: "Preset button for 1 second duration"
                                    );
                                    if ui.button(btn_1s).clicked() { cur_duration = 1.0; changed = true; }
                                    let btn_2s = format_message!(
                                        &intl,
                                        default_message: "2s",
                                        description: "Preset button for 2 seconds duration"
                                    );
                                    if ui.button(btn_2s).clicked() { cur_duration = 2.0; changed = true; }
                                    let btn_5s = format_message!(
                                        &intl,
                                        default_message: "5s",
                                        description: "Preset button for 5 seconds duration"
                                    );
                                    if ui.button(btn_5s).clicked() { cur_duration = 5.0; changed = true; }
                                    let btn_10s = format_message!(
                                        &intl,
                                        default_message: "10s",
                                        description: "Preset button for 10 seconds duration"
                                    );
                                    if ui.button(btn_10s).clicked() { cur_duration = 10.0; changed = true; }
                                });

                                // Controller speed is stored in the playlist entry, not the preview FPS.
                                ui.horizontal(|ui| {
                                    let lbl_speed = format_message!(
                                        &intl,
                                        id: "item-controller-speed",
                                        default_message: "Controller speed:",
                                        description: "Label for per-item controller playback speed"
                                    );
                                    ui.label(lbl_speed);
                                    ui.add(egui::DragValue::new(&mut cur_speed).range(1..=u8::MAX));
                                    ui.label("1–255");
                                });

                                let target_frames = ((cur_duration * DEFAULT_FPS).round() as usize).max(1);
                                let raw_cnt = self.project.items[sel_idx].raw_frames.len();
                                let total_text = if raw_cnt > 1 {
                                    format_message!(
                                        &intl,
                                        default_message: "Total: {duration} s ({frames} frames, ~{cycles} cycles)",
                                        description: "Item duration summary with cycle calculation for animated media",
                                        values: {
                                            duration: format!("{:.1}", cur_duration),
                                            frames: target_frames.to_string(),
                                            cycles: format!("{:.1}", target_frames as f32 / raw_cnt as f32),
                                        }
                                    )
                                } else {
                                    format_message!(
                                        &intl,
                                        default_message: "Total: {duration} s ({frames} frames)",
                                        description: "Item duration summary for static media",
                                        values: {
                                            duration: format!("{:.1}", cur_duration),
                                            frames: target_frames.to_string(),
                                        }
                                    )
                                };
                                ui.label(egui::RichText::new(total_text).color(Color32::from_rgb(180, 200, 255)));

                                // Fit mode selection
                                ui.horizontal(|ui| {
                                    let lbl_size = format_message!(
                                        &intl,
                                        default_message: "Size:",
                                        description: "Label for media framing mode selection"
                                    );
                                    ui.label(lbl_size);

                                    let fit_fit = format_message!(
                                        &intl,
                                        default_message: "Fit to circle",
                                        description: "Framing mode option to preserve aspect ratio with borders"
                                    );
                                    let tip_fit = format_message!(
                                        &intl,
                                        default_message: "Fit entirely into circle preserving aspect ratio (black borders, transparent in air)",
                                        description: "Tooltip for Fit to circle option"
                                    );
                                    if ui.selectable_label(cur_fit == FitMode::Fit, fit_fit)
                                        .on_hover_text(tip_fit)
                                        .clicked()
                                    {
                                        cur_fit = FitMode::Fit;
                                        changed = true;
                                    }

                                    let fit_fill = format_message!(
                                        &intl,
                                        default_message: "Fill circle",
                                        description: "Framing mode option to cover entire circle cropping edges"
                                    );
                                    let tip_fill = format_message!(
                                        &intl,
                                        default_message: "Fill entire circle without borders preserving aspect ratio (excess edges cropped)",
                                        description: "Tooltip for Fill circle option"
                                    );
                                    if ui.selectable_label(cur_fit == FitMode::Fill, fit_fill)
                                        .on_hover_text(tip_fill)
                                        .clicked()
                                    {
                                        cur_fit = FitMode::Fill;
                                        changed = true;
                                    }

                                    let fit_stretch = format_message!(
                                        &intl,
                                        id: "7BQbiC2tDl",
                                        default_message: "Stretch",
                                        description: "Framing mode option to stretch to 450x450 ignoring aspect ratio"
                                    );
                                    let tip_stretch = format_message!(
                                        &intl,
                                        id: "xgfO+GMO+c",
                                        default_message: "Stretch image over full 450x450 circle without preserving aspect ratio",
                                        description: "Tooltip for Stretch option"
                                    );
                                    if ui.selectable_label(cur_fit == FitMode::Stretch, fit_stretch)
                                        .on_hover_text(tip_stretch)
                                        .clicked()
                                    {
                                        cur_fit = FitMode::Stretch;
                                        changed = true;
                                    }
                                });

                                // Scaling / Zoom
                                ui.horizontal(|ui| {
                                    let lbl_scale = format_message!(
                                        &intl,
                                        default_message: "Scale:",
                                        description: "Label for zoom scale controls"
                                    );
                                    ui.label(lbl_scale);
                                    let btn_zoom_m = format_message!(
                                        &intl,
                                        default_message: "- 10%",
                                        description: "Button to decrease zoom scale by 10%"
                                    );
                                    if ui.button(btn_zoom_m).clicked() {
                                        cur_zoom = (cur_zoom - 0.1).max(0.2);
                                        changed = true;
                                    }
                                    let mut zoom_pct = (cur_zoom * 100.0).round() as i32;
                                    if ui.add(egui::Slider::new(&mut zoom_pct, 20..=300).suffix("%")).changed() {
                                        cur_zoom = zoom_pct as f32 / 100.0;
                                        changed = true;
                                    }
                                    let btn_zoom_p = format_message!(
                                        &intl,
                                        default_message: "+ 10%",
                                        description: "Button to increase zoom scale by 10%"
                                    );
                                    if ui.button(btn_zoom_p).clicked() {
                                        cur_zoom = (cur_zoom + 0.1).min(3.0);
                                        changed = true;
                                    }
                                    let btn_zoom_100 = format_message!(
                                        &intl,
                                        default_message: "100%",
                                        description: "Button to reset zoom scale to 100%"
                                    );
                                    if ui.button(btn_zoom_100).clicked() {
                                        cur_zoom = 1.0;
                                        changed = true;
                                    }
                                });

                                // Position offset
                                ui.horizontal(|ui| {
                                    let lbl_off = format_message!(
                                        &intl,
                                        default_message: "Offset:",
                                        description: "Label for position offset controls"
                                    );
                                    ui.label(lbl_off);
                                    let btn_left = format_message!(
                                        &intl,
                                        default_message: "< Left",
                                        description: "Button to shift image position left"
                                    );
                                    if ui.button(btn_left).clicked() {
                                        cur_off_x -= 10;
                                        changed = true;
                                    }
                                    let btn_right = format_message!(
                                        &intl,
                                        default_message: "> Right",
                                        description: "Button to shift image position right"
                                    );
                                    if ui.button(btn_right).clicked() {
                                        cur_off_x += 10;
                                        changed = true;
                                    }
                                    let btn_up = format_message!(
                                        &intl,
                                        default_message: "^ Up",
                                        description: "Button to shift image position up"
                                    );
                                    if ui.button(btn_up).clicked() {
                                        cur_off_y -= 10;
                                        changed = true;
                                    }
                                    let btn_down = format_message!(
                                        &intl,
                                        default_message: "v Down",
                                        description: "Button to shift image position down"
                                    );
                                    if ui.button(btn_down).clicked() {
                                        cur_off_y += 10;
                                        changed = true;
                                    }
                                    let btn_reset = format_message!(
                                        &intl,
                                        default_message: "Reset (0,0)",
                                        description: "Button to reset position offset to origin"
                                    );
                                    if (cur_off_x != 0 || cur_off_y != 0) && ui.button(btn_reset).clicked() {
                                        cur_off_x = 0;
                                        cur_off_y = 0;
                                        changed = true;
                                    }
                                    if cur_off_x != 0 || cur_off_y != 0 {
                                        ui.label(format!("(X: {}, Y: {})", cur_off_x, cur_off_y));
                                    }
                                });

                                // Rotation and mirror toggles
                                ui.horizontal(|ui| {
                                    let lbl_rot = format_message!(
                                        &intl,
                                        default_message: "Rotation:",
                                        description: "Label for rotation and mirror controls"
                                    );
                                    ui.label(lbl_rot);

                                    let btn_rot_m90 = format_message!(
                                        &intl,
                                        default_message: "-90°",
                                        description: "Button to rotate 90 degrees counter-clockwise"
                                    );
                                    let tip_rot_m90 = format_message!(
                                        &intl,
                                        default_message: "Rotate 90 degrees counter-clockwise",
                                        description: "Tooltip for counter-clockwise rotation button"
                                    );
                                    if ui.button(btn_rot_m90).on_hover_text(tip_rot_m90).clicked() {
                                        cur_rot = (cur_rot + 270) % 360;
                                        changed = true;
                                    }

                                    let btn_rot_p90 = format_message!(
                                        &intl,
                                        default_message: "+90°",
                                        description: "Button to rotate 90 degrees clockwise"
                                    );
                                    let tip_rot_p90 = format_message!(
                                        &intl,
                                        default_message: "Rotate 90 degrees clockwise",
                                        description: "Tooltip for clockwise rotation button"
                                    );
                                    if ui.button(btn_rot_p90).on_hover_text(tip_rot_p90).clicked() {
                                        cur_rot = (cur_rot + 90) % 360;
                                        changed = true;
                                    }

                                    let btn_rot_180 = format_message!(
                                        &intl,
                                        default_message: "180°",
                                        description: "Button to rotate 180 degrees"
                                    );
                                    if ui.button(btn_rot_180).clicked() {
                                        cur_rot = (cur_rot + 180) % 360;
                                        changed = true;
                                    }

                                    let flip_v_txt = if cur_flip_v {
                                        format_message!(
                                            &intl,
                                            default_message: "Mirror V: [ON]",
                                            description: "Button text when vertical flip is enabled"
                                        )
                                    } else {
                                        format_message!(
                                            &intl,
                                            default_message: "Mirror V",
                                            description: "Button text when vertical flip is disabled"
                                        )
                                    };
                                    let tip_flip_v = format_message!(
                                        &intl,
                                        default_message: "Vertical flip",
                                        description: "Tooltip for vertical flip button"
                                    );
                                    if ui.button(flip_v_txt).on_hover_text(tip_flip_v).clicked() {
                                        cur_flip_v = !cur_flip_v;
                                        changed = true;
                                    }

                                    let flip_h_txt = if cur_flip_h {
                                        format_message!(
                                            &intl,
                                            default_message: "Mirror H: [ON]",
                                            description: "Button text when horizontal flip is enabled"
                                        )
                                    } else {
                                        format_message!(
                                            &intl,
                                            default_message: "Mirror H",
                                            description: "Button text when horizontal flip is disabled"
                                        )
                                    };
                                    let tip_flip_h = format_message!(
                                        &intl,
                                        default_message: "Horizontal flip",
                                        description: "Tooltip for horizontal flip button"
                                    );
                                    if ui.button(flip_h_txt).on_hover_text(tip_flip_h).clicked() {
                                        cur_flip_h = !cur_flip_h;
                                        changed = true;
                                    }

                                    let angle_lbl = format_message!(
                                        &intl,
                                        default_message: "(angle: {angle}°)",
                                        description: "Current rotation angle indicator",
                                        values: { angle: cur_rot.to_string() }
                                    );
                                    ui.label(angle_lbl);
                                });

                                if changed {
                                    self.project.update_item_transform_full(
                                        sel_idx,
                                        cur_duration,
                                        &TransformParams {
                                            fit_mode: cur_fit,
                                            rotate_deg: cur_rot,
                                            flip_v: cur_flip_v,
                                            flip_h: cur_flip_h,
                                            zoom: cur_zoom,
                                            offset_x: cur_off_x,
                                            offset_y: cur_off_y,
                                        },
                                    );
                                    self.texture_frame_idx = None;
                                    self.status_message = format_message!(
                                        &intl,
                                        default_message: "Updated #{index}: {duration} s, rotation {rotation}°, zoom {zoom}%, offset ({x}, {y})",
                                        description: "Status message when item transform parameters are modified",
                                        values: {
                                            index: (sel_idx + 1).to_string(),
                                            duration: format!("{:.1}", cur_duration),
                                            rotation: cur_rot.to_string(),
                                            zoom: format!("{:.0}", cur_zoom * 100.0),
                                            x: cur_off_x.to_string(),
                                            y: cur_off_y.to_string(),
                                        }
                                    );
                                }
                                if cur_speed != original_speed {
                                    self.project.set_item_speed(sel_idx, cur_speed);
                                    self.status_message = format_message!(
                                        &intl,
                                        default_message: "Controller speed for item #{index} set to {speed}",
                                        description: "Status message after changing playlist item speed",
                                        values: {
                                            index: (sel_idx + 1).to_string(),
                                            speed: cur_speed.to_string(),
                                        }
                                    );
                                }
                            }
                        } else {
                            let empty_hint = format_message!(
                                &intl,
                                default_message: "ℹ Select an item in the playlist on the left to configure duration, rotation, and scale",
                                description: "Empty state hint when no playlist item is selected"
                            );
                            ui.label(egui::RichText::new(empty_hint).italics().color(Color32::GRAY));
                        }
                    });
                });
            });
        });
    }
}

/// Loads the embedded HoloFan application icon for window decoration and taskbar.
fn load_app_icon() -> Option<egui::IconData> {
    const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.png");
    let img = image::load_from_memory(ICON_BYTES).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

/// Launches the graphical user interface
pub fn run_gui() -> anyhow::Result<()> {
    run_gui_with_files(Vec::new())
}

/// Launches the graphical user interface with pre-loaded files
pub fn run_gui_with_files(files: Vec<PathBuf>) -> anyhow::Result<()> {
    let intl = crate::i18n::for_language(crate::i18n::Language::process_default());
    let mut wgpu_setup = eframe::egui_wgpu::WgpuSetupCreateNew::default();
    wgpu_setup.instance_descriptor.flags =
        eframe::egui_wgpu::wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER;
    wgpu_setup.instance_descriptor.backends = eframe::egui_wgpu::wgpu::Backends::all();
    wgpu_setup.power_preference = eframe::egui_wgpu::wgpu::PowerPreference::None;
    let adapter_err = format_message!(
        &intl,
        default_message: "No suitable graphics adapter found (DirectX/Vulkan/OpenGL)",
        description: "Error when no supported GPU adapter is found"
    );
    wgpu_setup.native_adapter_selector = Some(std::sync::Arc::new(move |adapters, surface| {
        if let Some(surface) = surface {
            for adapter in adapters {
                if adapter.is_surface_supported(surface) {
                    return Ok(adapter.clone());
                }
            }
        }
        if let Some(adapter) = adapters.first() {
            return Ok(adapter.clone());
        }
        Err(adapter_err.clone())
    }));

    let window_title = format_message!(
        &intl,
        default_message: "HoloFan — 3D Holographic LED Display Controller",
        description: "Main application window title"
    );

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1040.0, 700.0])
        .with_min_inner_size([840.0, 540.0])
        .with_title(window_title);

    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(wgpu_setup),
            ..Default::default()
        },
        ..Default::default()
    };

    let launch_err_msg = format_message!(
        &intl,
        default_message: "Failed to launch GUI: {error}",
        description: "Error message when eframe native window fails to launch",
        values: { error: "{error}".to_string() }
    );

    eframe::run_native(
        "Fan Software",
        native_options,
        Box::new(|_cc| Ok(Box::new(FanSoftwareApp::with_initial_files(files)))),
    )
    .map_err(|e| anyhow::anyhow!(launch_err_msg.replace("{error}", &e.to_string())))
}
