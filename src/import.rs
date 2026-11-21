//! Shared rules for selecting media files for a playlist import.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::format::{MAX_PLAYLIST_ENTRIES, MAX_SUPPORTED_FRAMES};

pub const MEDIA_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];
pub const GUI_MEDIA_DURATION_SECONDS: f32 = 5.0;
pub const GUI_MEDIA_FRAMES_PER_ITEM: usize = 75;

pub fn is_supported_media_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            MEDIA_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// Returns supported files directly inside `folder`, in stable playlist order.
pub fn list_folder_media(folder: &Path) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(folder)
        .with_context(|| format!("Failed to read media folder {}", folder.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("Failed to read an entry in {}", folder.display()))?;
        let path = entry.path();
        if path.is_file() && is_supported_media_path(&path) {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Normalizes a file-picker selection to the same deterministic playlist order.
pub fn normalize_media_selection(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut paths: Vec<_> = paths
        .into_iter()
        .filter(|path| path.is_file() && is_supported_media_path(path))
        .collect();
    paths.sort();
    paths
}

/// Checks both on-disk playlist limits before a batch can mutate the project.
pub fn preflight_capacity(
    existing_items: usize,
    existing_frames: usize,
    incoming_items: usize,
) -> Result<()> {
    let incoming_frames = incoming_items
        .checked_mul(GUI_MEDIA_FRAMES_PER_ITEM)
        .context("Import frame count overflow")?;
    preflight_capacity_with_frames(
        existing_items,
        existing_frames,
        incoming_items,
        incoming_frames,
    )
}

pub fn preflight_capacity_with_frames(
    existing_items: usize,
    existing_frames: usize,
    incoming_items: usize,
    incoming_frames: usize,
) -> Result<()> {
    let total_items = existing_items
        .checked_add(incoming_items)
        .context("Playlist item count overflow")?;
    if total_items > MAX_PLAYLIST_ENTRIES {
        bail!(
            "Import would create {total_items} playlist items; the format supports at most {MAX_PLAYLIST_ENTRIES}"
        );
    }

    let total_frames = existing_frames
        .checked_add(incoming_frames)
        .context("Project frame count overflow")?;
    if total_frames > MAX_SUPPORTED_FRAMES {
        bail!(
            "Import would create {total_frames} frames; the format supports at most {MAX_SUPPORTED_FRAMES}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn folder_import_is_flat_filtered_case_insensitive_and_sorted() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["z.JPG", "10.png", "2.gif", "ignored.txt", "ready.BIN"] {
            fs::write(temp.path().join(name), b"fixture").unwrap();
        }
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::write(temp.path().join("nested/inside.png"), b"fixture").unwrap();

        let names: Vec<_> = list_folder_media(temp.path())
            .unwrap()
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["10.png", "2.gif", "z.JPG"]);
    }

    #[test]
    fn folder_scan_surfaces_directory_errors_and_empty_results() {
        let temp = tempfile::tempdir().unwrap();
        assert!(list_folder_media(&temp.path().join("missing")).is_err());
        assert!(list_folder_media(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn selection_sorts_supported_regular_files_without_deduplicating() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("b.webp");
        let second = temp.path().join("a.jpeg");
        let unsupported = temp.path().join("x.txt");
        fs::write(&first, b"fixture").unwrap();
        fs::write(&second, b"fixture").unwrap();
        fs::write(&unsupported, b"fixture").unwrap();

        let files =
            normalize_media_selection([first.clone(), unsupported, second.clone(), first.clone()]);
        assert_eq!(files, [second, first.clone(), first]);
    }

    #[test]
    fn capacity_preflight_accepts_exact_limits_and_rejects_overflow_before_import() {
        assert!(preflight_capacity(0, 0, MAX_PLAYLIST_ENTRIES).is_ok());
        assert!(preflight_capacity(0, 0, MAX_PLAYLIST_ENTRIES + 1).is_err());
        assert!(preflight_capacity(0, MAX_SUPPORTED_FRAMES - GUI_MEDIA_FRAMES_PER_ITEM, 1).is_ok());
        assert!(preflight_capacity(0, MAX_SUPPORTED_FRAMES, 1).is_err());
        assert!(preflight_capacity(usize::MAX, 0, 1).is_err());
    }
}
