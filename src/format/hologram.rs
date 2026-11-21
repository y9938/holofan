//! Binary container format (hologram.bin) for holographic fan SD cards.
//!
//! Header layout (4096 bytes / 0x1000):
//! - 0..8:     Header magic signature `00 11 22 33 44 55 66 77`
//! - 8..10:    Total frame count (u16 Big-Endian)
//! - 10..16:   6 reserved zero bytes
//! - 16..3016: Playlist table of 500 entries (6 bytes each):
//!   - start_frame (u16 BE, 1-based)
//!   - end_frame   (u16 BE, 1-based)
//!   - speed       (u8)
//!   - mode        (u8)
//! - 3016..3024: 8 reserved zero bytes
//! - 3024..3032: Playlist trailer signature `77 66 55 44 33 22 11 00`
//! - 3032..4096: Zero padding to offset 0x1000 (1064 bytes)
//! - 4096..:     Array of frames (114,688 bytes each: 113,400 bytes data + 1,288 zero padding).

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use byteorder::{BigEndian, ByteOrder};

use crate::polar::PolarConfig;

/// Header magic signature.
pub const HOLOGRAM_HEADER_MAGIC: [u8; 8] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77];
/// Playlist trailer signature.
pub const HOLOGRAM_TRAILER_MAGIC: [u8; 8] = [0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00];

/// Header block size aligned to boundary.
pub const HOLOGRAM_HEADER_SIZE: usize = 4096;
/// Maximum number of playlist entries in header.
pub const MAX_PLAYLIST_ENTRIES: usize = 500;
pub const MAX_SUPPORTED_FRAMES: usize = u16::MAX as usize;
/// Size of a single playlist entry in bytes.
pub const PLAYLIST_ENTRY_SIZE: usize = 6;
/// Size of one padded frame in bytes.
pub const HOLOGRAM_FRAME_SIZE: usize = PolarConfig::FRAME_PADDED_SIZE;

/// Fan playlist entry specifying frame range and playback mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistEntry {
    /// 1-based start frame index.
    pub start_frame: u16,
    /// 1-based end frame index (inclusive).
    pub end_frame: u16,
    /// Playback speed / duration parameter.
    pub speed: u8,
    /// Display mode flags (looping, transition effects).
    pub mode: u8,
}

impl PlaylistEntry {
    /// Checks whether the entry is an empty/unpopulated slot.
    pub fn is_empty(&self) -> bool {
        self.start_frame == 0 && self.end_frame == 0 && self.speed == 0 && self.mode == 0
    }
}

/// Standard binary container structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HologramContainer {
    /// Total number of polar frames.
    pub total_frames: u16,
    /// Active playlist entries.
    pub playlist: Vec<PlaylistEntry>,
    /// Array of raw polar frames (114,688 bytes each).
    pub frames: Vec<Vec<u8>>,
}

impl HologramContainer {
    /// Creates a new container from raw polar frames and playlist entries.
    pub fn new(frames: Vec<Vec<u8>>, playlist: Vec<PlaylistEntry>) -> Self {
        let total_frames = frames.len() as u16;
        Self {
            total_frames,
            playlist,
            frames,
        }
    }

