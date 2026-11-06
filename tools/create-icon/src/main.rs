//! Generates the app, favicon and Windows icons from the approved Rising knot artwork.
//!
//! Run from the repository root with `cargo run -p create-icon`.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::imageops::{self, FilterType};
use image::{ExtendedColorType, GrayImage, Luma, RgbaImage};

const SIZES: [u32; 14] = [16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 96, 128, 256];
const SOURCE: &str = "src-tauri/icons/rising-knot.png";
const ICON_DIRECTORY: &str = "src-tauri/icons";
const PUBLIC_DIRECTORY: &str = "ui/public";

/// True inside a superellipse, which gives the tile continuous corners at every size.
fn inside_tile(x: f64, y: f64, half: f64) -> bool {
    ((x - half).abs() / half).powi(4) + ((y - half).abs() / half).powi(4) <= 1.0
}

/// Cuts the corners at twice the final resolution, then filters down for a smooth edge.
fn mask_tile(artwork: &mut RgbaImage) {
    let edge = artwork.width() * 2;
    let half = f64::from(edge - 1) / 2.0;
    let mask = GrayImage::from_fn(edge, edge, |x, y| {
        Luma([if inside_tile(f64::from(x), f64::from(y), half) {
            255
        } else {
            0
        }])
    });
    let mask = imageops::resize(
        &mask,
        artwork.width(),
        artwork.height(),
        FilterType::Lanczos3,
    );
    for (pixel, alpha) in artwork.pixels_mut().zip(mask.pixels()) {
        pixel[3] = alpha[0];
    }
}

/// Resizes and removes the near-transparent ringing the filter adds.
fn scaled(artwork: &RgbaImage, size: u32) -> RgbaImage {
    let mut frame = imageops::resize(artwork, size, size, FilterType::Lanczos3);
    for pixel in frame.pixels_mut().filter(|pixel| pixel[3] < 4) {
        pixel[3] = 0;
    }
    frame
}

fn png(image: &RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .expect("encoding a PNG in memory");
    bytes
}

fn ico(frames: &[RgbaImage]) -> Vec<u8> {
    let frames: Vec<_> = frames
        .iter()
        .map(|frame| {
            let (width, height) = frame.dimensions();
            IcoFrame::as_png(frame.as_raw(), width, height, ExtendedColorType::Rgba8)
                .expect("encoding an icon frame")
        })
        .collect();
    let mut bytes = Vec::new();
    IcoEncoder::new(&mut bytes)
        .encode_images(&frames)
        .expect("encoding the icon");
    bytes
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk
            .iter()
            .fold(0_u32, |group, byte| (group << 8) | u32::from(*byte))
            << (8 * (3 - chunk.len()));
        for index in 0..4 {
            if index <= chunk.len() {
                text.push(char::from(
                    ALPHABET[(group >> (18 - 6 * index)) as usize & 63],
                ));
            } else {
                text.push('=');
            }
        }
    }
    text
}

/// Checks every frame survives the ICO round trip with its corners cut.
fn verify(path: &Path) {
    let bytes = fs::read(path).expect("reading the written icon");
    verify_bytes(&bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}

/// Decodes every ICO payload and checks its dimensions and transparency.
fn verify_bytes(bytes: &[u8]) -> Result<(), String> {
    let invalid = || "Invalid or truncated ICO data.".to_owned();
    let header = bytes.get(..6).ok_or_else(invalid)?;
    if header[..4] != [0, 0, 1, 0]
        || usize::from(u16::from_le_bytes([header[4], header[5]])) != SIZES.len()
    {
        return Err(invalid());
    }
    let table_end = 6 + 16 * SIZES.len();
    let entries = bytes.get(6..table_end).ok_or_else(invalid)?;
    for (entry, size) in entries.as_chunks::<16>().0.iter().zip(SIZES) {
        let length = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        let end = offset.checked_add(length).ok_or_else(invalid)?;
        let dimension = if entry[0] == 0 {
            256
        } else {
            u32::from(entry[0])
        };
        if offset < table_end || dimension != size || entry[0] != entry[1] {
            return Err(invalid());
        }
        let payload = bytes.get(offset..end).ok_or_else(invalid)?;
        let frame = image::load_from_memory_with_format(payload, image::ImageFormat::Png)
            .map_err(|error| error.to_string())?
            .to_rgba8();
        if frame.dimensions() != (size, size)
            || frame.get_pixel(0, 0)[3] != 0
            || frame.get_pixel(size / 2, size / 2)[3] != 255
        {
            return Err("Icon dimensions or transparency did not survive encoding.".into());
        }
    }
    Ok(())
}

/// Writes matching app icons from the approved artwork and verifies the encoded ICOs.
fn main() {
    let mut artwork = image::open(SOURCE)
        .expect("reading the logo source")
        .to_rgba8();
    assert_eq!(
        artwork.width(),
        artwork.height(),
        "the logo source must be square"
    );
    mask_tile(&mut artwork);
    let frames = SIZES.map(|size| scaled(&artwork, size));
    let large = png(&scaled(&artwork, 512));
    let icon = ico(&frames);

    for (directory, stem) in [(ICON_DIRECTORY, "icon"), (PUBLIC_DIRECTORY, "ethos")] {
        let directory = Path::new(directory);
        fs::write(directory.join(format!("{stem}.png")), &large).expect("writing the PNG");
        let path = directory.join(format!("{stem}.ico"));
        fs::write(&path, &icon).expect("writing the icon");
        verify(&path);
    }

    // Embed the approved raster so SVG consumers use the exact same mark.
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 512 512\">\n  \
         <title>ethos Rising knot</title>\n  \
         <image width=\"512\" height=\"512\" href=\"data:image/png;base64,{}\"/>\n</svg>\n",
        base64(&large)
    );
    fs::write(Path::new(PUBLIC_DIRECTORY).join("ethos.svg"), svg).expect("writing the SVG");
    println!(
        "PASS: 2 ICO files, {} verified frames each; matching PNGs and SVG",
        SIZES.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn tile_keeps_the_center_and_drops_the_corners() {
        assert!(inside_tile(50.0, 50.0, 50.0));
        assert!(inside_tile(50.0, 1.0, 50.0));
        assert!(!inside_tile(2.0, 2.0, 50.0));
    }

    #[test]
    fn encoded_frames_are_decoded_and_checked() {
        let mut artwork = RgbaImage::from_pixel(256, 256, image::Rgba([30, 60, 90, 255]));
        mask_tile(&mut artwork);
        let frames = SIZES.map(|size| scaled(&artwork, size));
        let bytes = ico(&frames);
        verify_bytes(&bytes).unwrap();
        assert!(verify_bytes(&bytes[..5]).is_err());
        assert!(verify_bytes(&bytes[..bytes.len() - 20]).is_err());
        let mut corrupt = bytes.clone();
        let offset = u32::from_le_bytes(corrupt[18..22].try_into().unwrap()) as usize;
        corrupt[offset] = 0;
        assert!(verify_bytes(&corrupt).is_err());
        let mut wrong_dimensions = bytes;
        wrong_dimensions[6] = 24;
        assert!(verify_bytes(&wrong_dimensions).is_err());
        let opaque =
            SIZES.map(|size| RgbaImage::from_pixel(size, size, image::Rgba([30, 60, 90, 255])));
        assert!(verify_bytes(&ico(&opaque)).is_err());
    }
}
