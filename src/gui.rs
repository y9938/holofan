//! Slint desktop editor. The controller and `FanProject` remain the source of truth.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use image::RgbImage;
use slint::{ModelRc, SharedPixelBuffer, SharedString, VecModel};

use crate::{
    device::FanDevice,
    format::HologramContainer,
    import, media,
    polar::{decode_polar_to_image, FitMode, TransformParams},
    project::{FanProject, PreparedItemTransform, PreparedMedia},
};

slint::include_modules!();

struct EditorSession {
    project: FanProject,
    filename_draft: String,
    selected: Option<usize>,
    frame: usize,
    playing: bool,
    preview_fps: f32,
    last_tick: Instant,
    language: crate::i18n::Language,
    status: String,
    device_status: String,
    drives: Vec<PathBuf>,
    pending_import: Option<PendingImport>,
    preview_cache: Option<(u64, usize, slint::Image)>,
    transform_draft: Option<TransformDraft>,
    transform_worker: Option<TransformWorker>,
    transform_revision: u64,
}

#[derive(Clone)]
struct TransformDraft {
    item_id: usize,
    params: TransformParams,
    duration: f32,
    revision: u64,
    changed_at: Instant,
}

struct TransformWorker {
    revision: u64,
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Result<PreparedItemTransform, String>>,
}

struct PendingImport {
    receiver: Receiver<ImportMessage<PreparedMedia>>,
    cancel: Arc<AtomicBool>,
    total: usize,
    processed: usize,
    added: usize,
    failures: Vec<String>,
    capacity_error: Option<String>,
    progress: String,
}

enum ImportMessage<T = PreparedMedia> {
    Scanning {
        index: usize,
        path: PathBuf,
    },
    PlanComplete {
        plan_count: usize,
        failures: Vec<String>,
        capacity_error: Option<String>,
    },
    Started {
        index: usize,
        path: PathBuf,
    },
    Prepared {
        path: PathBuf,
        media: T,
    },
    Failed {
        path: PathBuf,
        error: String,
    },
    Finished {
        cancelled: bool,
    },
}

impl EditorSession {
    fn new(files: Vec<PathBuf>) -> Self {
        let mut session = Self {
            project: FanProject::new(),
            filename_draft: "hologram.bin".into(),
            selected: None,
            frame: 0,
            playing: false,
            preview_fps: 15.0,
            last_tick: Instant::now(),
            language: crate::i18n::Language::process_default(),
            status: "Ready · Add media or open a .bin file".into(),
            device_status: "Not connected".into(),
            drives: FanProject::find_removable_drives(),
            pending_import: None,
            preview_cache: None,
            transform_draft: None,
            transform_worker: None,
            transform_revision: 0,
        };
        if files.is_empty() {
            let default_bin = PathBuf::from("hologram.bin");
            if default_bin.exists() {
                if let Err(error) = session.open_bin(&default_bin) {
                    session.status = format!("Failed to open hologram.bin: {error:#}");
                }
            }
        } else {
            for path in files {
                if let Err(error) = session.load_path(&path) {
                    session.status = format!("Failed to load {}: {error:#}", path.display());
                }
            }
        }
        session
    }

    fn load_path(&mut self, path: &Path) -> Result<()> {
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bin"))
        {
            self.open_bin(path).map(|_| ())
        } else {
            self.add_media(path)
        }
    }

    fn open_bin(&mut self, path: &Path) -> Result<usize> {
        let container = HologramContainer::read_from_file(path)?;
        let frames = container.frames.len();
        self.cancel_pending_import();
        self.invalidate_transform();
        self.project.load_from_container(container)?;
        self.frame = 0;
        self.selected = (!self.project.items.is_empty()).then_some(0);
        self.project.output_filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.filename_draft = self.project.output_filename.clone();
        self.status = format!("Opened {} · {frames} frames", path.display());
        Ok(frames)
    }

    fn add_media(&mut self, path: &Path) -> Result<()> {
        self.project.add_media_file_for_playlist(path)?;
        self.selected = Some(self.project.item_count() - 1);
        self.frame = self
            .project
            .items
            .last()
            .map(|item| item.frame_start)
            .unwrap_or(0);
        self.status = format!("Added {}", path.display());
        Ok(())
    }

