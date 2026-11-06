use std::collections::HashMap;
use std::fmt::Write;

use ethos_core::{
    Edits, Mark, MarkKind, NOTE_FONT_SIZE, NOTE_LINE_HEIGHT, encode_win_ansi, note_width,
};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat, dictionary};

const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
const HIGHLIGHT_STATE: &str = "EthosHighlight";
const PEN_STATE: &str = "EthosPen";
const NOTE_FONT: &str = "EthosFont";
const HIGHLIGHT_OPACITY: f64 = 0.32;
const PEN_OPACITY: f64 = 0.95;
const PEN_WIDTH: f64 = 1.8;
const MARK_KEY: &str = "EthosMark";
const SIGNATURE_WIDTH: f64 = 1.4;
const SIGNATURE_INK: &str = "#1b2a4a";

fn describe(error: lopdf::Error) -> String {
    error.to_string()
}

/// Looks up `key` on a page or the nearest ancestor that defines it.
fn inherited(document: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut node = document.get_dictionary(page).ok()?;
    // Parent chains come from the file, so a cycle must not hang the export.
    for _ in 0..256 {
        if let Ok(value) = node.get(key) {
            return Some(value.clone());
        }
        let parent = node.get(b"Parent").and_then(Object::as_reference).ok()?;
        node = document.get_dictionary(parent).ok()?;
    }
    None
}

fn page_rotation(document: &Document, page: ObjectId) -> Result<i64, String> {
    let Some(value) = inherited(document, page, b"Rotate") else {
        return Ok(0);
    };
    let (_, value) = document.dereference(&value).map_err(describe)?;
    Ok(value.as_i64().map_err(describe)?.rem_euclid(360))
}

