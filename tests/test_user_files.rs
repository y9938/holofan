//! Integration tests for the export pipeline and direct removable media writes.

use std::fs::File;
use std::path::{Path, PathBuf};

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, Rgb, RgbImage, Rgba, RgbaImage};
use tempfile::tempdir;

use holofan::format::{HologramContainer, HOLOGRAM_FRAME_SIZE};
use holofan::project::FanProject;

/// Generates a synthetic test image.
fn create_test_image(dir: &Path, name: &str, color: [u8; 3]) -> PathBuf {
    let path = dir.join(name);
    let mut rgb = RgbImage::new(150, 150);
    for p in rgb.pixels_mut() {
        *p = Rgb(color);
    }
    rgb.save(&path).unwrap();
    path
}

/// Generates a synthetic animated GIF with specified frame count.
fn create_test_gif(dir: &Path, name: &str, frame_count: usize) -> PathBuf {
    let path = dir.join(name);
    let file = File::create(&path).unwrap();
    let mut encoder = GifEncoder::new(file);
    encoder.set_repeat(Repeat::Infinite).unwrap();

    for i in 0..frame_count {
        let mut img = RgbaImage::new(80, 80);
        let color = ((i * 40) % 255) as u8;
        for p in img.pixels_mut() {
            *p = Rgba([color, 100, 180, 255]);
        }
        let frame = Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1));
        encoder.encode_frame(frame).unwrap();
    }
    path
}

#[test]
fn test_export_pipeline_and_removable_drive_hermetic() {
    let workspace = tempdir().unwrap();
    let sd_mock = tempdir().unwrap();

    let gif_file = create_test_gif(workspace.path(), "animation.gif", 4);
    let jpg_file = create_test_image(workspace.path(), "slide1.jpg", [220, 30, 30]);
    let png_file = create_test_image(workspace.path(), "slide2.png", [30, 220, 30]);

    let mut proj = FanProject::new();
    proj.output_filename = "campaign_2026.bin".to_string();

    // Populate project with multi-format media items
    let f1 = proj
        .add_media_file(&gif_file, 15)
        .expect("Failed to add GIF");
    assert_eq!(f1, 4);

    let f2 = proj
        .add_media_file(&jpg_file, 10)
        .expect("Failed to add JPG");
    assert_eq!(f2, 10);

    let f3 = proj
        .add_media_file(&png_file, 6)
        .expect("Failed to add PNG");
    assert_eq!(f3, 6);

    // Verify global frame totals: 4 + 10 + 6 = 20 frames
    assert_eq!(proj.item_count(), 3);
    assert_eq!(proj.total_frames(), 20);

    // Verify sequential frame ranges
    assert_eq!(proj.items[0].frame_start, 0);
    assert_eq!(proj.items[0].frame_count, 4);

    assert_eq!(proj.items[1].frame_start, 4);
    assert_eq!(proj.items[1].frame_count, 10);

    assert_eq!(proj.items[2].frame_start, 14);
    assert_eq!(proj.items[2].frame_count, 6);

    // Export to binary container
    let container = proj.export_container().expect("Container export failed");
    assert_eq!(container.total_frames, 20);
    assert_eq!(container.playlist.len(), 3);

    // Validate 1-based contiguous playlist ranges
    assert_eq!(container.playlist[0].start_frame, 1);
    assert_eq!(container.playlist[0].end_frame, 4);

    assert_eq!(container.playlist[1].start_frame, 5);
    assert_eq!(container.playlist[1].end_frame, 14);

    assert_eq!(container.playlist[2].start_frame, 15);
    assert_eq!(container.playlist[2].end_frame, 20);

    // Write container directly into removable drive root
    let target_path = sd_mock.path().join(&proj.output_filename);
    container
        .write_to_file(&target_path)
        .expect("Failed to write to drive");
    assert!(target_path.exists());

    // Read back and validate container on target drive
    let loaded = HologramContainer::read_from_file(&target_path).expect("Failed to read container");
    assert_eq!(loaded.total_frames, 20);
    assert_eq!(loaded.playlist.len(), 3);
    assert_eq!(loaded.frames.len(), 20);
    for frame in &loaded.frames {
        assert_eq!(frame.len(), HOLOGRAM_FRAME_SIZE);
    }
}