    fn refresh(&mut self, ui: &MainWindow) {
        self.poll_transform_worker();
        let items: Vec<SharedString> = self
            .project
            .items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                format!(
                    "{:02}  {}   ·   {:.1}s   ·   {}°",
                    idx + 1,
                    item.title,
                    item.duration_seconds,
                    item.rotate_deg
                )
                .into()
            })
            .collect();
        ui.set_items(ModelRc::new(VecModel::from(items)));
        ui.set_selected_index(self.selected.map(|n| n as i32).unwrap_or(-1));
        ui.set_filename(self.filename_draft.clone().into());
        ui.set_language_index(
            crate::i18n::Language::available()
                .iter()
                .position(|lang| *lang == self.language)
                .unwrap_or(0) as i32,
        );
        ui.set_status(self.status.clone().into());
        ui.set_device_status(self.device_status.clone().into());
        ui.set_total_frames(self.project.total_frames() as i32);
        ui.set_current_frame(self.frame.min(i32::MAX as usize) as f32);
        ui.set_preview_fps(self.preview_fps.round() as i32);
        ui.set_frame_position(
            format!(
                "{} / {}",
                if self.project.total_frames() == 0 {
                    0
                } else {
                    self.frame + 1
                },
                self.project.total_frames()
            )
            .into(),
        );
        ui.set_frame_summary(
            format!(
                "{} items · {} frames · {:.1} s",
                self.project.item_count(),
                self.project.total_frames(),
                self.project.total_duration_seconds()
            )
            .into(),
        );
        ui.set_playing(self.playing);
        ui.set_import_active(self.pending_import.is_some());
        if let Some(task) = &self.pending_import {
            ui.set_import_progress(task.progress.clone().into());
        }
        if let Some(idx) = self.selected.filter(|idx| *idx < self.project.items.len()) {
            let item = &self.project.items[idx];
            let (duration, params) = self
                .transform_draft
                .as_ref()
                .filter(|draft| draft.item_id == item.id)
                .map(|draft| (draft.duration, draft.params.clone()))
                .unwrap_or_else(|| (item.duration_seconds, item.transform_params()));
            ui.set_item_title(format!("Item {} · {}", idx + 1, item.title).into());
            ui.set_duration_tenths((duration * 10.0).round() as i32);
            ui.set_duration_input(format!("{:.1}", duration).into());
            ui.set_controller_speed(item.speed as i32);
            ui.set_zoom_percent((params.zoom * 100.0).round() as i32);
            ui.set_offset_x(params.offset_x);
            ui.set_offset_y(params.offset_y);
            ui.set_rotation(params.rotate_deg);
            ui.set_fit_mode(match params.fit_mode {
                FitMode::Fit => 0,
                FitMode::Fill => 1,
                FitMode::Stretch => 2,
            });
            ui.set_flip_v(params.flip_v);
            ui.set_flip_h(params.flip_h);
        } else {
            ui.set_item_title("Select an item in the playlist".into());
        }
        let preview_key = (self.project.frame_revision(), self.frame);
        if let Some((revision, frame_index, image)) = &self.preview_cache {
            if (*revision, *frame_index) == preview_key {
                ui.set_preview(image.clone());
                return;
            }
        }
        if let Some(frame) = self.project.frames.get(self.frame) {
            match decode_polar_to_image(frame, &self.project.config) {
                Ok(img) => {
                    let rgb: RgbImage = img;
                    let pixels = SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(
                        rgb.as_raw(),
                        rgb.width(),
                        rgb.height(),
                    );
                    let image = slint::Image::from_rgb8(pixels);
                    self.preview_cache = Some((preview_key.0, preview_key.1, image.clone()));
                    ui.set_preview(image);
                }
                Err(error) => {
                    self.status = format!("Preview decode failed: {error:#}");
                    ui.set_status(self.status.clone().into());
                }
            }
        } else {
            self.preview_cache = None;
            ui.set_preview(slint::Image::default());
        }
    }

    fn save(&mut self) {
        if self.transform_draft.is_some() || self.transform_worker.is_some() {
            self.status = "Wait for the current item update before saving".into();
            return;
        }
        let result = (|| -> Result<()> {
            self.commit_filename()?;
            let container = self.project.export_container()?;
            let path = rfd::FileDialog::new()
                .set_file_name(&self.project.output_filename)
                .add_filter("Hologram fan file", &["bin"])
                .save_file();
            if let Some(mut path) = path {
                if !path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("bin"))
                {
                    path.set_extension("bin");
                }
                container.write_to_file(&path)?;
                self.project.output_filename = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                self.filename_draft = self.project.output_filename.clone();
                self.status = format!("Saved {}", path.display());
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.status = format!("Save failed: {error:#}");
        }
    }

    fn write_storage(&mut self) {
        if self.transform_draft.is_some() || self.transform_worker.is_some() {
            self.status = "Wait for the current item update before writing to storage".into();
            return;
        }
        if let Err(error) = self.commit_filename() {
            self.status = format!("Write failed: {error:#}");
            return;
        }
        self.drives = FanProject::find_removable_drives();
        let target_dir = self.drives.first().cloned().or_else(|| {
            rfd::FileDialog::new()
                .set_title("Select SD card or USB root")
                .pick_folder()
        });
        let Some(dir) = target_dir else {
            return;
        };
        let result = (|| -> Result<PathBuf> {
            let container = self.project.export_container()?;
            let target = dir.join(&self.project.output_filename);
            container.write_to_file(&target)?;
            Ok(target)
        })();
        self.status = match result {
            Ok(path) => format!("Written to {}", path.display()),
            Err(error) => format!("Write failed: {error:#}"),
        };
    }

    fn edit_filename(&mut self, name: &str) {
        self.filename_draft = name.to_string();
    }

    fn commit_filename(&mut self) -> Result<()> {
        let name = self.filename_draft.trim();
        if name.is_empty() {
            self.filename_draft = self.project.output_filename.clone();
            return Ok(());
        }
        anyhow::ensure!(
            !name.chars().any(|ch| ch.is_control()
                || matches!(ch, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*')),
            "file name contains characters that are not allowed"
        );
        let normalized = if name.to_ascii_lowercase().ends_with(".bin") {
            name.to_string()
        } else {
            format!("{name}.bin")
        };
        self.project.output_filename = normalized.clone();
        self.filename_draft = normalized;
        Ok(())
    }

    fn update_transform(&mut self, f: impl FnOnce(&mut TransformParams, &mut f32)) {
        if self.pending_import.is_some() {
            self.status = "Wait for media import to finish before editing an item".into();
            return;
        }
        let Some(idx) = self.selected.filter(|idx| *idx < self.project.items.len()) else {
            return;
        };
        let item = &self.project.items[idx];
        if self
            .transform_draft
            .as_ref()
            .is_some_and(|draft| draft.item_id != item.id)
        {
            self.status =
                "Wait for the selected item's update to finish before editing another item".into();
            return;
        }
        let prior = self
            .transform_draft
            .as_ref()
            .filter(|draft| draft.item_id == item.id);
        let mut params = prior
            .map(|draft| draft.params.clone())
            .unwrap_or_else(|| item.transform_params());
        let mut duration = prior
            .map(|draft| draft.duration)
            .unwrap_or(item.duration_seconds);
        f(&mut params, &mut duration);
        if let Some(worker) = &self.transform_worker {
            worker.cancel.store(true, Ordering::Release);
        }
        self.transform_revision = self.transform_revision.wrapping_add(1);
        self.transform_draft = Some(TransformDraft {
            item_id: item.id,
            params,
            duration: duration.clamp(0.2, 60.0),
            revision: self.transform_revision,
            changed_at: Instant::now(),
        });
        self.status = "Updating item…".into();
    }

    fn invalidate_transform(&mut self) {
        if let Some(worker) = &self.transform_worker {
            worker.cancel.store(true, Ordering::Release);
        }
        self.transform_worker = None;
        self.transform_draft = None;
        self.transform_revision = self.transform_revision.wrapping_add(1);
    }

    fn cancel_pending_import(&mut self) {
        if let Some(task) = &self.pending_import {
            task.cancel.store(true, Ordering::Release);
        }
        self.pending_import = None;
    }

    fn poll_transform_worker(&mut self) {
        if let Some(worker) = self.transform_worker.take() {
            match worker.receiver.try_recv() {
                Ok(Ok(prepared)) => {
                    if self
                        .transform_draft
                        .as_ref()
                        .is_some_and(|draft| draft.revision == worker.revision)
                    {
                        match self.project.commit_item_transform(prepared) {
                            Ok(()) => {
                                if let Some(idx) =
                                    self.selected.filter(|i| *i < self.project.items.len())
                                {
                                    self.frame = self.project.items[idx].frame_start;
                                }
                                self.transform_draft = None;
                                self.status = "Item updated".into();
                            }
                            Err(error) => {
                                self.transform_draft = None;
                                self.status = format!("Could not update item: {error:#}");
                            }
                        }
                    }
                }
                Ok(Err(error)) => {
                    if self
                        .transform_draft
                        .as_ref()
                        .is_some_and(|draft| draft.revision == worker.revision)
                    {
                        self.transform_draft = None;
                        if error != "Transform cancelled" && error != "GIF import cancelled" {
                            self.status = format!("Could not update item: {error}");
                        }
                    }
                }
                Err(TryRecvError::Empty) => self.transform_worker = Some(worker),
                Err(TryRecvError::Disconnected) => {
                    if self
                        .transform_draft
                        .as_ref()
                        .is_some_and(|draft| draft.revision == worker.revision)
                    {
                        self.transform_draft = None;
                        self.status = "Transform worker stopped without a result".into();
                    }
                }
            }
        }

        let ready = self.transform_draft.as_ref().is_some_and(|draft| {
            self.transform_worker.is_none()
                && draft.changed_at.elapsed() >= Duration::from_millis(180)
        });
        if ready {
            let draft = self.transform_draft.as_ref().unwrap().clone();
            let item = self
                .project
                .items
                .iter()
                .find(|item| item.id == draft.item_id)
                .cloned();
            let config = self.project.config.clone();
            let (sender, receiver) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel.clone();
            thread::spawn(move || {
                let result = item
                    .context("Selected item disappeared before transform started")
                    .and_then(|item| {
                        FanProject::prepare_item_transform_snapshot(
                            item,
                            config,
                            draft.duration,
                            &draft.params,
                            || worker_cancel.load(Ordering::Acquire),
                        )
                    })
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send(result);
            });
            self.transform_worker = Some(TransformWorker {
                revision: draft.revision,
                cancel,
                receiver,
            });
        }
    }

    fn seek(&mut self, frame: usize) {
        if self.project.total_frames() == 0 {
            self.frame = 0;
            return;
        }
        self.frame = frame.min(self.project.total_frames() - 1);
        if let Some((idx, item)) = self.project.items.iter().enumerate().find(|(_, item)| {
            self.frame >= item.frame_start && self.frame < item.frame_start + item.frame_count
        }) {
            self.selected = Some(idx);
            let _ = item;
        }
    }
}

fn supported_drop(path: &Path) -> bool {
    path.is_dir()
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("bin") || import::is_supported_media_path(path)
            })
}