/// Replaces the page tree with a flat list so pages can be dropped or reordered.
fn rebuild_page_tree(document: &mut Document, pages: &[ObjectId]) -> Result<(), String> {
    let root = document
        .catalog()
        .and_then(|catalog| catalog.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(describe)?;
    for &page in pages {
        // Attributes on removed ancestors must move down to the page itself.
        let attributes: Vec<_> = INHERITABLE
            .iter()
            .filter_map(|key| Some((*key, inherited(document, page, key)?)))
            .collect();
        let node = document
            .get_object_mut(page)
            .and_then(Object::as_dict_mut)
            .map_err(describe)?;
        for (key, value) in attributes {
            node.set(key, value);
        }
        node.set("Parent", root);
    }
    let node = document
        .get_object_mut(root)
        .and_then(Object::as_dict_mut)
        .map_err(describe)?;
    node.set(
        "Kids",
        pages.iter().copied().map(Object::from).collect::<Vec<_>>(),
    );
    node.set("Count", pages.len() as i64);
    INHERITABLE.iter().for_each(|key| drop(node.remove(key)));
    Ok(())
}

fn color(hex: &str) -> [f64; 3] {
    [1, 3, 5].map(|start| {
        f64::from(u8::from_str_radix(&hex[start..start + 2], 16).expect("ink colors are hex"))
            / 255.0
    })
}

/// The operators that draw `mark` in PDF page coordinates, used as its appearance.
fn draw(mark: &Mark, output: &mut String) -> Result<(), String> {
    let ink = |values: [f64; 3]| format!("{:.4} {:.4} {:.4}", values[0], values[1], values[2]);
    match &mark.kind {
        MarkKind::Highlight { rects, .. } => {
            let fill = ink(color(mark.color.ink()));
            for rect in rects {
                let (x, y, width, height) = (rect.x, rect.y, rect.width, rect.height);
                writeln!(
                    output,
                    "q /{HIGHLIGHT_STATE} gs {fill} rg {x:.3} {y:.3} {width:.3} {height:.3} re f Q"
                )
                .expect("writing to a String");
            }
        }
        MarkKind::Pen { points } => {
            let Some((first, rest)) = points.split_first() else {
                return Ok(());
            };
            let stroke = ink(color(mark.color.stroke()));
            write!(
                output,
                "q /{PEN_STATE} gs {stroke} RG {PEN_WIDTH} w 1 J 1 j {:.3} {:.3} m",
                first.x, first.y
            )
            .expect("writing to a String");
            for point in rest {
                write!(output, " {:.3} {:.3} l", point.x, point.y).expect("writing to a String");
            }
            output.push_str(" S Q\n");
        }
        MarkKind::Signature { strokes } => {
            let stroke = ink(color(SIGNATURE_INK));
            write!(output, "q {stroke} RG {SIGNATURE_WIDTH} w 1 J 1 j")
                .expect("writing to a String");
            for points in strokes {
                let Some((first, rest)) = points.split_first() else {
                    continue;
                };
                write!(output, " {:.3} {:.3} m", first.x, first.y).expect("writing to a String");
                for point in rest {
                    write!(output, " {:.3} {:.3} l", point.x, point.y)
                        .expect("writing to a String");
                }
                output.push_str(" S");
            }
            output.push_str(" Q\n");
        }
        // Redactions are burned into pixels by a later step, never drawn as a mark.
        MarkKind::Redaction { .. } => {}
        MarkKind::Text { point, text } => {
            let fill = ink(color(mark.color.stroke()));
            write!(
                output,
                "q {fill} rg BT /{NOTE_FONT} {NOTE_FONT_SIZE} Tf {NOTE_LINE_HEIGHT} TL {:.3} {:.3} Td",
                point.x, point.y
            )
            .expect("writing to a String");
            for (index, line) in text.split('\n').enumerate() {
                let encoded = encode_win_ansi(line).ok_or(
                    "This note contains characters the export font cannot display. Use Western European text for now.",
                )?;
                let hex: String = encoded.iter().map(|byte| format!("{byte:02x}")).collect();
                let advance = if index == 0 { "" } else { "T* " };
                write!(output, " {advance}<{hex}> Tj").expect("writing to a String");
            }
            output.push_str(" ET Q\n");
        }
    }
    Ok(())
}

/// Objects shared by every annotation of one export.
struct MarkResources {
    highlight: ObjectId,
    pen: ObjectId,
    font: ObjectId,
}

impl MarkResources {
    fn new(document: &mut Document) -> Self {
        Self {
            highlight: document.add_object(
                dictionary! { "Type" => "ExtGState", "ca" => HIGHLIGHT_OPACITY as f32 },
            ),
            pen: document
                .add_object(dictionary! { "Type" => "ExtGState", "CA" => PEN_OPACITY as f32 }),
            font: document.add_object(dictionary! {
                "Type" => "Font",
                "Subtype" => "Type1",
                "BaseFont" => "Helvetica",
                "Encoding" => "WinAnsiEncoding",
            }),
        }
    }
}

/// The smallest `[left, bottom, right, top]` box holding a mark's appearance.
fn mark_box(mark: &Mark) -> [f64; 4] {
    let around = |points: &mut dyn Iterator<Item = (f64, f64)>, pad: f64| {
        let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for (x, y) in points {
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
        }
        [
            bounds[0] - pad,
            bounds[1] - pad,
            bounds[2] + pad,
            bounds[3] + pad,
        ]
    };
    match &mark.kind {
        MarkKind::Highlight { rects, .. } => around(
            &mut rects
                .iter()
                .flat_map(|r| [(r.x, r.y), (r.x + r.width, r.y + r.height)]),
            0.0,
        ),
        MarkKind::Pen { points } => around(
            &mut points.iter().map(|point| (point.x, point.y)),
            PEN_WIDTH,
        ),
        MarkKind::Signature { strokes } => around(
            &mut strokes.iter().flatten().map(|point| (point.x, point.y)),
            SIGNATURE_WIDTH,
        ),
        MarkKind::Redaction { rect } => [rect.x, rect.y, rect.x + rect.width, rect.y + rect.height],
        MarkKind::Text { point, text } => {
            let lines = text.split('\n').count() as f64;
            let width = text.split('\n').map(note_width).fold(0.0, f64::max);
            [
                point.x - 1.0,
                point.y - (lines - 1.0) * NOTE_LINE_HEIGHT - 4.0,
                point.x + width + 1.0,
                point.y + NOTE_FONT_SIZE,
            ]
        }
    }
}

fn numbers(values: impl IntoIterator<Item = f64>) -> Vec<Object> {
    values
        .into_iter()
        .map(|value| Object::Real(value as f32))
        .collect()
}

/// PDF text strings outside ASCII are UTF-16 with a byte order mark.
fn text_string(text: &str) -> Object {
    let mut bytes = vec![0xfe, 0xff];
    bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    Object::String(bytes, StringFormat::Hexadecimal)
}

/// Builds a standard annotation with its appearance, so any PDF app shows and edits it.
/// `EthosMark` keeps the exact mark, so ethos can restore it without guessing.
fn annotation(
    document: &mut Document,
    mark: &Mark,
    shared: &MarkResources,
) -> Result<ObjectId, String> {
    let mut appearance = String::new();
    draw(mark, &mut appearance)?;
    let bounds = mark_box(mark);
    let form = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => numbers(bounds),
            "Resources" => dictionary! {
                "ExtGState" => dictionary! {
                    HIGHLIGHT_STATE => shared.highlight,
                    PEN_STATE => shared.pen,
                },
                "Font" => dictionary! { NOTE_FONT => shared.font },
            },
        },
        appearance.into_bytes(),
    );
    let form = document.add_object(form);
    let stored = serde_json::to_vec(mark).map_err(|error| error.to_string())?;
    let hex = match mark.kind {
        MarkKind::Highlight { .. } => mark.color.ink(),
        _ => mark.color.stroke(),
    };
    let mut node = dictionary! {
        "Type" => "Annot",
        "Rect" => numbers(bounds),
        "NM" => text_string(&mark.id),
        "F" => 4,
        "C" => numbers(color(hex)),
        "AP" => dictionary! { "N" => form },
        MARK_KEY => Object::String(stored, StringFormat::Hexadecimal),
    };
    match &mark.kind {
        MarkKind::Highlight { rects, text } => {
            let corners = rects.iter().flat_map(|r| {
                let (right, top) = (r.x + r.width, r.y + r.height);
                [r.x, top, right, top, r.x, r.y, right, r.y]
            });
            node.set("Subtype", "Highlight");
            node.set("QuadPoints", numbers(corners));
            node.set("Contents", text_string(text));
        }
        MarkKind::Pen { points } => {
            let path = points.iter().flat_map(|point| [point.x, point.y]);
            node.set("Subtype", "Ink");
            node.set("InkList", vec![Object::Array(numbers(path))]);
            node.set("BS", dictionary! { "W" => PEN_WIDTH as f32 });
        }
        MarkKind::Signature { strokes } => {
            let paths = strokes.iter().map(|points| {
                Object::Array(numbers(points.iter().flat_map(|point| [point.x, point.y])))
            });
            node.set("Subtype", "Ink");
            node.set("InkList", paths.collect::<Vec<_>>());
            node.set("BS", dictionary! { "W" => SIGNATURE_WIDTH as f32 });
        }
        MarkKind::Redaction { .. } => node.set("Subtype", "Square"),
        MarkKind::Text { text, .. } => {
            let [r, g, b] = color(mark.color.stroke());
            let default = format!("/{NOTE_FONT} {NOTE_FONT_SIZE} Tf {r:.4} {g:.4} {b:.4} rg");
            node.set("Subtype", "FreeText");
            node.set("Contents", text_string(text));
            node.set("DA", Object::string_literal(default));
        }
    }
    Ok(document.add_object(node))
}

