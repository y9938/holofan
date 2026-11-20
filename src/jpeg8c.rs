//! IJG 8c JPEG decoding used for byte-compatible factory imports.

use anyhow::{anyhow, bail, Context, Result};
use image::RgbImage;
use std::ffi::{c_char, c_int, c_ulong, c_void, CStr};
use std::path::Path;

unsafe extern "C" {
    fn hf_jpeg8c_decode_rgb(
        input: *const u8,
        input_len: usize,
        pixels: *mut *mut u8,
        width: *mut u32,
        height: *mut u32,
        error: *mut c_char,
        error_cap: usize,
        warning: *mut c_char,
        warning_cap: usize,
    ) -> c_int;
    fn hf_jpeg8c_free(pixels: *mut c_void);
}

/// Decodes a JPEG byte stream to interleaved RGB using the bundled IJG 8c code.
pub fn decode_rgb(bytes: &[u8]) -> Result<RgbImage> {
    c_ulong::try_from(bytes.len()).context("JPEG input is too large for IJG 8c")?;

    let mut pixels = std::ptr::null_mut();
    let mut width = 0;
    let mut height = 0;
    let mut error = [0 as c_char; 512];
    let mut warning = [0 as c_char; 512];
    // The C wrapper owns the IJG decompressor and returns its output allocation.
    let ok = unsafe {
        hf_jpeg8c_decode_rgb(
            bytes.as_ptr(),
            bytes.len(),
            &mut pixels,
            &mut width,
            &mut height,
            error.as_mut_ptr(),
            error.len(),
            warning.as_mut_ptr(),
            warning.len(),
        )
    };
    if ok == 0 {
        let message = unsafe { CStr::from_ptr(error.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        bail!("IJG 8c JPEG decode failed: {message}");
    }

    let allocation = JpegPixels(pixels);
    if warning[0] != 0 {
        let message = unsafe { CStr::from_ptr(warning.as_ptr()) }.to_string_lossy();
        bail!("IJG 8c reported a JPEG warning: {message}");
    }
    let byte_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|size| size.checked_mul(3))
        .ok_or_else(|| anyhow!("IJG 8c returned invalid JPEG dimensions"))?;
    if allocation.0.is_null() || byte_len == 0 {
        bail!("IJG 8c returned an empty JPEG image");
    }
    let raw = unsafe { std::slice::from_raw_parts(allocation.0, byte_len) }.to_vec();
    RgbImage::from_raw(width, height, raw)
        .ok_or_else(|| anyhow!("IJG 8c returned an inconsistent JPEG raster"))
}

pub fn open(path: &Path) -> Result<RgbImage> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read JPEG: {}", path.display()))?;
    decode_rgb(&bytes).with_context(|| format!("Failed to decode JPEG: {}", path.display()))
}

struct JpegPixels(*mut u8);

impl Drop for JpegPixels {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { hf_jpeg8c_free(self.0.cast()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::ExtendedColorType;

    #[test]
    fn decodes_a_valid_jpeg_to_rgb() {
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, 95)
            .encode(&[120u8; 16 * 16 * 3], 16, 16, ExtendedColorType::Rgb8)
            .unwrap();

        let decoded = decode_rgb(&encoded).unwrap();
        assert_eq!(decoded.dimensions(), (16, 16));
        assert!(decoded
            .pixels()
            .all(|pixel| pixel[0] > 110 && pixel[0] < 130));
    }

    #[test]
    fn rejects_empty_and_truncated_jpeg_data() {
        assert!(decode_rgb(&[]).is_err());
        assert!(decode_rgb(&[0xff, 0xd8]).is_err());
    }
}