fn begin_media_import(session: &Rc<RefCell<EditorSession>>, ui: &MainWindow, paths: Vec<PathBuf>) {
    let paths = import::normalize_media_selection(paths);
    let mut state = session.borrow_mut();
    if state.pending_import.is_some() {
        return;
    }
    if state.transform_draft.is_some() || state.transform_worker.is_some() {
        state.status = "Wait for the current item update before importing media".into();
        state.refresh(ui);
        return;
    }
    if paths.is_empty() {
        state.status = "No supported media files found".into();
        state.refresh(ui);
        return;
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let existing_items = state.project.items.len();
    let existing_frames = state.project.total_frames();
    state.pending_import = Some(PendingImport {
        receiver,
        cancel: cancel.clone(),
        total: paths.len(),
        processed: 0,
        added: 0,
        failures: Vec::new(),
        capacity_error: None,
        progress: format!("Checking {} file(s)…", paths.len()),
    });
    state.playing = false;
    ui.set_show_import_result(false);
    state.status = format!("Importing {} file(s)…", paths.len());
    state.refresh(ui);
    drop(state);

    let config_project = FanProject::new();
    thread::spawn(move || {
        run_media_import_batch(
            paths,
            cancel,
            existing_items,
            existing_frames,
            |path, cancel| {
                media::plan_playlist_media_cancellable(path, crate::project::DEFAULT_FPS, || {
                    cancel.load(Ordering::Relaxed)
                })
            },
            |plan, cancel| {
                config_project
                    .prepare_media_plan_cancellable(plan, || cancel.load(Ordering::Relaxed))
            },
            |plan: &media::PlaylistMediaPlan| plan.output_frames,
            |message| sender.send(message).is_ok(),
        );
    });
}

fn run_media_import_batch<P, T, Plan, Prepare, FrameCount, Send>(
    paths: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
    existing_items: usize,
    existing_frames: usize,
    mut plan_media: Plan,
    mut prepare_media: Prepare,
    frame_count: FrameCount,
    mut send: Send,
) where
    Plan: FnMut(&Path, &AtomicBool) -> Result<P>,
    Prepare: FnMut(P, &AtomicBool) -> Result<T>,
    FrameCount: Fn(&P) -> usize,
    Send: FnMut(ImportMessage<T>) -> bool,
{
    let total = paths.len();
    let mut cancelled = false;
    let mut plans = Vec::with_capacity(total);
    let mut failures = Vec::new();
    for (index, path) in paths.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        if !send(ImportMessage::Scanning {
            index: index + 1,
            path: path.clone(),
        }) {
            return;
        }
        match plan_media(&path, &cancel) {
            Ok(plan) => plans.push((path, plan)),
            Err(_) if cancel.load(Ordering::Relaxed) => {
                cancelled = true;
                break;
            }
            Err(error) => failures.push(format!("{}: {error:#}", path.display())),
        }
    }
    let incoming_frames = plans.iter().try_fold(0usize, |count, (_, plan)| {
        count.checked_add(frame_count(plan))
    });
    let capacity_error = if cancelled {
        None
    } else {
        incoming_frames
            .ok_or_else(|| anyhow::anyhow!("Import frame count overflow"))
            .and_then(|frames| {
                import::preflight_capacity_with_frames(
                    existing_items,
                    existing_frames,
                    plans.len(),
                    frames,
                )
            })
            .err()
            .map(|error| format!("{error:#}"))
    };
    if !send(ImportMessage::PlanComplete {
        plan_count: plans.len(),
        failures,
        capacity_error: capacity_error.clone(),
    }) {
        return;
    }
    if cancelled || capacity_error.is_some() {
        let _ = send(ImportMessage::Finished { cancelled });
        return;
    }

    for (index, (path, plan)) in plans.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        if !send(ImportMessage::Started {
            index: index + 1,
            path: path.clone(),
        }) {
            return;
        }
        let message = match prepare_media(plan, &cancel) {
            Ok(media) => ImportMessage::Prepared { path, media },
            Err(_) if cancel.load(Ordering::Relaxed) => {
                cancelled = true;
                break;
            }
            Err(error) => ImportMessage::Failed {
                path,
                error: format!("{error:#}"),
            },
        };
        if !send(message) {
            return;
        }
    }
    cancelled |= cancel.load(Ordering::Relaxed);
    let _ = send(ImportMessage::Finished { cancelled });
}