/// The annotation references a page holds directly or through an indirect array.
fn page_annotations(document: &Document, page: ObjectId) -> Result<Vec<Object>, String> {
    let node = document.get_dictionary(page).map_err(describe)?;
    match node.get(b"Annots") {
        Ok(value) => {
            let (_, value) = document.dereference(value).map_err(describe)?;
            value.as_array().cloned().map_err(describe)
        }
        Err(_) => Ok(Vec::new()),
    }
}

fn set_page_annotations(
    document: &mut Document,
    page: ObjectId,
    annotations: Vec<Object>,
) -> Result<(), String> {
    let node = document
        .get_object_mut(page)
        .and_then(Object::as_dict_mut)
        .map_err(describe)?;
    node.set("Annots", annotations);
    Ok(())
}

/// Adds marks to a page as annotations, leaving its content streams untouched.
fn annotate(
    document: &mut Document,
    page: ObjectId,
    marks: &[&Mark],
    shared: &MarkResources,
) -> Result<(), String> {
    let mut annotations = page_annotations(document, page)?;
    for mark in marks {
        annotations.push(annotation(document, mark, shared)?.into());
    }
    set_page_annotations(document, page, annotations)
}

/// Takes ethos marks back out of a saved copy so they are editable instead of painted in.
/// Returns the file without them, or `None` when it holds none, along with the marks.
pub fn split_marks(bytes: &[u8]) -> (Option<Vec<u8>>, Vec<Mark>) {
    let Ok(mut document) = Document::load_mem(bytes) else {
        return (None, Vec::new());
    };
    // Encrypted files cannot be rewritten without their password, so their marks stay as
    // ordinary annotations.
    if document.is_encrypted() || document.encryption_state.is_some() {
        return (None, Vec::new());
    }
    let mut marks = Vec::new();
    for (number, page) in document.get_pages() {
        let Ok(annotations) = page_annotations(&document, page) else {
            continue;
        };
        let mut kept = Vec::with_capacity(annotations.len());
        for item in annotations {
            let stored = document
                .dereference(&item)
                .ok()
                .and_then(|(_, object)| object.as_dict().ok())
                .and_then(|node| node.get(MARK_KEY.as_bytes()).ok())
                .and_then(|value| value.as_str().ok())
                .and_then(|text| serde_json::from_slice::<Mark>(text).ok());
            match stored {
                Some(mark) => marks.push(Mark {
                    page: number,
                    ..mark
                }),
                None => kept.push(item),
            }
        }
        if set_page_annotations(&mut document, page, kept).is_err() {
            return (None, Vec::new());
        }
    }
    if marks.is_empty() {
        return (None, marks);
    }
    document.prune_objects();
    let mut clean = Vec::new();
    match document.save_to(&mut clean) {
        Ok(_) => (Some(clean), marks),
        Err(_) => (None, Vec::new()),
    }
}

fn stamp_metadata(document: &mut Document) -> Result<(), String> {
    let info = match document.trailer.get(b"Info").and_then(Object::as_reference) {
        Ok(id) => id,
        Err(_) => {
            let id = document.add_object(Dictionary::new());
            document.trailer.set("Info", id);
            id
        }
    };
    let info = document
        .get_object_mut(info)
        .and_then(Object::as_dict_mut)
        .map_err(describe)?;
    if !info.has(b"Title") {
        info.set("Title", Object::string_literal("Document"));
    }
    info.set("Creator", Object::string_literal("ethos"));
    Ok(())
}

