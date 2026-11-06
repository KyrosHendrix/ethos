//! Tools that work with pictures: images to PDF, PDF to images, smaller copies, redaction.

use std::io::Cursor;

use ethos_core::Rect;
use image::codecs::jpeg::JpegEncoder;
use image::{
    DynamicImage, ExtendedColorType, GenericImageView, ImageFormat, ImageReader, RgbImage,
};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use pdfium_render::prelude::*;

use crate::pages::{describe, serialize};

const IMAGE_COMPRESS_QUALITY: u8 = 72;
/// Pictures above this resolution are scaled down in a smaller copy.
const TARGET_PPI: f64 = 150.0;
/// Redacted pages are rebuilt from pixels at this resolution.
const REDACT_DPI: f32 = 200.0;
const POINTS_PER_INCH: f32 = 72.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageFit {
    /// Each page takes the size of its picture.
    Original,
    /// Each picture is centered on an A4 page with a margin.
    A4,
}

fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|_| "One of these files is not a picture ethos can read.".to_owned())
}

fn flate(content: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(content)
        .map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())
}

/// Adds `picture` as an image object. JPEG files keep their original bytes.
fn add_picture(document: &mut Document, bytes: &[u8]) -> Result<(ObjectId, u32, u32), String> {
    let format = image::guess_format(bytes).ok();
    let picture = decode(bytes)?;
    let (width, height) = picture.dimensions();
    let base = |space: &str, filter: &str| {
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => i64::from(width),
            "Height" => i64::from(height),
            "ColorSpace" => space,
            "BitsPerComponent" => 8,
            "Filter" => filter,
        }
    };
    let is_plain_jpeg = format == Some(ImageFormat::Jpeg) && picture.color().channel_count() == 3;
    let object = if is_plain_jpeg {
        Stream::new(base("DeviceRGB", "DCTDecode"), bytes.to_vec()).with_compression(false)
    } else {
        let mut node = base("DeviceRGB", "FlateDecode");
        if picture.color().has_alpha() {
            let alpha: Vec<u8> = picture.to_rgba8().pixels().map(|pixel| pixel[3]).collect();
            let mut mask = base("DeviceGray", "FlateDecode");
            mask.remove(b"Type");
            mask.set("Type", "XObject");
            let mask =
                document.add_object(Stream::new(mask, flate(&alpha)?).with_compression(false));
            node.set("SMask", mask);
        }
        Stream::new(node, flate(picture.to_rgb8().as_raw())?).with_compression(false)
    };
    Ok((document.add_object(object), width, height))
}

/// One new PDF with a page per picture, in order.
pub fn images_to_pdf(pictures: &[Vec<u8>], fit: PageFit) -> Result<Vec<u8>, String> {
    const A4: (f64, f64) = (595.28, 841.89);
    const MARGIN: f64 = 36.0;
    if pictures.is_empty() {
        return Err("Choose at least one picture.".into());
    }
    let mut document = Document::with_version("1.7");
    let root = document.new_object_id();
    let mut kids: Vec<Object> = Vec::new();
    for bytes in pictures {
        let (picture, width, height) = add_picture(&mut document, bytes)?;
        let (width, height) = (f64::from(width), f64::from(height));
        let (page_width, page_height, x, y, drawn_width, drawn_height) = match fit {
            PageFit::Original => (width, height, 0.0, 0.0, width, height),
            PageFit::A4 => {
                let (free_width, free_height) = (A4.0 - 2.0 * MARGIN, A4.1 - 2.0 * MARGIN);
                let scale = (free_width / width).min(free_height / height);
                let (drawn_width, drawn_height) = (width * scale, height * scale);
                let (x, y) = ((A4.0 - drawn_width) / 2.0, (A4.1 - drawn_height) / 2.0);
                (A4.0, A4.1, x, y, drawn_width, drawn_height)
            }
        };
        let content =
            format!("q {drawn_width:.3} 0 0 {drawn_height:.3} {x:.3} {y:.3} cm /Picture Do Q\n");
        let content = document.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let page = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => root,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(page_width as f32), Object::Real(page_height as f32)],
            "Contents" => content,
            "Resources" => dictionary! { "XObject" => dictionary! { "Picture" => picture } },
        });
        kids.push(page.into());
    }
    document.objects.insert(
        root,
        dictionary! { "Type" => "Pages", "Count" => kids.len() as i64, "Kids" => kids }.into(),
    );
    let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => root });
    document.trailer.set("Root", catalog);
    serialize(&mut document)
}

