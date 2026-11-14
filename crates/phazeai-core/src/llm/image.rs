//! Pictures sent to a vision model with a chat message.
//!
//! A screenshot of a broken layout or an error dialog says more than a
//! paragraph describing it. [`ImageInput`] holds one picture as the base64
//! text every provider takes (Anthropic, OpenAI-compatible servers and
//! Ollama), with its media type read from the file's own first bytes rather
//! than trusted from its name.
//!
//! Each provider takes PNG, JPEG, GIF and WebP. Anthropic refuses a picture
//! over 5 MB, so that is the cap here for every provider: one rule, and a
//! picture that big costs the same tokens as a smaller copy anyway, since
//! the model sees it scaled down.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The largest picture accepted, in bytes (Anthropic's per-image limit).
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// The most pictures sent with one message, so a glob or a pasted list of
/// paths can't send a whole folder of screenshots.
pub const MAX_IMAGES_PER_MESSAGE: usize = 8;

/// What a picture costs when its size can't be read: the most Anthropic
/// charges for one picture (a 1568 px edge, about 1.15 megapixels).
const FALLBACK_IMAGE_TOKENS: usize = 1600;

/// The longest edge a vision model sees; larger pictures are scaled down to
/// it before they're counted.
const MAX_EDGE_PX: u64 = 1568;

/// One picture sent with a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInput {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub media_type: String,
    /// The file's bytes, base64 encoded (standard alphabet, padded).
    pub data: String,
    /// Width and height in pixels, when the header could be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<(u32, u32)>,
}

impl ImageInput {
    /// A picture from its bytes, refused when it isn't PNG, JPEG, GIF or
    /// WebP, or is over [`MAX_IMAGE_BYTES`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("the file is empty".to_string());
        }
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(format!(
                "it is {} and the most a model takes is {}",
                human_size(bytes.len()),
                human_size(MAX_IMAGE_BYTES)
            ));
        }
        let media_type = sniff_media_type(bytes)
            .ok_or_else(|| "it isn't a PNG, JPEG, GIF or WebP picture".to_string())?;
        Ok(Self {
            media_type: media_type.to_string(),
            data: base64_encode(bytes),
            size: pixel_size(bytes),
        })
    }

    /// A picture read from `path`.
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.len() > MAX_IMAGE_BYTES as u64 {
            return Err(format!(
                "it is {} and the most a model takes is {}",
                human_size(meta.len() as usize),
                human_size(MAX_IMAGE_BYTES)
            ));
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        Self::from_bytes(&bytes)
    }

    /// The `data:` URL OpenAI-compatible servers take.
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.data)
    }

    /// Rough input tokens this picture costs: Anthropic's published
    /// `width * height / 750` after scaling the longest edge to 1568 px.
    /// OpenAI's tile count lands in the same range, so one estimate serves
    /// the context budget for every provider.
    pub fn estimated_tokens(&self) -> usize {
        let Some((w, h)) = self.size else {
            return FALLBACK_IMAGE_TOKENS;
        };
        let (w, h) = (u64::from(w.max(1)), u64::from(h.max(1)));
        let long = w.max(h);
        let (w, h) = if long > MAX_EDGE_PX {
            (w * MAX_EDGE_PX / long, h * MAX_EDGE_PX / long)
        } else {
            (w, h)
        };
        ((w * h).div_ceil(750) as usize).clamp(1, FALLBACK_IMAGE_TOKENS)
    }

    /// A short label for logs and transcripts, such as `PNG 1280x720`.
    pub fn label(&self) -> String {
        let kind = self
            .media_type
            .strip_prefix("image/")
            .unwrap_or(&self.media_type)
            .to_ascii_uppercase();
        match self.size {
            Some((w, h)) => format!("{kind} {w}x{h}"),
            None => kind,
        }
    }
}

/// Whether `path` is named like a picture a model can take, judged by its
/// extension (the bytes are checked again when it's read).
pub fn is_image_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp"
        )
    })
}

/// The media type of `bytes`, from the signature at the start of the file.
pub fn sniff_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Width and height in pixels, read from the picture's header.
pub fn pixel_size(bytes: &[u8]) -> Option<(u32, u32)> {
    match sniff_media_type(bytes)? {
        "image/png" => {
            // The IHDR chunk comes first: its width and height are big-endian
            // u32s right after the 8-byte signature and the chunk's length and
            // type.
            (bytes.get(12..16)? == b"IHDR").then_some(())?;
            Some((be_u32(bytes, 16)?, be_u32(bytes, 20)?))
        }
        "image/gif" => Some((u32::from(le_u16(bytes, 6)?), u32::from(le_u16(bytes, 8)?))),
        "image/jpeg" => jpeg_size(bytes),
        "image/webp" => webp_size(bytes),
        _ => None,
    }
}

/// A JPEG's size, from its first start-of-frame segment.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        // Fill bytes and markers that carry no length.
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let len = usize::from(be_u16(bytes, i + 2)?);
        // SOF0..SOF15, leaving out DHT (C4), JPG (C8) and DAC (CC).
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let h = be_u16(bytes, i + 5)?;
            let w = be_u16(bytes, i + 7)?;
            return Some((u32::from(w), u32::from(h)));
        }
        if len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

