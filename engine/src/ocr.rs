//! Makes scanned pages searchable with the OCR engine built into Windows.

use ethos_core::{Point, Viewport};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use crate::pages::{describe, serialize};

/// A recognized word, in pixels from the bitmap's top-left corner.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// Text recognized on one page of a bitmap rendered at `scale` pixels per point.
pub struct OcrPage {
    pub number: u32,
    pub view: [f64; 4],
    pub rotation: u16,
    pub scale: f64,
    pub words: Vec<OcrWord>,
}

#[cfg(windows)]
pub fn recognize(rgba: &[u8], width: u32, height: u32) -> Result<Vec<OcrWord>, String> {
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
    let failed = |error: windows::core::Error| error.message();
    // Already initialized threads report that here, which is fine.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let bgra: Vec<u8> = rgba
        .chunks(4)
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
        .collect();
    let writer = DataWriter::new().map_err(failed)?;
    writer.WriteBytes(&bgra).map_err(failed)?;
    let buffer = writer.DetachBuffer().map_err(failed)?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
        BitmapAlphaMode::Premultiplied,
    )
    .map_err(failed)?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| {
        "Windows has no text recognition language installed. Add one in Settings, under Language.".to_owned()
    })?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(failed)?
        .join()
        .map_err(failed)?;
    let mut words = Vec::new();
    for line in result.Lines().map_err(failed)? {
        for word in line.Words().map_err(failed)? {
            let rect = word.BoundingRect().map_err(failed)?;
            words.push(OcrWord {
                text: word.Text().map_err(failed)?.to_string(),
                left: f64::from(rect.X),
                top: f64::from(rect.Y),
                width: f64::from(rect.Width),
                height: f64::from(rect.Height),
            });
        }
    }
    Ok(words)
}

#[cfg(not(windows))]
pub fn recognize(_rgba: &[u8], _width: u32, _height: u32) -> Result<Vec<OcrWord>, String> {
    Err("Text recognition needs Windows.".into())
}

/// A font with no glyphs whose character codes are their own Unicode values. Text set in it
/// is never painted, so it only has to map codes back to characters for search and copy.
fn invisible_font(document: &mut Document) -> ObjectId {
    let mapping = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
        /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
        /CMapName /Adobe-Identity-UCS def /CMapType 2 def\n\
        1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
        1 beginbfrange <0000> <FFFF> <0000> endbfrange\n\
        endcmap CMapName currentdict /CMap defineresource pop end end\n";
    let mapping = document.add_object(Stream::new(Dictionary::new(), mapping.as_bytes().to_vec()));
    let descriptor = document.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "EthosInvisible",
        "Flags" => 4,
        "FontBBox" => vec![0.into(), 0.into(), 1000.into(), 1000.into()],
        "ItalicAngle" => 0,
        "Ascent" => 1000,
        "Descent" => 0,
        "CapHeight" => 1000,
        "StemV" => 80,
    });
    let descendant = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => "EthosInvisible",
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("Identity"),
            "Supplement" => 0,
        },
        "FontDescriptor" => descriptor,
        "DW" => 1000,
        "CIDToGIDMap" => "Identity",
    });
    document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "EthosInvisible",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![descendant.into()],
        "ToUnicode" => mapping,
    })
}

fn word_operators(word: &OcrWord, viewport: Viewport, scale: f64) -> Option<String> {
    let characters = word.text.encode_utf16().count();
    if characters == 0 {
        return None;
    }
    let baseline_y = (word.top + word.height) / scale;
    let start = viewport.to_pdf(Point {
        x: word.left / scale,
        y: baseline_y,
    });
    let end = viewport.to_pdf(Point {
        x: (word.left + word.width) / scale,
        y: baseline_y,
    });
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let length = dx.hypot(dy);
    let size = word.height / scale;
    if length <= 0.0 || size <= 0.0 {
        return None;
    }
    let (a, b) = (dx / length, dy / length);
    let stretch = length / (characters as f64 * size) * 100.0;
    let hex: String = word
        .text
        .encode_utf16()
        .map(|unit| format!("{unit:04x}"))
        .collect();
    Some(format!(
        "BT /EthosOcr {size:.3} Tf 3 Tr {stretch:.2} Tz {a:.5} {b:.5} {:.5} {a:.5} {:.3} {:.3} Tm <{hex}> Tj ET\n",
        -b, start.x, start.y
    ))
}