/// Encodes raw RGBA pixels as PNG, or as JPEG on a white background.
pub fn encode_image(rgba: Vec<u8>, width: u32, height: u32, jpeg: bool) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    if jpeg {
        let rgb: Vec<u8> = rgba
            .chunks(4)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        JpegEncoder::new_with_quality(&mut output, 90)
            .encode(&rgb, width, height, ExtendedColorType::Rgb8)
            .map_err(|error| error.to_string())?;
    } else {
        let picture = image::RgbaImage::from_raw(width, height, rgba)
            .ok_or("This page could not be converted.")?;
        picture
            .write_to(&mut Cursor::new(&mut output), ImageFormat::Png)
            .map_err(|error| error.to_string())?;
    }
    Ok(output)
}

fn number(object: &Object) -> Option<f64> {
    object
        .as_float()
        .map(f64::from)
        .or_else(|_| object.as_i64().map(|value| value as f64))
        .ok()
}

/// The largest size each image is drawn at, in points, by scanning page content.
fn drawn_sizes(document: &Document) -> std::collections::HashMap<ObjectId, f64> {
    let mut sizes = std::collections::HashMap::new();
    for page in document.page_iter() {
        let Ok(content) = document.get_and_decode_page_content(page) else {
            continue;
        };
        let images: Vec<(Vec<u8>, ObjectId)> = document
            .get_page_images(page)
            .map(|images| {
                images
                    .into_iter()
                    .map(|image| (Vec::new(), image.id))
                    .collect()
            })
            .unwrap_or_default();
        let mut scale = 1.0_f64;
        let mut stack = Vec::new();
        for operation in &content.operations {
            match operation.operator.as_str() {
                "q" => stack.push(scale),
                "Q" => scale = stack.pop().unwrap_or(1.0),
                "cm" => {
                    if let Some(a) = operation.operands.first().and_then(number) {
                        scale *= a.abs().max(1e-3);
                    }
                }
                "Do" => {
                    for (_, id) in &images {
                        let entry = sizes.entry(*id).or_insert(0.0_f64);
                        *entry = entry.max(scale);
                    }
                }
                _ => {}
            }
        }
    }
    sizes
}

/// Recompresses large pictures and page streams. Returns the input if that is not smaller.
pub fn compress_pdf(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut document = Document::load_mem(bytes).map_err(describe)?;
    if document.is_encrypted() || document.encryption_state.is_some() {
        return Err("Remove this PDF's password first, then try again.".into());
    }
    let sizes = drawn_sizes(&document);
    let ids: Vec<ObjectId> = document.objects.keys().copied().collect();
    for id in ids {
        let Some(replacement) = shrink_image(&document, id, sizes.get(&id).copied()) else {
            continue;
        };
        document.objects.insert(id, replacement);
    }
    document.prune_objects();
    document.compress();
    let mut output = Vec::new();
    document
        .save_modern(&mut output)
        .map_err(|error| error.to_string())?;
    Ok(if output.len() < bytes.len() {
        output
    } else {
        bytes.to_vec()
    })
}