fn begin_folder_import(session: &Rc<RefCell<EditorSession>>, ui: &MainWindow, folder: &Path) {
    match import::list_folder_media(folder) {
        Ok(paths) => begin_media_import(session, ui, paths),
        Err(error) => {
            let mut state = session.borrow_mut();
            state.status = format!("Could not import folder: {error:#}");
            state.refresh(ui);
        }
    }
}

impl EditorSession {
    fn advance_media_import(&mut self, ui: &MainWindow) {
        let message = match self.pending_import.as_ref() {
            Some(task) => match task.receiver.try_recv() {
                Ok(message) => Some(message),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(ImportMessage::Finished { cancelled: true })
                }
            },
            None => None,
        };
        let Some(message) = message else {
            return;
        };

        match message {
            ImportMessage::Scanning { index, path } => {
                if let Some(task) = &mut self.pending_import {
                    task.progress = format!("Checking {index}/{} · {}", task.total, path.display());
                }
                self.refresh(ui);
            }
            ImportMessage::PlanComplete {
                plan_count,
                failures,
                capacity_error,
            } => {
                if let Some(task) = &mut self.pending_import {
                    task.failures.extend(failures);
                    task.capacity_error = capacity_error.clone();
                    task.progress = format!("Preparing {plan_count} valid file(s)…");
                }
                if let Some(error) = capacity_error {
                    self.status = format!("Import rejected: {error}");
                }
                self.refresh(ui);
            }
            ImportMessage::Started { index, path } => {
                if let Some(task) = &mut self.pending_import {
                    task.progress = format!("{index}/{} · {}", task.total, path.display());
                }
                self.refresh(ui);
            }
            ImportMessage::Prepared { path, media } => {
                match self.project.append_prepared_media(media) {
                    Ok(_) => {
                        self.selected = Some(self.project.item_count() - 1);
                        self.frame = self
                            .project
                            .items
                            .last()
                            .map(|item| item.frame_start)
                            .unwrap_or(0);
                        if let Some(task) = &mut self.pending_import {
                            task.added += 1;
                            task.processed += 1;
                        }
                    }
                    Err(error) => {
                        if let Some(task) = &mut self.pending_import {
                            task.failures.push(format!("{}: {error:#}", path.display()));
                            task.processed += 1;
                        }
                    }
                }
                if let Some(task) = &mut self.pending_import {
                    task.progress = format!("{}/{} processed", task.processed, task.total);
                }
                self.refresh(ui);
            }
            ImportMessage::Failed { path, error } => {
                if let Some(task) = &mut self.pending_import {
                    task.failures.push(format!("{}: {error}", path.display()));
                    task.processed += 1;
                    task.progress = format!("{}/{} processed", task.processed, task.total);
                }
                self.refresh(ui);
            }
            ImportMessage::Finished { cancelled } => {
                if let Some(task) = self.pending_import.take() {
                    let failure_count = task.failures.len();
                    if failure_count > 0 || task.capacity_error.is_some() {
                        let mut report = format!(
                            "Added {} of {} file(s). {} file(s) could not be imported.\n\n",
                            task.added, task.total, failure_count
                        );
                        if let Some(error) = &task.capacity_error {
                            report.push_str(&format!("Batch rejected: {error}\n\n"));
                        }
                        for failure in task.failures.iter().take(10) {
                            report.push_str("• ");
                            report.push_str(failure);
                            report.push('\n');
                        }
                        if failure_count > 10 {
                            report.push_str(&format!("… and {} more", failure_count - 10));
                        }
                        ui.set_import_result(report.into());
                        ui.set_show_import_result(true);
                    }
                    let details = task
                        .failures
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("; ");
                    let mut status = if let Some(error) = &task.capacity_error {
                        format!("Import rejected · {error}")
                    } else if cancelled {
                        format!("Import canceled · added {} of {}", task.added, task.total)
                    } else {
                        format!("Import complete · added {} of {}", task.added, task.total)
                    };
                    if failure_count > 0 {
                        status.push_str(&format!(" · {failure_count} failed: {details}"));
                        if failure_count > 3 {
                            status.push_str(&format!("; and {} more", failure_count - 3));
                        }
                    }
                    self.status = status;
                }
                self.refresh(ui);
            }
        }
    }

    fn cancel_media_import(&mut self) {
        if let Some(task) = &mut self.pending_import {
            task.cancel.store(true, Ordering::Relaxed);
            task.progress = "Stopping after the current file…".into();
        }
    }
}