/// Adds recognized words to pages as invisible text, leaving how they look unchanged.
pub fn add_text_layer(bytes: &[u8], pages: &[OcrPage]) -> Result<Vec<u8>, String> {
    let mut document = Document::load_mem(bytes).map_err(describe)?;
    let font = invisible_font(&mut document);
    let ids = document.get_pages();
    for page in pages {
        let viewport = Viewport::new(page.view, page.rotation, 1.0);
        let operators: String = page
            .words
            .iter()
            .filter_map(|word| word_operators(word, viewport, page.scale))
            .collect();
        if operators.is_empty() {
            continue;
        }
        let id = *ids
            .get(&page.number)
            .ok_or("A page went missing while it was being read.")?;
        let stream = document.add_object(Stream::new(Dictionary::new(), operators.into_bytes()));
        let restore = document.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
        let close = document.add_object(Stream::new(Dictionary::new(), b"Q\n".to_vec()));
        let node = document.get_dictionary(id).map_err(describe)?;
        let mut resources = match crate::pages::inherited(&document, id, b"Resources") {
            Some(value) => document
                .dereference(&value)
                .map_err(describe)?
                .1
                .as_dict()
                .map_err(describe)?
                .clone(),
            None => Dictionary::new(),
        };
        let mut fonts = match resources.get(b"Font") {
            Ok(value) => document
                .dereference(value)
                .map_err(describe)?
                .1
                .as_dict()
                .map_err(describe)?
                .clone(),
            Err(_) => Dictionary::new(),
        };
        fonts.set("EthosOcr", font);
        resources.set("Font", fonts);
        let existing = match node.get(b"Contents") {
            Ok(Object::Array(streams)) => streams.clone(),
            Ok(stream) => match document.dereference(stream).map_err(describe)?.1 {
                Object::Array(streams) => streams.clone(),
                _ => vec![stream.clone()],
            },
            Err(_) => Vec::new(),
        };
        let mut contents: Vec<Object> = vec![restore.into()];
        contents.extend(existing);
        contents.push(close.into());
        contents.push(stream.into());
        let node = document
            .get_object_mut(id)
            .and_then(Object::as_dict_mut)
            .map_err(describe)?;
        node.set("Resources", resources);
        node.set("Contents", contents);
    }
    serialize(&mut document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{PageFit, images_to_pdf};
    use crate::reader::tests::engine;

    fn word(text: &str, left: f64) -> OcrWord {
        OcrWord {
            text: text.into(),
            left,
            top: 100.0,
            width: 120.0,
            height: 24.0,
        }
    }

    fn scan() -> Vec<u8> {
        let picture = image::RgbImage::from_pixel(612, 792, image::Rgb([255, 255, 255]));
        let mut png = Vec::new();
        picture
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        images_to_pdf(&[png], PageFit::Original).unwrap()
    }

    #[test]
    fn recognized_words_become_searchable_in_any_script() {
        let page = OcrPage {
            number: 1,
            view: [0.0, 0.0, 612.0, 792.0],
            rotation: 0,
            scale: 2.0,
            words: vec![
                word("Scanned", 100.0),
                word("résumé", 260.0),
                word("你好", 420.0),
            ],
        };
        let searchable = add_text_layer(&scan(), &[page]).unwrap();
        let engine = engine();
        engine.load(1, searchable, None).unwrap();
        let text = engine
            .text_runs(1, 1)
            .unwrap()
            .into_iter()
            .map(|run| run.text)
            .collect::<Vec<_>>()
            .join(" ");
        for expected in ["Scanned", "résumé", "你好"] {
            assert!(text.contains(expected), "{expected} in {text}");
        }
        let hits = engine.search(1, "SCANNED".into(), vec![1]).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn the_invisible_layer_does_not_change_how_the_page_looks() {
        let render = |bytes: Vec<u8>| {
            let engine = engine();
            engine.load(1, bytes, None).unwrap();
            let frame = crate::Frame {
                token: 1,
                source: 1,
                rotation: 0,
                width: 153,
                height: 198,
            };
            engine.render(1, frame).unwrap()
        };
        let page = OcrPage {
            number: 1,
            view: [0.0, 0.0, 612.0, 792.0],
            rotation: 0,
            scale: 2.0,
            words: vec![word("Hidden", 100.0)],
        };
        let searchable = add_text_layer(&scan(), &[page]).unwrap();
        assert_eq!(render(searchable), render(scan()));
    }

    #[cfg(windows)]
    #[test]
    fn windows_reads_text_from_a_rendered_page() {
        let pixels = {
            let engine = engine();
            engine
                .load(1, crate::create_sample().unwrap(), None)
                .unwrap();
            let frame = crate::Frame {
                token: 1,
                source: 1,
                rotation: 0,
                width: 1224,
                height: 1584,
            };
            engine.render(1, frame).unwrap()
        };
        match recognize(&pixels, 1224, 1584) {
            Ok(words) => {
                let text = words
                    .iter()
                    .map(|word| word.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                assert!(text.to_lowercase().contains("attention"), "{text}");
            }
            Err(error) => assert!(error.contains("language"), "{error}"),
        }
    }
}