/// A smaller JPEG version of the image object `id`, when that actually saves space.
fn shrink_image(document: &Document, id: ObjectId, drawn: Option<f64>) -> Option<Object> {
    let stream = document.get_object(id).ok()?.as_stream().ok()?;
    let node = &stream.dict;
    let is_image = node.get(b"Subtype").and_then(Object::as_name).ok() == Some(&b"Image"[..]);
    if !is_image || node.has(b"SMask") || node.has(b"Mask") || node.has(b"ImageMask") {
        return None;
    }
    let width = node.get(b"Width").ok()?.as_i64().ok()? as u32;
    let height = node.get(b"Height").ok()?.as_i64().ok()? as u32;
    let bits = node.get(b"BitsPerComponent").ok()?.as_i64().ok()?;
    let filter = node.get(b"Filter").ok().and_then(|filter| match filter {
        Object::Name(name) => Some(name.clone()),
        Object::Array(items) if items.len() == 1 => items[0].as_name().ok().map(<[u8]>::to_vec),
        _ => None,
    });
    let picture = match filter.as_deref() {
        Some(b"DCTDecode") => {
            let picture = ImageReader::with_format(Cursor::new(&stream.content), ImageFormat::Jpeg);
            picture.decode().ok()?.to_rgb8()
        }
        Some(b"FlateDecode") if bits == 8 => {
            let space = document.dereference(node.get(b"ColorSpace").ok()?).ok()?.1;
            if space.as_name().ok()? != b"DeviceRGB" {
                return None;
            }
            let pixels = stream.decompressed_content().ok()?;
            let expected = (width as usize) * (height as usize) * 3;
            (pixels.len() == expected).then_some(())?;
            RgbImage::from_raw(width, height, pixels)?
        }
        _ => return None,
    };
    let target = drawn
        .filter(|drawn| *drawn > 1.0)
        .map(|drawn| (drawn / 72.0 * TARGET_PPI).ceil() as u32);
    let (new_width, new_height) = match target {
        Some(target) if target < width => (target, (height * target / width).max(1)),
        _ => (width, height),
    };
    let picture = if new_width < width {
        image::imageops::resize(
            &picture,
            new_width,
            new_height,
            image::imageops::FilterType::Triangle,
        )
    } else {
        picture
    };
    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, IMAGE_COMPRESS_QUALITY)
        .encode(
            picture.as_raw(),
            new_width,
            new_height,
            ExtendedColorType::Rgb8,
        )
        .ok()?;
    if encoded.len() + 512 >= stream.content.len() {
        return None;
    }
    let replacement = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => i64::from(new_width),
        "Height" => i64::from(new_height),
        "ColorSpace" => "DeviceRGB",
        "BitsPerComponent" => 8,
        "Filter" => "DCTDecode",
    };
    Some(
        Stream::new(replacement, encoded)
            .with_compression(false)
            .into(),
    )
}

/// One page that needs areas removed: `rects` are in PDF coordinates.
pub struct RedactionPage {
    pub number: u32,
    pub rects: Vec<Rect>,
}