fn wire_ui(ui: &MainWindow, session: Rc<RefCell<EditorSession>>) {
    let weak = ui.as_weak();
    {
        let weak = ui.as_weak();
        ui.on_add_media(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_show_add_menu(true);
            }
        });
        let session_files = session.clone();
        let weak = ui.as_weak();
        ui.on_add_media_files(move || {
            if let Some(paths) = rfd::FileDialog::new()
                .add_filter("Images and animations", import::MEDIA_EXTENSIONS)
                .pick_files()
            {
                if let Some(ui) = weak.upgrade() {
                    begin_media_import(&session_files, &ui, paths);
                }
            }
        });
        let session_folder = session.clone();
        let weak = ui.as_weak();
        ui.on_add_media_folder(move || {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                if let Some(ui) = weak.upgrade() {
                    begin_folder_import(&session_folder, &ui, &folder);
                }
            }
        });
        let session_cancel = session.clone();
        let weak = ui.as_weak();
        ui.on_cancel_import(move || {
            if let Some(ui) = weak.upgrade() {
                let mut state = session_cancel.borrow_mut();
                state.cancel_media_import();
                state.refresh(&ui);
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_dismiss_import_result(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_show_import_result(false);
            }
        });
    }
    {
        use slint::winit_030::{winit, EventResult, WinitWindowAccessor};

        let session = session.clone();
        let weak = ui.as_weak();
        ui.window().on_winit_window_event(move |_, event| {
            match event {
                winit::event::WindowEvent::HoveredFile(path) => {
                    if supported_drop(path) {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_drop_hovered(true);
                        }
                    }
                }
                winit::event::WindowEvent::HoveredFileCancelled => {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_drop_hovered(false);
                    }
                }
                winit::event::WindowEvent::DroppedFile(path) => {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_drop_hovered(false);
                    }
                    if path.is_dir() {
                        if let Some(ui) = weak.upgrade() {
                            begin_folder_import(&session, &ui, path);
                        }
                    } else if import::is_supported_media_path(path) {
                        if let Some(ui) = weak.upgrade() {
                            begin_media_import(&session, &ui, vec![path.clone()]);
                        }
                    } else if supported_drop(path) {
                        let mut state = session.borrow_mut();
                        if let Err(error) = state.load_path(path) {
                            state.status = format!("Failed to load {}: {error:#}", path.display());
                        }
                        if let Some(ui) = weak.upgrade() {
                            state.refresh(&ui);
                        }
                    }
                }
                _ => {}
            }
            EventResult::Propagate
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_open_bin(move || {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Hologram fan files", &["bin"])
                .pick_file()
            {
                let mut state = session.borrow_mut();
                if let Err(e) = state.open_bin(&path) {
                    state.status = format!("Open failed: {e:#}");
                }
                if let Some(ui) = weak.upgrade() {
                    state.refresh(&ui);
                }
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_save_bin(move || {
            let mut state = session.borrow_mut();
            state.save();
            if let Some(ui) = weak.upgrade() {
                state.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_write_storage(move || {
            let mut state = session.borrow_mut();
            state.write_storage();
            if let Some(ui) = weak.upgrade() {
                state.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_export_png(move || {
            let mut state = session.borrow_mut();
            let outcome: Result<String> = if state.project.frames.is_empty() {
                Err(anyhow::anyhow!("No frames to export"))
            } else if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                media::export_polar_frames_to_png(
                    &state.project.frames,
                    &folder,
                    &state.project.config,
                )
                .map(|n| format!("Exported {n} frames to {}", folder.display()))
            } else {
                return;
            };
            state.status = outcome.unwrap_or_else(|e| format!("Export failed: {e:#}"));
            if let Some(ui) = weak.upgrade() {
                state.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_connect_hid(move || {
            let mut state = session.borrow_mut();
            match FanDevice::connect().and_then(|device| device.query_status()) {
                Ok(_) => {
                    state.device_status = format!(
                        "Connected · VID:PID {:04x}:{:04x}",
                        crate::device::VENDOR_ID,
                        crate::device::PRODUCT_ID
                    );
                    state.status = "Controller responded to USB HID status query".into();
                }
                Err(e) => {
                    state.device_status = format!("Not connected: {e}");
                    state.status = state.device_status.clone();
                }
            }
            if let Some(ui) = weak.upgrade() {
                state.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_select_item(move |idx| {
            let mut s = session.borrow_mut();
            if idx >= 0 && (idx as usize) < s.project.items.len() {
                s.selected = Some(idx as usize);
                s.frame = s.project.items[idx as usize].frame_start;
            }
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_remove_item(move || {
            let mut s = session.borrow_mut();
            if let Some(idx) = s.selected {
                let removing_id = s.project.items.get(idx).map(|item| item.id);
                if s.transform_draft
                    .as_ref()
                    .is_some_and(|draft| Some(draft.item_id) == removing_id)
                {
                    s.invalidate_transform();
                }
                s.project.remove_item(idx);
                s.selected = (!s.project.items.is_empty())
                    .then_some(idx.min(s.project.items.len().saturating_sub(1)));
                s.frame = s
                    .selected
                    .map(|i| s.project.items[i].frame_start)
                    .unwrap_or(0);
            }
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_move_up(move || {
            let mut s = session.borrow_mut();
            if let Some(idx) = s.selected {
                match s.project.move_item_up(idx) {
                    Ok(true) => s.selected = Some(idx - 1),
                    Ok(false) => {}
                    Err(error) => s.status = format!("Could not move item: {error:#}"),
                }
            }
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_move_down(move || {
            let mut s = session.borrow_mut();
            if let Some(idx) = s.selected {
                match s.project.move_item_down(idx) {
                    Ok(true) => s.selected = Some(idx + 1),
                    Ok(false) => {}
                    Err(error) => s.status = format!("Could not move item: {error:#}"),
                }
            }
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_clear_items(move || {
            let mut s = session.borrow_mut();
            s.cancel_pending_import();
            s.invalidate_transform();
            s.project.clear();
            s.selected = None;
            s.frame = 0;
            s.status = "Playlist cleared".into();
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_update_duration(move |tenths| {
            let mut s = session.borrow_mut();
            s.update_transform(|_, duration| *duration = (tenths as f32 / 10.0).clamp(0.2, 60.0));
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_update_duration_text(move |value| {
            let mut s = session.borrow_mut();
            match value.trim().parse::<f32>() {
                Ok(seconds) if seconds.is_finite() && (0.2..=60.0).contains(&seconds) => {
                    s.update_transform(|_, duration| *duration = seconds)
                }
                _ => s.status = "Duration must be between 0.2 and 60 seconds".into(),
            }
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_update_speed(move |speed| {
            let mut s = session.borrow_mut();
            if let Some(i) = s.selected {
                s.project.set_item_speed(i, speed.clamp(1, 255) as u8);
            }
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_update_zoom(move |zoom| {
            let mut s = session.borrow_mut();
            s.update_transform(|p, _| p.zoom = zoom as f32 / 100.0);
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_update_offset(move |x, y| {
            let mut s = session.borrow_mut();
            s.update_transform(|p, _| {
                p.offset_x = x.clamp(-150, 150);
                p.offset_y = y.clamp(-150, 150);
            });
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_update_rotation(move |rotation| {
            let mut s = session.borrow_mut();
            s.update_transform(|p, _| p.rotate_deg = rotation.rem_euclid(360));
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_update_fit(move |mode| {
            let mut s = session.borrow_mut();
            s.update_transform(|p, _| {
                p.fit_mode = match mode {
                    0 => FitMode::Fit,
                    2 => FitMode::Stretch,
                    _ => FitMode::Fill,
                }
            });
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_toggle_flip_v(move || {
            let mut s = session.borrow_mut();
            s.update_transform(|p, _| p.flip_v = !p.flip_v);
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_toggle_flip_h(move || {
            let mut s = session.borrow_mut();
            s.update_transform(|p, _| p.flip_h = !p.flip_h);
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_frame_step(move |delta| {
            let mut s = session.borrow_mut();
            let next = if delta < -1000 {
                0
            } else if delta > 1000 {
                s.project.total_frames().saturating_sub(1)
            } else {
                (s.frame as i64 + delta as i64)
                    .clamp(0, s.project.total_frames().saturating_sub(1) as i64)
                    as usize
            };
            s.seek(next);
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_frame_seek(move |frame| {
            let mut s = session.borrow_mut();
            s.seek(frame.max(0) as usize);
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_preview_fps_changed(move |fps| {
            let mut s = session.borrow_mut();
            s.preview_fps = fps.clamp(1, 60) as f32;
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_toggle_play(move || {
            let mut s = session.borrow_mut();
            s.playing = !s.playing;
            s.last_tick = Instant::now();
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        ui.on_filename_changed(move |name| {
            let mut s = session.borrow_mut();
            s.edit_filename(&name);
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_filename_accepted(move |name| {
            let mut s = session.borrow_mut();
            s.edit_filename(&name);
            if let Err(error) = s.commit_filename() {
                s.status = format!("Invalid file name: {error:#}");
            }
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
    {
        let session = session.clone();
        let weak = weak.clone();
        ui.on_language_changed(move |index| {
            let mut s = session.borrow_mut();
            if let Some(lang) = crate::i18n::Language::available().get(index.max(0) as usize) {
                s.language = *lang;
            }
            if let Some(ui) = weak.upgrade() {
                s.refresh(&ui);
            }
        });
    }
}

pub fn run_gui() -> Result<()> {
    run_gui_with_files(Vec::new())
}

pub fn run_gui_with_files(files: Vec<PathBuf>) -> Result<()> {
    let ui = MainWindow::new().context("create Slint main window")?;
    ui.window().set_size(slint::LogicalSize::new(1180.0, 780.0));
    let session = Rc::new(RefCell::new(EditorSession::new(files)));
    wire_ui(&ui, session.clone());
    session.borrow_mut().refresh(&ui);
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            if let Some(ui) = weak.upgrade() {
                let mut state = session.borrow_mut();
                state.advance_media_import(&ui);
                if state.playing
                    && state.project.total_frames() > 0
                    && state.last_tick.elapsed().as_secs_f32() >= 1.0 / state.preview_fps
                {
                    let next = (state.frame + 1) % state.project.total_frames();
                    state.seek(next);
                    state.last_tick = Instant::now();
                    state.refresh(&ui);
                }
            }
        },
    );
    ui.run().context("run Slint event loop")
}

#[cfg(test)]
mod import_tests {
    use super::*;

    #[test]
    fn batch_reports_corrupt_media_and_continues_with_valid_files() {
        let (sender, receiver) = mpsc::channel();
        run_media_import_batch(
            vec!["bad-plan", "bad-decode", "valid"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            Arc::new(AtomicBool::new(false)),
            0,
            0,
            |path, _| {
                if path.file_name().unwrap() == "bad-plan" {
                    anyhow::bail!("cannot inspect source")
                }
                Ok(path.to_path_buf())
            },
            |path, _| {
                if path.file_name().unwrap() == "bad-decode" {
                    anyhow::bail!("cannot decode source")
                }
                Ok(())
            },
            |_| 75,
            |message| sender.send(message).is_ok(),
        );
        let messages: Vec<_> = receiver.try_iter().collect();

        assert!(matches!(
            messages[3],
            ImportMessage::PlanComplete { plan_count: 2, .. }
        ));
        assert!(messages.iter().any(|message| matches!(
            message,
            ImportMessage::Failed { path, .. } if path.file_name().unwrap() == "bad-decode"
        )));
        assert!(messages.iter().any(|message| matches!(
            message,
            ImportMessage::Prepared { path, media: (), } if path.file_name().unwrap() == "valid"
        )));
        assert!(matches!(
            messages.last(),
            Some(ImportMessage::Finished { cancelled: false })
        ));
    }

    #[test]
    fn frame_limit_is_checked_for_the_whole_batch_before_any_item_is_prepared() {
        let (sender, receiver) = mpsc::channel();
        run_media_import_batch(
            vec![PathBuf::from("too-many-frames.gif")],
            Arc::new(AtomicBool::new(false)),
            0,
            crate::format::MAX_SUPPORTED_FRAMES - 10,
            |_, _| Ok(()),
            |_, _| -> Result<()> { panic!("rejected batch must not encode any item") },
            |_| 75,
            |message| sender.send(message).is_ok(),
        );
        let messages: Vec<_> = receiver.try_iter().collect();
        assert!(matches!(
            messages[1],
            ImportMessage::PlanComplete {
                capacity_error: Some(_),
                ..
            }
        ));
        assert!(!messages
            .iter()
            .any(|message| matches!(message, ImportMessage::Started { .. })));
        assert!(matches!(
            messages.last(),
            Some(ImportMessage::Finished { cancelled: false })
        ));
    }

    #[test]
    fn batch_cancel_before_start_does_not_prepare_or_mutate_any_media() {
        let cancel = Arc::new(AtomicBool::new(true));
        let (sender, receiver) = mpsc::channel();
        run_media_import_batch(
            vec![PathBuf::from("unread.png")],
            cancel,
            0,
            0,
            |_, _| -> Result<()> { panic!("cancelled batch must not scan media") },
            |_, _| -> Result<()> { panic!("cancelled batch must not encode media") },
            |_| 75,
            |message| sender.send(message).is_ok(),
        );
        let messages: Vec<_> = receiver.try_iter().collect();
        assert!(matches!(
            messages.last(),
            Some(ImportMessage::Finished { cancelled: true })
        ));
        assert!(!messages
            .iter()
            .any(|message| matches!(message, ImportMessage::Prepared { .. })));
    }

    #[test]
    fn batch_cancel_finishes_current_file_and_skips_remaining_files() {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_after_first = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        run_media_import_batch(
            vec![PathBuf::from("first.png"), PathBuf::from("second.png")],
            cancel,
            0,
            0,
            |path, _| Ok(path.to_path_buf()),
            |_, _| Ok(()),
            |_| 75,
            move |message| {
                if matches!(&message, ImportMessage::Started { index: 1, .. }) {
                    cancel_after_first.store(true, Ordering::Relaxed);
                }
                sender.send(message).is_ok()
            },
        );

        let messages: Vec<_> = receiver.try_iter().collect();
        assert!(
            matches!(messages.iter().find(|message| matches!(message, ImportMessage::Prepared { .. })), Some(ImportMessage::Prepared { path, media: (), }) if path == Path::new("first.png"))
        );
        assert!(matches!(
            messages.last(),
            Some(ImportMessage::Finished { cancelled: true })
        ));
        assert!(!messages.iter().any(|message| matches!(message, ImportMessage::Started { path, .. } if path == Path::new("second.png"))));
    }
}
