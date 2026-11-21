//! Integration tests for playlist project lifecycle and inspector settings.

use std::fs::File;
use std::path::{Path, PathBuf};

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, Rgb, RgbImage, Rgba, RgbaImage};
use tempfile::tempdir;

use holofan::format::HologramContainer;
use holofan::polar::{FitMode, TransformParams};
use holofan::project::FanProject;

/// Generates a synthetic single-color PNG image in the target directory.
fn create_test_png(dir: &Path, name: &str, color: [u8; 3]) -> PathBuf {
    let path = dir.join(name);
    let mut rgb = RgbImage::new(200, 200);
    for p in rgb.pixels_mut() {
        *p = Rgb(color);
    }
    rgb.save(&path).unwrap();
    path
}

/// Generates a synthetic multi-frame animated GIF in the target directory.
fn create_test_gif(dir: &Path, name: &str, frame_count: usize) -> PathBuf {
    let path = dir.join(name);
    let file = File::create(&path).unwrap();
    let mut encoder = GifEncoder::new(file);
    encoder.set_repeat(Repeat::Infinite).unwrap();

    for i in 0..frame_count {
        let mut img = RgbaImage::new(100, 100);
        let color = ((i * 50) % 255) as u8;
        for p in img.pixels_mut() {
            *p = Rgba([color, 150, 200, 255]);
        }
        let frame = Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1));
        encoder.encode_frame(frame).unwrap();
    }
    path
}

#[test]
fn test_project_lifecycle() {
    let dir = tempdir().unwrap();
    let mut proj = FanProject::new();
    assert_eq!(proj.item_count(), 0);
    assert_eq!(proj.total_frames(), 0);

    let img_path1 = create_test_png(dir.path(), "test1.png", [255, 0, 0]);
    let img_path2 = create_test_png(dir.path(), "test2.png", [0, 255, 0]);

    // Add first static image with 15 frames duration
    let count1 = proj
        .add_media_file(&img_path1, 15)
        .expect("Failed to add first image");
    assert_eq!(count1, 15);
    assert_eq!(proj.item_count(), 1);
    assert_eq!(proj.total_frames(), 15);
    assert_eq!(proj.items[0].frame_start, 0);
    assert_eq!(proj.items[0].frame_count, 15);
    assert_eq!(proj.items[0].speed, 50);

    // Add second static image with 10 frames duration
    let count2 = proj
        .add_media_file(&img_path2, 10)
        .expect("Failed to add second image");
    assert_eq!(count2, 10);
    assert_eq!(proj.item_count(), 2);
    assert_eq!(proj.total_frames(), 25);
    assert_eq!(proj.items[1].frame_start, 15);
    assert_eq!(proj.items[1].frame_count, 10);

    // Export to binary container and verify playlist ranges
    let container = proj.export_container().expect("Container export failed");
    assert_eq!(container.total_frames, 25);
    assert_eq!(container.playlist.len(), 2);
    assert_eq!(container.playlist[0].start_frame, 1);
    assert_eq!(container.playlist[0].end_frame, 15);
    assert_eq!(container.playlist[1].start_frame, 16);
    assert_eq!(container.playlist[1].end_frame, 25);

    // Write container to disk and read back
    let bin_path = dir.path().join("hologram.bin");
    container
        .write_to_file(&bin_path)
        .expect("Failed to write container");

    let loaded = HologramContainer::read_from_file(&bin_path).expect("Failed to read container");
    assert_eq!(loaded.total_frames, 25);
    assert_eq!(loaded.playlist.len(), 2);

    // Restore project from container and verify reconstructed state
    let mut proj2 = FanProject::new();
    proj2
        .load_from_container(loaded)
        .expect("Failed to decode loaded frames");
    assert_eq!(proj2.item_count(), 2);
    assert_eq!(proj2.total_frames(), 25);
    assert_eq!(proj2.items[0].frame_start, 0);
    assert_eq!(proj2.items[0].frame_count, 15);
    assert_eq!(proj2.items[1].frame_start, 15);
    assert_eq!(proj2.items[1].frame_count, 10);

    // Remove first item and verify subsequent offset recalculation
    let removed = proj2.remove_item(0);
    assert!(removed);
    assert_eq!(proj2.item_count(), 1);
    assert_eq!(proj2.total_frames(), 10);
    assert_eq!(proj2.items[0].frame_start, 0);
    assert_eq!(proj2.items[0].frame_count, 10);
}

#[test]
fn test_empty_project_export_fails() {
    let proj = FanProject::new();
    assert!(proj.export_container().is_err());
}