/// Rebuilds pages from pixels with `rects` painted black, so nothing beneath survives.
///
/// Only the listed pages lose their selectable text. Any text left in a redacted area is
/// checked for afterwards, and the save fails rather than pretend it worked.
pub fn apply_redactions(
    pdfium: &PdfDocument,
    bytes: &[u8],
    targets: &[RedactionPage],
) -> Result<Vec<u8>, String> {
    let failed = || "The redaction could not be applied, so nothing was saved.".to_owned();
    let mut document = Document::load_mem(bytes).map_err(describe)?;
    let page_ids = document.get_pages();
    for target in targets {
        let page = pdfium
            .pages()
            .get((target.number - 1) as i32)
            .map_err(|_| failed())?;
        let scale = REDACT_DPI / POINTS_PER_INCH;
        let bounds = page.boundaries().bounding().map_err(|_| failed())?.bounds;
        let (left, bottom) = (bounds.left().value, bounds.bottom().value);
        let (page_width, page_height) = (bounds.width().value, bounds.height().value);
        let (width, height) = ((page_width * scale) as u32, (page_height * scale) as u32);
        let stored = page.rotation().map_err(|_| failed())?;
        let config = PdfRenderConfig::new()
            .set_target_size(width as Pixels, height as Pixels)
            .rotate(
                match stored {
                    PdfPageRenderRotation::Degrees90 => PdfPageRenderRotation::Degrees270,
                    PdfPageRenderRotation::Degrees270 => PdfPageRenderRotation::Degrees90,
                    other => other,
                },
                false,
            )
            .render_annotations(true);
        let bitmap = page.render_with_config(&config).map_err(|_| failed())?;
        let mut pixels = RgbImage::new(width, height);
        for (pixel, source) in pixels.pixels_mut().zip(bitmap.as_rgba_bytes().chunks(4)) {
            *pixel = image::Rgb([source[0], source[1], source[2]]);
        }
        for rect in &target.rects {
            let from_x = ((rect.x as f32 - left) * scale).floor().max(0.0) as u32;
            let to_x = (((rect.x + rect.width) as f32 - left) * scale).ceil() as u32;
            let top = page_height - ((rect.y + rect.height) as f32 - bottom);
            let from_y = (top * scale).floor().max(0.0) as u32;
            let to_y = ((top + rect.height as f32) * scale).ceil() as u32;
            for y in from_y..to_y.min(height) {
                for x in from_x..to_x.min(width) {
                    pixels.put_pixel(x, y, image::Rgb([0, 0, 0]));
                }
            }
        }
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 88)
            .encode(pixels.as_raw(), width, height, ExtendedColorType::Rgb8)
            .map_err(|_| failed())?;
        let picture = document.add_object(
            Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Image",
                    "Width" => i64::from(width),
                    "Height" => i64::from(height),
                    "ColorSpace" => "DeviceRGB",
                    "BitsPerComponent" => 8,
                    "Filter" => "DCTDecode",
                },
                jpeg,
            )
            .with_compression(false),
        );
        let content = format!("q {page_width} 0 0 {page_height} {left} {bottom} cm /Page Do Q\n");
        let content = document.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let id = *page_ids.get(&target.number).ok_or_else(failed)?;
        let node = document
            .get_object_mut(id)
            .and_then(Object::as_dict_mut)
            .map_err(|_| failed())?;
        node.set("Contents", content);
        node.set(
            "Resources",
            dictionary! { "XObject" => dictionary! { "Page" => picture } },
        );
        for key in [
            &b"Annots"[..],
            b"Thumb",
            b"Metadata",
            b"PieceInfo",
            b"StructParents",
            b"B",
        ] {
            node.remove(key);
        }
    }
    // Outlines and named destinations can repeat a redacted heading.
    let root = document
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|_| failed())?;
    if let Ok(catalog) = document.get_object_mut(root).and_then(Object::as_dict_mut) {
        for key in [
            &b"Outlines"[..],
            b"Metadata",
            b"StructTreeRoot",
            b"MarkInfo",
            b"Names",
        ] {
            catalog.remove(key);
        }
    }
    if !targets.is_empty() {
        document.prune_objects();
    }
    serialize(&mut document)
}

