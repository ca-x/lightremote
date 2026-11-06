//! Display-local wire coordinates. The host adds the display's global offset once.
use anyhow::{Context, Result, ensure};
use serde_json::Value;

/// Match the host's desktop cap without trusting the peer's display metadata.
pub const MAX_FRAME_EDGE: u32 = 16384;
pub const MAX_FRAME_PIXELS: u64 = 16_777_216;

/// Header-only admission before a JPEG reaches GDK, AppKit or the Windows decoder.
/// Encoded record bounds alone do not bound the decoder's raster allocation.
pub fn admit_jpeg(bytes: &[u8]) -> Result<(u32, u32)> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= lightremote_core::records::MAX_RECORD,
        "Desktop JPEG exceeds encoded record limit"
    );
    ensure!(
        bytes.starts_with(&[0xff, 0xd8]),
        "Host sent a non-JPEG desktop record"
    );
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_FRAME_EDGE);
    limits.max_image_height = Some(MAX_FRAME_EDGE);
    reader.limits(limits);
    // into_dimensions decodes JPEG headers, never the pixel raster.
    let (width, height) = reader
        .into_dimensions()
        .context("Invalid desktop JPEG header or dimensions")?;
    ensure!(
        width > 0 && height > 0 && width <= MAX_FRAME_EDGE && height <= MAX_FRAME_EDGE,
        "Desktop JPEG dimensions out of bounds"
    );
    ensure!(
        u64::from(width) * u64::from(height) <= MAX_FRAME_PIXELS,
        "Desktop JPEG pixel limit exceeded"
    );
    Ok((width, height))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Display {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
}

impl Display {
    pub fn from_response(response: &Value, id: u32) -> Result<Self> {
        let entry = response["displays"]
            .as_array()
            .context("Host omitted display list")?
            .iter()
            .find(|entry| entry["id"].as_u64() == Some(u64::from(id)))
            .context("Selected display is unavailable")?;
        let dimension = |name| -> Result<u32> {
            Ok(u32::try_from(
                entry[name].as_u64().context("Invalid display dimensions")?,
            )?)
        };
        let coordinate = |name| -> Result<i32> {
            Ok(i32::try_from(
                entry[name].as_i64().context("Invalid display offset")?,
            )?)
        };
        let display = Self {
            id,
            width: dimension("width")?,
            height: dimension("height")?,
            x: coordinate("x")?,
            y: coordinate("y")?,
        };
        ensure!(
            display.width > 0
                && display.height > 0
                && display.width <= 16384
                && display.height <= 16384,
            "Invalid display size"
        );
        Ok(display)
    }

    /// Map a contained image, excluding letterboxing. `x/y` are in local widget points,
    /// independent of OS DPI. Wire coordinates stay display-local; the input service
    /// translates them by this display's (possibly negative) x/y offset.
    pub fn map(self, x: f64, y: f64, width: f64, height: f64) -> Option<(i32, i32)> {
        if ![x, y, width, height].iter().all(|n| n.is_finite()) || width <= 0.0 || height <= 0.0 {
            return None;
        }
        let scale = (width / f64::from(self.width)).min(height / f64::from(self.height));
        let left = ((width - f64::from(self.width) * scale) / 2.0).max(0.0);
        let top = ((height - f64::from(self.height) * scale) / 2.0).max(0.0);
        if x < left
            || y < top
            || x >= (left + f64::from(self.width) * scale).min(width)
            || y >= (top + f64::from(self.height) * scale).min(height)
        {
            return None;
        }
        let x = (x - left) / scale;
        let y = (y - top) / scale;
        if x < 0.0 || y < 0.0 || x >= f64::from(self.width) || y >= f64::from(self.height) {
            return None;
        }
        let (x, y) = (
            x.round().clamp(0.0, f64::from(self.width - 1)) as i32,
            y.round().clamp(0.0, f64::from(self.height - 1)) as i32,
        );
        self.x.checked_add(x)?;
        self.y.checked_add(y)?;
        Some((x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn jpeg_with_dimensions(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode(&[0, 0, 0], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let sof = bytes
            .windows(2)
            .position(|pair| pair == [0xff, 0xc0])
            .expect("baseline JPEG SOF");
        bytes[sof + 5..sof + 7].copy_from_slice(&height.to_be_bytes());
        bytes[sof + 7..sof + 9].copy_from_slice(&width.to_be_bytes());
        bytes
    }
    #[test]
    fn jpeg_admission_rejects_small_encoded_pixel_bombs_before_platform_decode() {
        let bytes = jpeg_with_dimensions(16384, 16384);
        assert!(bytes.len() < 1024);
        assert!(
            admit_jpeg(&bytes)
                .unwrap_err()
                .to_string()
                .contains("pixel limit")
        );
        assert!(admit_jpeg(&jpeg_with_dimensions(4096, 4097)).is_err());
        assert!(admit_jpeg(&jpeg_with_dimensions(16385, 1)).is_err());
        assert!(admit_jpeg(&jpeg_with_dimensions(0, 1)).is_err());
    }
    #[test]
    fn jpeg_admission_accepts_pixel_boundary_and_rejects_bad_or_oversized_records() {
        assert_eq!(admit_jpeg(&jpeg_with_dimensions(1, 1)).unwrap(), (1, 1));
        assert_eq!(
            admit_jpeg(&jpeg_with_dimensions(4096, 4096)).unwrap(),
            (4096, 4096)
        );
        assert!(admit_jpeg(&[]).is_err());
        assert!(admit_jpeg(&[0xff, 0xd8, 0xff, 0xd9]).is_err());
        assert!(admit_jpeg(&vec![0; lightremote_core::records::MAX_RECORD + 1]).is_err());
    }
    #[test]
    fn scaled_letterbox_maps_to_selected_display_without_double_offset() {
        let d = Display {
            id: 2,
            width: 1920,
            height: 1080,
            x: -1920,
            y: 120,
        };
        assert_eq!(d.map(500.0, 300.0, 1000.0, 600.0), Some((960, 540)));
        assert_eq!(d.map(0.0, 0.0, 1000.0, 600.0), None);
        assert_eq!(d.map(1000.0, 300.0, 1000.0, 600.0), None);
        assert_eq!(d.map(f64::NAN, 0.0, 1000.0, 600.0), None);
        assert_eq!((d.x + 960, d.y + 540), (-960, 660));
        assert_eq!(d.map(100.0, 100.0, 0.0, 600.0), None);
    }
    #[test]
    fn display_metadata_requires_an_exact_id_and_valid_geometry() {
        let response = json!({"displays":[{"id":7,"width":800,"height":600,"x":-800,"y":0}]});
        assert!(Display::from_response(&response, 0).is_err());
        assert_eq!(Display::from_response(&response, 7).unwrap().x, -800);
        assert!(
            Display::from_response(
                &json!({"displays":[{"id":0,"width":0,"height":600,"x":0,"y":0}]}),
                0
            )
            .is_err()
        );
    }
}