/// A WebP's size, from its first chunk (lossy `VP8 `, lossless `VP8L` or
/// extended `VP8X`).
fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    match bytes.get(12..16)? {
        b"VP8 " => {
            // The frame tag (3 bytes) and start code (9D 01 2A) come before
            // two little-endian 14-bit dimensions.
            (bytes.get(23..26)? == [0x9D, 0x01, 0x2A]).then_some(())?;
            let w = le_u16(bytes, 26)? & 0x3FFF;
            let h = le_u16(bytes, 28)? & 0x3FFF;
            Some((u32::from(w), u32::from(h)))
        }
        b"VP8L" => {
            (*bytes.get(20)? == 0x2F).then_some(())?;
            let b = bytes.get(21..25)?;
            let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => {
            let b = bytes.get(24..30)?;
            let w = u32::from_le_bytes([b[0], b[1], b[2], 0]) + 1;
            let h = u32::from_le_bytes([b[3], b[4], b[5], 0]) + 1;
            Some((w, h))
        }
        _ => None,
    }
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let b = bytes.get(at..at + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let b = bytes.get(at..at + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// Standard base64 (RFC 4648, with padding), as every provider takes it.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn human_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A real 1x1 PNG.
    pub(crate) const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut b = PNG_1X1[..16].to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&PNG_1X1[24..]);
        b
    }

    #[test]
    fn base64_matches_rfc_4648_test_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(input.as_bytes()), want, "{input:?}");
        }
        assert_eq!(base64_encode(&[0xFF, 0xFE, 0xFD]), "//79");
    }

    #[test]
    fn the_media_type_comes_from_the_bytes_not_the_name() {
        assert_eq!(sniff_media_type(PNG_1X1), Some("image/png"));
        assert_eq!(
            sniff_media_type(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(
            sniff_media_type(b"GIF89a\x02\x00\x03\x00"),
            Some("image/gif")
        );
        assert_eq!(
            sniff_media_type(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(sniff_media_type(b"%PDF-1.7"), None);
        assert_eq!(sniff_media_type(b"fn main() {}"), None);
    }

    #[test]
    fn sizes_are_read_from_each_format_header() {
        assert_eq!(pixel_size(PNG_1X1), Some((1, 1)));
        assert_eq!(pixel_size(&png_header(1920, 1080)), Some((1920, 1080)));
        assert_eq!(pixel_size(b"GIF89a\x40\x01\xF0\x00"), Some((320, 240)));

        // JPEG: SOI, an APP0 segment, then SOF0 with height 480, width 640.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0xE0, 0x02, 0x80, 0x03]);
        assert_eq!(pixel_size(&jpeg), Some((640, 480)));

        // WebP lossy, lossless and extended.
        let mut vp8 = b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0\x9D\x01\x2A".to_vec();
        vp8.extend_from_slice(&800u16.to_le_bytes());
        vp8.extend_from_slice(&600u16.to_le_bytes());
        assert_eq!(pixel_size(&vp8), Some((800, 600)));

        let bits: u32 = (100 - 1) | ((50 - 1) << 14);
        let mut vp8l = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2F".to_vec();
        vp8l.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(pixel_size(&vp8l), Some((100, 50)));

        let mut vp8x = b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0\0\0\0\0".to_vec();
        vp8x.extend_from_slice(&(3000u32 - 1).to_le_bytes()[..3]);
        vp8x.extend_from_slice(&(2000u32 - 1).to_le_bytes()[..3]);
        assert_eq!(pixel_size(&vp8x), Some((3000, 2000)));
    }

    #[test]
    fn a_real_png_becomes_a_base64_image() {
        let img = ImageInput::from_bytes(PNG_1X1).unwrap();
        assert_eq!(img.media_type, "image/png");
        assert_eq!(img.size, Some((1, 1)));
        assert!(img.data.starts_with("iVBORw0KGgo"), "{}", img.data);
        assert!(img
            .data_url()
            .starts_with("data:image/png;base64,iVBORw0KGgo"));
        assert_eq!(img.label(), "PNG 1x1");
    }

    #[test]
    fn non_pictures_empty_files_and_huge_files_are_refused() {
        assert!(ImageInput::from_bytes(b"").unwrap_err().contains("empty"));
        assert!(ImageInput::from_bytes(b"hello")
            .unwrap_err()
            .contains("isn't a PNG"));
        let mut big = PNG_1X1.to_vec();
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        assert!(ImageInput::from_bytes(&big).unwrap_err().contains("5.0 MB"));
    }

    #[test]
    fn a_file_on_disk_is_read_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("shot.png");
        std::fs::write(&png, PNG_1X1).unwrap();
        assert_eq!(ImageInput::from_path(&png).unwrap().media_type, "image/png");

        // A text file named like a picture is still refused.
        let fake = dir.path().join("notes.png");
        std::fs::write(&fake, "not a picture").unwrap();
        assert!(ImageInput::from_path(&fake).is_err());
        assert!(ImageInput::from_path(&dir.path().join("missing.png")).is_err());
    }

    #[test]
    fn token_estimates_follow_the_scaled_pixel_count() {
        let small = ImageInput::from_bytes(&png_header(200, 200)).unwrap();
        assert_eq!(small.estimated_tokens(), 54); // 40000 / 750, rounded up

        // 1000x500 is left as it is.
        let mid = ImageInput::from_bytes(&png_header(1000, 500)).unwrap();
        assert_eq!(mid.estimated_tokens(), 667);

        // A 4K screenshot is scaled down and costs the most a picture can.
        let uhd = ImageInput::from_bytes(&png_header(3840, 2160)).unwrap();
        assert_eq!(uhd.estimated_tokens(), 1600);

        let unknown = ImageInput {
            media_type: "image/png".into(),
            data: String::new(),
            size: None,
        };
        assert_eq!(unknown.estimated_tokens(), 1600);
    }

    #[test]
    fn picture_names_are_recognised_by_extension() {
        for name in ["a.png", "b.JPG", "c.jpeg", "d.gif", "e.WebP"] {
            assert!(is_image_path(Path::new(name)), "{name}");
        }
        for name in ["a.svg", "b.rs", "c", "d.png.txt"] {
            assert!(!is_image_path(Path::new(name)), "{name}");
        }
    }
}