/// Fails unless the given pages are pure pictures with no text left to extract.
pub fn verify_redactions(document: &PdfDocument, pages: &[u32]) -> Result<(), String> {
    for &number in pages {
        let page = document
            .pages()
            .get(number as i32 - 1)
            .map_err(|error| error.to_string())?;
        let text = page.text().map_err(|error| error.to_string())?;
        let clean = text
            .segments()
            .iter()
            .all(|segment| segment.text().trim().is_empty());
        if !clean {
            return Err("The redaction could not be confirmed, so nothing was saved.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::tests::fixture;
    use crate::reader::tests::engine;
    use ethos_core::LoadOutcome;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let picture = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 30, 30, 128]));
        let mut output = Vec::new();
        picture
            .write_to(&mut Cursor::new(&mut output), ImageFormat::Png)
            .unwrap();
        output
    }

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let picture = RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x * y) % 239) as u8])
        });
        let mut output = Vec::new();
        JpegEncoder::new_with_quality(&mut output, 95)
            .encode(picture.as_raw(), width, height, ExtendedColorType::Rgb8)
            .unwrap();
        output
    }

    fn page_sizes(bytes: Vec<u8>) -> Vec<(f64, f64)> {
        let engine = engine();
        let LoadOutcome::Ready { pages, .. } = engine.load(1, bytes, None).unwrap() else {
            panic!("password prompt");
        };
        pages
            .iter()
            .map(|page| (page.view[2], page.view[3]))
            .collect()
    }

    #[test]
    fn pictures_become_pages_that_render() {
        let pdf = images_to_pdf(&[png(40, 20), jpeg(30, 60)], PageFit::Original).unwrap();
        assert_eq!(page_sizes(pdf), [(40.0, 20.0), (30.0, 60.0)]);
    }

    #[test]
    fn jpegs_are_embedded_without_recompressing() {
        let source = jpeg(30, 60);
        let pdf = images_to_pdf(std::slice::from_ref(&source), PageFit::Original).unwrap();
        assert!(pdf.windows(source.len()).any(|part| part == source));
    }

    #[test]
    fn a4_pages_center_pictures_within_a_margin() {
        let pdf = images_to_pdf(&[png(10, 10)], PageFit::A4).unwrap();
        let (width, height) = page_sizes(pdf)[0];
        assert!((width - 595.28).abs() < 0.1 && (height - 841.89).abs() < 0.1);
    }

    #[test]
    fn unreadable_pictures_fail_clearly() {
        let error = images_to_pdf(&[b"not an image".to_vec()], PageFit::A4).unwrap_err();
        assert!(error.contains("picture"));
    }

    #[test]
    fn page_images_encode_to_valid_files() {
        let rgba = vec![255; 16];
        let png = encode_image(rgba.clone(), 2, 2, false).unwrap();
        assert_eq!(image::guess_format(&png).unwrap(), ImageFormat::Png);
        let jpeg = encode_image(rgba, 2, 2, true).unwrap();
        assert_eq!(image::guess_format(&jpeg).unwrap(), ImageFormat::Jpeg);
    }

    #[test]
    fn a_smaller_copy_shrinks_oversized_pictures_and_still_opens() {
        let source = images_to_pdf(&[jpeg(2400, 2400)], PageFit::A4).unwrap();
        let smaller = compress_pdf(&source).unwrap();
        assert!(
            smaller.len() < source.len() / 2,
            "{} -> {}",
            source.len(),
            smaller.len()
        );
        assert_eq!(page_sizes(smaller).len(), 1);
    }

    #[test]
    fn compression_never_returns_something_larger() {
        let source = fixture();
        assert!(compress_pdf(&source).unwrap().len() <= source.len());
    }

    #[test]
    fn redaction_leaves_nothing_of_the_page_behind() {
        let source = fixture();
        let redacted = {
            let engine = engine();
            let bytes = source.clone();
            engine
                .with_temporary(source.clone(), None, move |document| {
                    let targets = [RedactionPage {
                        number: 1,
                        rects: vec![Rect {
                            x: 15.0,
                            y: 340.0,
                            width: 120.0,
                            height: 25.0,
                        }],
                    }];
                    apply_redactions(document, &bytes, &targets)
                })
                .unwrap()
        };
        let document = Document::load_mem(&redacted).unwrap();
        let everything: Vec<u8> = document
            .objects
            .values()
            .filter_map(|object| object.as_stream().ok())
            .flat_map(|stream| {
                stream
                    .decompressed_content()
                    .unwrap_or_else(|_| stream.content.clone())
            })
            .collect();
        let needle = b"Original page 1";
        assert!(!everything.windows(needle.len()).any(|part| part == needle));
        let needle = b"Original page 2";
        assert!(
            everything.windows(needle.len()).any(|part| part == needle),
            "other pages stay"
        );
        let engine = engine();
        engine.load(1, redacted, None).unwrap();
        assert!(engine.text_runs(1, 1).unwrap().is_empty());
        assert_eq!(engine.text_runs(1, 2).unwrap()[0].text, "Original page 2");
    }
}