#[test]
fn item_speed_is_saved_in_playlist_without_reencoding_frames() {
    let dir = tempdir().unwrap();
    let image_path = create_test_png(dir.path(), "speed.png", [40, 80, 120]);
    let mut project = FanProject::new();
    project.add_media_file(&image_path, 1).unwrap();
    let frames_before = project.frames.clone();

    project.set_item_speed(0, 50);
    project.items[0].mode = 2;

    assert_eq!(project.frames, frames_before);
    let exported = project.export_container().unwrap();
    assert_eq!(exported.playlist[0].speed, 50);
    assert_eq!(exported.playlist[0].mode, 2);

    let mut reloaded = FanProject::new();
    reloaded.load_from_container(exported).unwrap();
    assert_eq!(reloaded.items[0].speed, 50);
    assert_eq!(reloaded.items[0].mode, 2);
}

#[test]
fn test_find_drives_hermetic() {
    let drives = FanProject::find_removable_drives();
    for drive in &drives {
        assert!(drive.is_dir());
    }
}

#[test]
fn test_seconds_duration_and_live_inspector() {
    let dir = tempdir().unwrap();
    let gif_path = create_test_gif(dir.path(), "sample.gif", 5);

    let mut proj = FanProject::new();

    // Adding 5-frame GIF with 2.0s duration generates 30 frames (6 complete loops at 15 FPS)
    let count = proj
        .add_media_file_with_duration(&gif_path, 2.0)
        .expect("Failed to add GIF item");
    assert_eq!(count, 30);
    assert_eq!(proj.total_frames(), 30);
    assert_eq!(proj.items[0].duration_seconds, 2.0);

    // Update inspector parameters on the fly
    proj.update_item_transform_full(
        0,
        1.0,
        &TransformParams {
            fit_mode: FitMode::Fill,
            rotate_deg: 90,
            flip_v: true,
            flip_h: false,
            zoom: 1.5,
            offset_x: 10,
            offset_y: -5,
        },
    )
    .unwrap();
    assert_eq!(proj.total_frames(), 15);
    assert_eq!(proj.items[0].frame_count, 15);
    assert_eq!(proj.items[0].rotate_deg, 90);
    assert!(proj.items[0].flip_v);
    assert!(!proj.items[0].flip_h);
    assert_eq!(proj.items[0].zoom, 1.5);
    assert_eq!(proj.items[0].fit_mode, FitMode::Fill);
    assert_eq!(proj.items[0].offset_x, 10);
    assert_eq!(proj.items[0].offset_y, -5);

    // Adding second item with 3.0s duration appends 45 frames
    let count2 = proj
        .add_media_file_with_duration(&gif_path, 3.0)
        .expect("Failed to add second item");
    assert_eq!(count2, 45);
    assert_eq!(proj.total_frames(), 60);
    assert_eq!(proj.items[0].frame_count, 15);
    assert_eq!(proj.items[1].frame_start, 15);
    assert_eq!(proj.items[1].frame_count, 45);
}

#[test]
fn playlist_defaults_keep_gif_source_duration_and_five_seconds_for_stills() {
    let dir = tempdir().unwrap();
    let gif_path = create_test_gif(dir.path(), "short.gif", 5);
    let still_path = create_test_png(dir.path(), "still.png", [80, 120, 160]);
    let mut project = FanProject::new();

    project.add_media_file_for_playlist(&gif_path).unwrap();
    project.add_media_file_for_playlist(&still_path).unwrap();

    assert!((project.items[0].duration_seconds - 0.5).abs() < 0.001);
    assert_eq!(project.items[0].frame_count, 8);
    assert_eq!(project.items[1].duration_seconds, 5.0);
    assert_eq!(project.items[1].frame_count, 75);
}

#[test]
fn background_transform_preparation_is_transactional_and_tracks_item_identity() {
    let dir = tempdir().unwrap();
    let first = create_test_png(dir.path(), "first.png", [220, 30, 30]);
    let second = create_test_png(dir.path(), "second.png", [30, 220, 30]);
    let mut project = FanProject::new();
    project.add_media_file(&first, 1).unwrap();
    project.add_media_file(&second, 1).unwrap();
    let frames_before = project.frames.clone();
    let first_id = project.items[0].id;

    let prepared = project
        .prepare_item_transform(
            0,
            2.0,
            &TransformParams {
                rotate_deg: 90,
                ..TransformParams::default()
            },
            || false,
        )
        .unwrap();
    assert_eq!(project.frames, frames_before);
    assert_eq!(project.items[0].duration_seconds, 1.0 / 15.0);

    project.move_item_down(0).unwrap();
    project.commit_item_transform(prepared).unwrap();
    let updated = project
        .items
        .iter()
        .find(|item| item.id == first_id)
        .unwrap();
    assert_eq!(updated.duration_seconds, 2.0);
    assert_eq!(updated.rotate_deg, 90);
    assert_eq!(updated.frame_count, 30);
    assert_eq!(updated.frame_start, 1);
    assert_eq!(project.total_frames(), 31);
}