/// What must be removed from a saved copy, by that copy's page numbers.
pub fn redaction_pages(edits: &Edits) -> Vec<crate::RedactionPage> {
    edits
        .pages
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let rects: Vec<_> = edits
                .marks
                .iter()
                .filter(|mark| mark.page == entry.source)
                .filter_map(|mark| match mark.kind {
                    MarkKind::Redaction { rect } => Some(rect),
                    _ => None,
                })
                .collect();
            (!rects.is_empty()).then_some(crate::RedactionPage {
                number: index as u32 + 1,
                rects,
            })
        })
        .collect()
}

/// Writes `edits` into a new copy of the PDF in `bytes`, which is left untouched.
pub fn export_pdf(bytes: &[u8], edits: &Edits) -> Result<Vec<u8>, String> {
    const PROTECTED: &str =
        "Protected PDFs can be read here, but exporting them is not supported yet.";
    let mut document = Document::load_mem(bytes).map_err(|error| {
        let message = error.to_string();
        if message.to_lowercase().contains("encrypt") || message.to_lowercase().contains("decrypt")
        {
            PROTECTED.to_owned()
        } else {
            message
        }
    })?;
    if document.is_encrypted() || document.encryption_state.is_some() {
        return Err(PROTECTED.into());
    }

    let sources = document.get_pages();
    let pages = edits
        .pages
        .iter()
        .map(|entry| {
            sources
                .get(&entry.source)
                .copied()
                .ok_or_else(|| format!("Page {} is missing from this document.", entry.source))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let reshaped = pages.len() != sources.len()
        || edits
            .pages
            .iter()
            .enumerate()
            .any(|(index, entry)| entry.source as usize != index + 1);
    if reshaped {
        rebuild_page_tree(&mut document, &pages)?;
    }

    let mut marks: HashMap<u32, Vec<&Mark>> = HashMap::new();
    for mark in edits
        .marks
        .iter()
        .filter(|mark| !matches!(mark.kind, MarkKind::Redaction { .. }))
    {
        marks.entry(mark.page).or_default().push(mark);
    }
    let has_visible_marks = edits
        .pages
        .iter()
        .any(|entry| marks.contains_key(&entry.source));
    let shared = has_visible_marks.then(|| MarkResources::new(&mut document));

    for (&page, entry) in pages.iter().zip(&edits.pages) {
        if let Some(rotation) = entry.rotation
            && i64::from(rotation) != page_rotation(&document, page)?
        {
            document
                .get_object_mut(page)
                .and_then(Object::as_dict_mut)
                .map_err(describe)?
                .set("Rotate", i64::from(rotation));
        }
        if let (Some(marks), Some(shared)) = (marks.get(&entry.source), &shared) {
            annotate(&mut document, page, marks, shared)?;
        }
    }
    crate::forms::apply_fields(&mut document, &edits.fields)?;
    stamp_metadata(&mut document)?;
    if reshaped {
        document.prune_objects();
    }
    let mut output = Vec::new();
    document
        .save_to(&mut output)
        .map_err(|error| error.to_string())?;
    Ok(output)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::create_sample;
    use crate::reader::Frame;
    use crate::reader::tests::engine;
    use ethos_core::{InkColor, LoadOutcome, PageEntry, Point, Rect};

    fn stream_text(id: &str, x: i64, y: i64, line: &str) -> Vec<u8> {
        format!("BT /{id} 12 Tf {x} {y} Td ({line}) Tj ET\n").into_bytes()
    }

    fn helvetica(document: &mut Document) -> Dictionary {
        let font = document.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        dictionary! { "Font" => dictionary! { "F" => font } }
    }

    fn save(document: &mut Document) -> Vec<u8> {
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        bytes
    }

    fn finish(document: &mut Document, pages: ObjectId, kids: Vec<Object>) {
        document.objects.insert(
            pages,
            dictionary! { "Type" => "Pages", "Count" => kids.len() as i64, "Kids" => kids }.into(),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        document.trailer.set("Root", catalog);
    }

    pub(crate) fn fixture_document() -> Document {
        let mut document = Document::with_version("1.7");
        let pages = document.new_object_id();
        let resources = helvetica(&mut document);
        let kids = (1..=3)
            .map(|index| {
                let content = stream_text("F", 20, 350, &format!("Original page {index}"));
                let content = document.add_object(Stream::new(Dictionary::new(), content));
                let mut page = dictionary! {
                    "Type" => "Page",
                    "Parent" => pages,
                    "MediaBox" => vec![0.into(), 0.into(), (300 + index * 10).into(), 400.into()],
                    "Contents" => content,
                    "Resources" => resources.clone(),
                };
                if index == 2 {
                    page.set("Rotate", 90);
                }
                document.add_object(page).into()
            })
            .collect();
        finish(&mut document, pages, kids);
        let info = document.add_object(dictionary! {
            "Title" => Object::string_literal("Fixture title"),
            "Author" => Object::string_literal("Fixture author"),
            "Subject" => Object::string_literal("Fixture subject"),
            "Keywords" => Object::string_literal("fixture roundtrip"),
        });
        document.trailer.set("Info", info);
        document
    }

    /// Three pages sized 310, 320 and 330 by 400, the second stored at 90 degrees.
    pub(crate) fn fixture() -> Vec<u8> {
        save(&mut fixture_document())
    }

    /// One page whose text runs up the page, as in many scanned landscape documents.
    pub(crate) fn sideways_fixture() -> Vec<u8> {
        let mut document = Document::with_version("1.7");
        let pages = document.new_object_id();
        let resources = helvetica(&mut document);
        let content = b"BT /F 12 Tf 0 1 -1 0 100 100 Tm (Reading upward) Tj ET\n".to_vec();
        let content = document.add_object(Stream::new(Dictionary::new(), content));
        let page = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages,
            "MediaBox" => vec![0.into(), 0.into(), 300.into(), 400.into()],
            "Contents" => content,
            "Resources" => resources,
        });
        finish(&mut document, pages, vec![page.into()]);
        save(&mut document)
    }

    pub(crate) fn protected_fixture() -> Vec<u8> {
        use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
        use lopdf::{EncryptionState, EncryptionVersion, Permissions};
        use std::collections::BTreeMap;
        use std::sync::Arc;
        let mut document = fixture_document();
        let id = Object::string_literal("ethos-fixture-id");
        document.trailer.set("ID", vec![id.clone(), id]);
        let filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
        let version = EncryptionVersion::V4 {
            document: &document,
            encrypt_metadata: true,
            crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: "owner",
            user_password: "secret",
            permissions: Permissions::all(),
        };
        let state = EncryptionState::try_from(version).unwrap();
        document.encrypt(&state).unwrap();
        save(&mut document)
    }

    /// The first page takes its geometry, rotation and resources from a page-tree branch.
    pub(crate) fn inherited_fixture() -> Vec<u8> {
        let mut document = Document::with_version("1.7");
        let root = document.new_object_id();
        let resources = helvetica(&mut document);
        let branch = document.new_object_id();
        let content = stream_text("F", 20, 300, "Inherited resource content");
        let content = document.add_object(Stream::new(Dictionary::new(), content));
        let first = document.add_object(dictionary! {
            "Type" => "Page", "Parent" => branch, "Contents" => content,
        });
        document.objects.insert(
            branch,
            dictionary! {
                "Type" => "Pages",
                "Parent" => root,
                "Kids" => vec![first.into()],
                "Count" => 1,
                "Resources" => resources,
                "MediaBox" => vec![0.into(), 0.into(), 300.into(), 400.into()],
                "CropBox" => vec![10.into(), 20.into(), 290.into(), 380.into()],
                "Rotate" => 90,
            }
            .into(),
        );
        let second = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => root,
            "MediaBox" => vec![0.into(), 0.into(), 500.into(), 600.into()],
        });
        finish(&mut document, root, vec![branch.into(), second.into()]);
        save(&mut document)
    }

    fn texts(bytes: &[u8]) -> Vec<String> {
        let engine = engine();
        let LoadOutcome::Ready { pages, .. } = engine.load(1, bytes.to_vec(), None).unwrap() else {
            panic!("password prompt");
        };
        (1..=pages.len() as u32)
            .map(|page| {
                engine
                    .text_runs(1, page)
                    .unwrap()
                    .into_iter()
                    .map(|run| run.text)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    fn entry(source: u32, rotation: Option<u16>) -> PageEntry {
        PageEntry { source, rotation }
    }

    fn edits(pages: Vec<PageEntry>, marks: Vec<Mark>) -> Edits {
        Edits {
            marks,
            pages,
            fields: Vec::new(),
        }
    }

    fn note(page: u32, text: &str) -> Mark {
        Mark {
            id: page.to_string(),
            page,
            color: InkColor::Blue,
            kind: MarkKind::Text {
                point: Point { x: 20.0, y: 200.0 },
                text: text.into(),
            },
        }
    }

    fn stored_rotation(document: &Document, number: u32) -> i64 {
        page_rotation(document, document.get_pages()[&number]).unwrap()
    }

    fn info_text(document: &Document, key: &[u8]) -> String {
        let id = document
            .trailer
            .get(b"Info")
            .unwrap()
            .as_reference()
            .unwrap();
        let value = document.get_dictionary(id).unwrap().get(key).unwrap();
        String::from_utf8_lossy(value.as_str().unwrap()).into_owned()
    }

    #[test]
    fn preserves_inherited_geometry_and_resources_with_and_without_marks() {
        let bytes = inherited_fixture();
        let highlight = Mark {
            id: "h".into(),
            page: 1,
            color: InkColor::Yellow,
            kind: MarkKind::Highlight {
                text: "Inherited".into(),
                rects: vec![Rect {
                    x: 20.0,
                    y: 295.0,
                    width: 100.0,
                    height: 15.0,
                }],
            },
        };
        for marks in [vec![], vec![highlight]] {
            let result = export_pdf(&bytes, &edits(vec![entry(1, Some(270))], marks)).unwrap();
            let outcome = engine().load(1, result.clone(), None).unwrap();
            let LoadOutcome::Ready { pages, .. } = outcome else {
                panic!("password prompt");
            };
            assert_eq!(pages.len(), 1);
            assert_eq!(pages[0].view, [10.0, 20.0, 290.0, 380.0]);
            assert_eq!(pages[0].rotate, 270);
            assert_eq!(texts(&result), ["Inherited resource content"]);
        }
    }

    #[test]
    fn retains_form_fields_and_their_values() {
        let mut document = fixture_document();
        let field = document.add_object(dictionary! {
            "FT" => "Tx",
            "T" => Object::string_literal("reader-name"),
            "V" => Object::string_literal("Existing form content"),
            "Rect" => vec![30.into(), 300.into(), 230.into(), 324.into()],
            "Subtype" => "Widget",
        });
        let first = document.get_pages()[&1];
        document
            .get_object_mut(first)
            .and_then(Object::as_dict_mut)
            .unwrap()
            .set("Annots", vec![field.into()]);
        let catalog = document.catalog_mut().unwrap();
        catalog.set("AcroForm", dictionary! { "Fields" => vec![field.into()] });
        let result = export_pdf(
            &save(&mut document),
            &edits(
                vec![entry(1, Some(0)), entry(2, None), entry(3, None)],
                vec![],
            ),
        )
        .unwrap();
        let restored = Document::load_mem(&result).unwrap();
        let form = restored
            .catalog()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_dict()
            .unwrap();
        let field = form.get(b"Fields").unwrap().as_array().unwrap()[0]
            .as_reference()
            .unwrap();
        let value = restored.get_dictionary(field).unwrap().get(b"V").unwrap();
        assert_eq!(value.as_str().unwrap(), b"Existing form content");
    }

    #[test]
    fn keeps_stored_rotations_when_no_rotation_was_requested() {
        let result = export_pdf(
            &fixture(),
            &edits(vec![entry(2, None), entry(1, None)], vec![]),
        )
        .unwrap();
        let restored = Document::load_mem(&result).unwrap();
        assert_eq!(stored_rotation(&restored, 1), 90);
        assert_eq!(stored_rotation(&restored, 2), 0);
        assert_eq!(texts(&result), ["Original page 2", "Original page 1"]);
    }

    #[test]
    fn plain_exports_keep_the_page_tree_and_add_no_font() {
        let bytes = inherited_fixture();
        let result =
            export_pdf(&bytes, &edits(vec![entry(1, None), entry(2, None)], vec![])).unwrap();
        let restored = Document::load_mem(&result).unwrap();
        let parent = |document: &Document| {
            let page = document.get_dictionary(document.get_pages()[&1]).unwrap();
            page.get(b"Parent").unwrap().as_reference().unwrap()
        };
        assert_eq!(
            parent(&restored),
            parent(&Document::load_mem(&bytes).unwrap())
        );
        let fonts = restored
            .objects
            .values()
            .filter(|object| object.as_dict().is_ok_and(|node| node.has_type(b"Font")))
            .count();
        assert_eq!(fonts, 1, "only the original Helvetica");
    }

    #[test]
    fn sample_is_a_readable_four_page_pdf_with_title_and_author() {
        let bytes = create_sample().unwrap();
        let document = Document::load_mem(&bytes).unwrap();
        assert_eq!(document.get_pages().len(), 4);
        assert_eq!(
            info_text(&document, b"Title"),
            "A little more room to think"
        );
        assert_eq!(info_text(&document, b"Author"), "ethos");
        let content = texts(&bytes);
        assert!(content[0].contains("A little more room"));
        assert!(content[3].contains("Save a copy to keep your annotations"));
    }

    #[test]
    fn preserves_original_bytes_text_and_metadata() {
        let bytes = fixture();
        let before = bytes.clone();
        let pages = vec![entry(1, Some(0)), entry(2, Some(90)), entry(3, Some(0))];
        let result = export_pdf(&bytes, &edits(pages, vec![])).unwrap();
        assert_eq!(bytes, before);
        let restored = Document::load_mem(&result).unwrap();
        assert_eq!(info_text(&restored, b"Title"), "Fixture title");
        assert_eq!(info_text(&restored, b"Author"), "Fixture author");
        assert_eq!(info_text(&restored, b"Subject"), "Fixture subject");
        assert!(info_text(&restored, b"Keywords").contains("roundtrip"));
        assert_eq!(info_text(&restored, b"Creator"), "ethos");
        assert_eq!(texts(&result), texts(&bytes));
    }

    #[test]
    fn untitled_documents_get_a_default_title() {
        let bytes = inherited_fixture();
        let result =
            export_pdf(&bytes, &edits(vec![entry(1, None), entry(2, None)], vec![])).unwrap();
        assert_eq!(
            info_text(&Document::load_mem(&result).unwrap(), b"Title"),
            "Document"
        );
    }

    #[test]
    fn reorders_rotates_and_deletes_pages_by_source_identity() {
        let bytes = fixture();
        let pages = vec![entry(3, Some(270)), entry(1, Some(180))];
        let result = export_pdf(&bytes, &edits(pages, vec![])).unwrap();
        let restored = Document::load_mem(&result).unwrap();
        assert_eq!(restored.get_pages().len(), 2);
        assert_eq!(stored_rotation(&restored, 1), 270);
        assert_eq!(stored_rotation(&restored, 2), 180);
        assert_eq!(texts(&result), ["Original page 3", "Original page 1"]);
        assert_eq!(Document::load_mem(&bytes).unwrap().get_pages().len(), 3);
    }

    #[test]
    fn deleted_pages_leave_the_file() {
        let result = export_pdf(&fixture(), &edits(vec![entry(1, None)], vec![])).unwrap();
        assert!(
            !Document::load_mem(&result)
                .unwrap()
                .objects
                .values()
                .any(|object| {
                    object
                        .as_stream()
                        .is_ok_and(|stream| stream.content.windows(6).any(|part| part == b"page 2"))
                })
        );
    }

    fn note_text(mark: &Mark) -> &str {
        match &mark.kind {
            MarkKind::Text { text, .. } => text,
            _ => panic!("expected a note"),
        }
    }

    #[test]
    fn notes_follow_their_source_page_and_deleted_notes_disappear() {
        let marks = (1..=3)
            .map(|page| note(page, &format!("Annotation {page}")))
            .collect();
        let result = export_pdf(
            &fixture(),
            &edits(vec![entry(3, Some(0)), entry(1, Some(0))], marks),
        )
        .unwrap();
        let (_, marks) = split_marks(&result);
        let found: Vec<_> = marks
            .iter()
            .map(|mark| (mark.page, note_text(mark)))
            .collect();
        assert_eq!(found, [(1, "Annotation 3"), (2, "Annotation 1")]);
    }

    #[test]
    fn multi_line_notes_keep_every_line() {
        let pages = vec![entry(1, None), entry(2, None), entry(3, None)];
        let result = export_pdf(
            &fixture(),
            &edits(pages, vec![note(1, "First line\nSecond résumé")]),
        )
        .unwrap();
        assert_eq!(
            note_text(&split_marks(&result).1[0]),
            "First line\nSecond résumé"
        );
    }

    #[test]
    fn signatures_round_trip_and_redactions_never_become_annotations() {
        let signature = Mark {
            id: "s".into(),
            page: 1,
            color: InkColor::Blue,
            kind: MarkKind::Signature {
                strokes: vec![
                    vec![Point { x: 20.0, y: 20.0 }, Point { x: 60.0, y: 40.0 }],
                    vec![Point { x: 70.0, y: 20.0 }, Point { x: 90.0, y: 30.0 }],
                ],
            },
        };
        let redaction = Mark {
            id: "r".into(),
            page: 1,
            color: InkColor::Blue,
            kind: MarkKind::Redaction {
                rect: Rect {
                    x: 1.0,
                    y: 1.0,
                    width: 5.0,
                    height: 5.0,
                },
            },
        };
        let all_pages = vec![entry(1, None), entry(2, None), entry(3, None)];
        let marks = vec![signature.clone(), redaction];
        let chosen = edits(all_pages, marks);
        assert_eq!(redaction_pages(&chosen)[0].number, 1);
        let result = export_pdf(&fixture(), &chosen).unwrap();
        assert_eq!(annotation_kinds(&result), ["Ink"]);
        assert_eq!(split_marks(&result).1, [signature]);
    }

    #[test]
    fn compressed_marks_reopen_as_editable_annotations() {
        let marks = every_mark();
        let pages = vec![entry(1, None), entry(2, None), entry(3, None)];
        let exported = export_pdf(&fixture(), &edits(pages, marks.clone())).unwrap();
        let mut document = Document::load_mem(&exported).unwrap();
        let mut bytes = Vec::new();
        document.save_modern(&mut bytes).unwrap();
        let (stripped, restored) = split_marks(&bytes);
        assert!(stripped.is_some());
        assert_eq!(restored, marks);
    }

    fn every_mark() -> Vec<Mark> {
        vec![
            Mark {
                id: "h".into(),
                page: 1,
                color: InkColor::Yellow,
                kind: MarkKind::Highlight {
                    text: "Original 你好".into(),
                    rects: vec![Rect {
                        x: 20.0,
                        y: 345.0,
                        width: 80.0,
                        height: 15.0,
                    }],
                },
            },
            Mark {
                id: "p".into(),
                page: 1,
                color: InkColor::Mint,
                kind: MarkKind::Pen {
                    points: vec![Point { x: 20.0, y: 20.0 }, Point { x: 100.0, y: 100.0 }],
                },
            },
            note(1, "A note"),
        ]
    }

    fn annotation_kinds(bytes: &[u8]) -> Vec<String> {
        let document = Document::load_mem(bytes).unwrap();
        let mut kinds = Vec::new();
        for page in document.get_pages().into_values() {
            for item in page_annotations(&document, page).unwrap() {
                let (_, object) = document.dereference(&item).unwrap();
                let subtype = object.as_dict().unwrap().get(b"Subtype").unwrap();
                kinds.push(String::from_utf8_lossy(subtype.as_name().unwrap()).into_owned());
            }
        }
        kinds
    }

    #[test]
    fn marks_become_standard_annotations_any_pdf_app_can_read() {
        let pages = vec![entry(1, None), entry(2, None), entry(3, None)];
        let result = export_pdf(&fixture(), &edits(pages, every_mark())).unwrap();
        assert_eq!(annotation_kinds(&result), ["Highlight", "Ink", "FreeText"]);
    }

    #[test]
    fn saved_marks_come_back_exactly_and_leave_a_clean_file() {
        let pages = vec![entry(1, None), entry(2, None), entry(3, None)];
        let result = export_pdf(&fixture(), &edits(pages, every_mark())).unwrap();
        let (clean, marks) = split_marks(&result);
        assert_eq!(marks, every_mark());
        let clean = clean.expect("marks were removed");
        assert_eq!(annotation_kinds(&clean), Vec::<String>::new());
        assert_eq!(split_marks(&clean), (None, Vec::new()));
        assert_eq!(texts(&clean), texts(&fixture()));
    }

    #[test]
    fn files_without_ethos_marks_are_left_alone() {
        assert_eq!(split_marks(&fixture()), (None, Vec::new()));
        assert_eq!(
            split_marks(b"%PDF-1.7 /EthosMark but damaged"),
            (None, Vec::new())
        );
    }

    #[test]
    fn protected_copies_keep_their_marks_and_stay_openable() {
        let pages = vec![entry(1, None), entry(2, None), entry(3, None)];
        let marked = export_pdf(&fixture(), &edits(pages, vec![note(1, "Kept")])).unwrap();
        let locked = crate::protect_pdf(&marked, "secret").unwrap();
        assert_eq!(split_marks(&locked), (None, Vec::new()));
        let engine = engine();
        let outcome = engine.load(1, locked, Some("secret".into())).unwrap();
        assert!(matches!(outcome, LoadOutcome::Ready { .. }));
    }

    #[test]
    fn annotations_from_other_apps_survive_a_round_trip() {
        let mut document = fixture_document();
        let foreign = document.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![10.into(), 10.into(), 30.into(), 30.into()],
            "Contents" => Object::string_literal("From another app"),
        });
        let first = document.get_pages()[&1];
        set_page_annotations(&mut document, first, vec![foreign.into()]).unwrap();
        let pages = vec![entry(1, None), entry(2, None), entry(3, None)];
        let result =
            export_pdf(&save(&mut document), &edits(pages, vec![note(1, "Mine")])).unwrap();
        assert_eq!(annotation_kinds(&result), ["Text", "FreeText"]);
        let (clean, marks) = split_marks(&result);
        assert_eq!(marks.len(), 1);
        assert_eq!(annotation_kinds(&clean.unwrap()), ["Text"]);
    }

    #[test]
    fn appearances_render_for_viewers_that_do_not_know_ethos() {
        let render = |bytes: Vec<u8>| {
            let engine = engine();
            engine.load(1, bytes, None).unwrap();
            let frame = Frame {
                token: 1,
                source: 1,
                rotation: 0,
                width: 310,
                height: 400,
            };
            engine.render(1, frame).unwrap()
        };
        let pages = vec![entry(1, None), entry(2, None), entry(3, None)];
        let marked = export_pdf(&fixture(), &edits(pages, vec![note(1, "Visible")])).unwrap();
        assert_ne!(render(marked), render(fixture()));
    }

    #[test]
    fn page_content_is_never_rewritten() {
        let pages = vec![entry(1, None), entry(2, None), entry(3, None)];
        let source = fixture();
        let result = export_pdf(&source, &edits(pages, every_mark())).unwrap();
        let contents = |bytes: &[u8]| {
            let document = Document::load_mem(bytes).unwrap();
            let page = document.get_dictionary(document.get_pages()[&1]).unwrap();
            let id = page.get(b"Contents").unwrap().as_reference().unwrap();
            document
                .get_object(id)
                .unwrap()
                .as_stream()
                .unwrap()
                .content
                .clone()
        };
        assert_eq!(contents(&result), contents(&source));
    }

    #[test]
    fn unsupported_characters_fail_clearly_without_changing_the_input() {
        let bytes = fixture();
        let before = bytes.clone();
        let error = export_pdf(
            &bytes,
            &edits(
                vec![entry(1, None), entry(2, None), entry(3, None)],
                vec![note(1, "你好")],
            ),
        )
        .unwrap_err();
        assert!(error.contains("Western European"));
        assert_eq!(bytes, before);
    }

    #[test]
    fn protected_documents_cannot_be_exported() {
        let error =
            export_pdf(&protected_fixture(), &edits(vec![entry(1, None)], vec![])).unwrap_err();
        assert!(error.contains("Protected PDFs"), "{error}");
    }
}