    /// Parses container from raw binary data.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < HOLOGRAM_HEADER_SIZE {
            bail!(
                "File size ({}) is smaller than minimum header size ({})",
                data.len(),
                HOLOGRAM_HEADER_SIZE
            );
        }

        // Validate header signature
        if data[0..8] != HOLOGRAM_HEADER_MAGIC {
            bail!(
                "Invalid header magic signature: expected {:02x?}, got {:02x?}",
                HOLOGRAM_HEADER_MAGIC,
                &data[0..8]
            );
        }

        let total_frames = BigEndian::read_u16(&data[8..10]);

        // Validate playlist trailer signature at offset 3024..3032
        if data[3024..3032] != HOLOGRAM_TRAILER_MAGIC {
            bail!(
                "Invalid playlist trailer signature: expected {:02x?}, got {:02x?}",
                HOLOGRAM_TRAILER_MAGIC,
                &data[3024..3032]
            );
        }

        // Parse up to 500 playlist entries from offset 16..3016
        let mut playlist = Vec::new();
        for i in 0..MAX_PLAYLIST_ENTRIES {
            let offset = 16 + i * PLAYLIST_ENTRY_SIZE;
            let start_frame = BigEndian::read_u16(&data[offset..offset + 2]);
            let end_frame = BigEndian::read_u16(&data[offset + 2..offset + 4]);
            let speed = data[offset + 4];
            let mode = data[offset + 5];

            let entry = PlaylistEntry {
                start_frame,
                end_frame,
                speed,
                mode,
            };

            if !entry.is_empty() {
                playlist.push(entry);
            }
        }

        // Validate payload length against expected frame count
        let payload_len = data.len() - HOLOGRAM_HEADER_SIZE;
        let expected_payload_len = (total_frames as usize) * HOLOGRAM_FRAME_SIZE;

        if payload_len != expected_payload_len {
            bail!(
                "Invalid payload size: expected exactly {} bytes for {} frames, found {}",
                expected_payload_len,
                total_frames,
                payload_len
            );
        }

        // Extract individual polar frames
        let mut frames = Vec::with_capacity(total_frames as usize);
        for frame_idx in 0..(total_frames as usize) {
            let start = HOLOGRAM_HEADER_SIZE + frame_idx * HOLOGRAM_FRAME_SIZE;
            let end = start + HOLOGRAM_FRAME_SIZE;
            let frame = &data[start..end];
            if frame[PolarConfig::ACTIVE_FRAME_SIZE..]
                .iter()
                .any(|byte| *byte != 0)
            {
                bail!(
                    "Frame {} has nonzero bytes in its padding area",
                    frame_idx + 1
                );
            }
            frames.push(frame.to_vec());
        }

        validate_playlist(&playlist, total_frames as usize)?;

        Ok(Self {
            total_frames,
            playlist,
            frames,
        })
    }

    /// Serializes container into a binary stream.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if self.frames.len() > MAX_SUPPORTED_FRAMES {
            bail!(
                "Container has {} frames; the header supports at most {}",
                self.frames.len(),
                MAX_SUPPORTED_FRAMES
            );
        }
        if self.playlist.len() > MAX_PLAYLIST_ENTRIES {
            bail!(
                "Container has {} playlist entries; the header supports at most {}",
                self.playlist.len(),
                MAX_PLAYLIST_ENTRIES
            );
        }
        if let Some((idx, frame)) = self
            .frames
            .iter()
            .enumerate()
            .find(|(_, frame)| frame.len() != HOLOGRAM_FRAME_SIZE)
        {
            bail!(
                "Frame {} has {} bytes; expected exactly {}",
                idx + 1,
                frame.len(),
                HOLOGRAM_FRAME_SIZE
            );
        }
        if let Some((idx, _)) = self.frames.iter().enumerate().find(|(_, frame)| {
            frame[PolarConfig::ACTIVE_FRAME_SIZE..]
                .iter()
                .any(|byte| *byte != 0)
        }) {
            bail!("Frame {} has nonzero bytes in its padding area", idx + 1);
        }
        validate_playlist(&self.playlist, self.frames.len())?;

        let total_size = HOLOGRAM_HEADER_SIZE + self.frames.len() * HOLOGRAM_FRAME_SIZE;
        let mut out = vec![0u8; total_size];

        // Header magic signature
        out[0..8].copy_from_slice(&HOLOGRAM_HEADER_MAGIC);

        // Total frame count in Big-Endian
        let total_frames = self.frames.len() as u16;
        BigEndian::write_u16(&mut out[8..10], total_frames);

        // Playlist table entries
        for (i, entry) in self.playlist.iter().enumerate() {
            let offset = 16 + i * PLAYLIST_ENTRY_SIZE;
            BigEndian::write_u16(&mut out[offset..offset + 2], entry.start_frame);
            BigEndian::write_u16(&mut out[offset + 2..offset + 4], entry.end_frame);
            out[offset + 4] = entry.speed;
            out[offset + 5] = entry.mode;
        }

        // Playlist trailer signature
        out[3024..3032].copy_from_slice(&HOLOGRAM_TRAILER_MAGIC);

        // Serialized polar frames
        for (frame_idx, frame) in self.frames.iter().enumerate() {
            let start = HOLOGRAM_HEADER_SIZE + frame_idx * HOLOGRAM_FRAME_SIZE;
            out[start..start + HOLOGRAM_FRAME_SIZE].copy_from_slice(frame);
        }

        Ok(out)
    }

    /// Reads binary container from a file path.
    pub fn read_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let mut file =
            File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .with_context(|| format!("Failed to read file: {}", path.display()))?;
        Self::parse(&buffer)
    }

    /// Writes binary container to a file path.
    pub fn write_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();
        let bytes = self.to_bytes()?;
        let mut file = File::create(path)
            .with_context(|| format!("Failed to create file: {}", path.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("Failed to write to file: {}", path.display()))?;
        Ok(())
    }
}

fn validate_playlist(playlist: &[PlaylistEntry], total_frames: usize) -> Result<()> {
    for (idx, entry) in playlist.iter().enumerate() {
        let start = entry.start_frame as usize;
        let end = entry.end_frame as usize;
        if start == 0 || end < start || end > total_frames {
            bail!(
                "Playlist entry {} references frames {}..{} but the container has {} frames",
                idx + 1,
                entry.start_frame,
                entry.end_frame,
                total_frames
            );
        }
    }
    Ok(())
}
