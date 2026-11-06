use std::collections::HashMap;
use std::fmt::Write;

use ethos_core::{Edits, Mark, MarkKind, NOTE_FONT_SIZE, NOTE_LINE_HEIGHT, encode_win_ansi};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
const HIGHLIGHT_STATE: &str = "EthosHighlight";
const PEN_STATE: &str = "EthosPen";
const NOTE_FONT: &str = "EthosFont";
const HIGHLIGHT_OPACITY: f64 = 0.32;
const PEN_OPACITY: f64 = 0.95;
const PEN_WIDTH: f64 = 1.8;

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

fn resolved_dictionary(document: &Document, value: &Object) -> Result<Dictionary, String> {
    document
        .dereference(value)
        .and_then(|(_, object)| object.as_dict())
        .cloned()
        .map_err(describe)
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

/// The content-stream operators that draw `mark` in PDF page coordinates.
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
                "q /{PEN_STATE} gs {stroke} RG {PEN_WIDTH} w {:.3} {:.3} m",
                first.x, first.y
            )
            .expect("writing to a String");
            for point in rest {
                write!(output, " {:.3} {:.3} l", point.x, point.y).expect("writing to a String");
            }
            output.push_str(" S Q\n");
        }
        MarkKind::Text { point, text } => {
            let fill = ink(color(mark.color.stroke()));
            write!(output, "q {fill} rg BT /{NOTE_FONT} {NOTE_FONT_SIZE} Tf {NOTE_LINE_HEIGHT} TL {:.3} {:.3} Td", point.x, point.y)
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

/// Objects shared by every annotated page of one export.
struct MarkResources {
    highlight: ObjectId,
    pen: ObjectId,
    font: ObjectId,
    save_state: ObjectId,
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
            save_state: document.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec())),
        }
    }
}

/// Appends marks to a page, isolating them from any state its content leaves behind.
fn annotate(
    document: &mut Document,
    page: ObjectId,
    marks: &[&Mark],
    shared: &MarkResources,
) -> Result<(), String> {
    let mut content = String::from("Q\n");
    for mark in marks {
        draw(mark, &mut content)?;
    }
    let drawing = document.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

    let mut resources = match inherited(document, page, b"Resources") {
        Some(value) => resolved_dictionary(document, &value)?,
        None => Dictionary::new(),
    };
    let section = |resources: &Dictionary, key: &[u8]| match resources.get(key) {
        Ok(value) => resolved_dictionary(document, value),
        Err(_) => Ok(Dictionary::new()),
    };
    let mut states = section(&resources, b"ExtGState")?;
    let mut fonts = section(&resources, b"Font")?;
    states.set(HIGHLIGHT_STATE, shared.highlight);
    states.set(PEN_STATE, shared.pen);
    fonts.set(NOTE_FONT, shared.font);
    resources.set("ExtGState", states);
    resources.set("Font", fonts);

    let node = document.get_dictionary(page).map_err(describe)?;
    let existing = match node.get(b"Contents") {
        Ok(Object::Array(streams)) => streams.clone(),
        Ok(stream) => match document.dereference(stream).map_err(describe)?.1 {
            Object::Array(streams) => streams.clone(),
            _ => vec![stream.clone()],
        },
        Err(_) => Vec::new(),
    };
    let mut contents: Vec<Object> = vec![shared.save_state.into()];
    contents.extend(existing);
    contents.push(drawing.into());

    let node = document
        .get_object_mut(page)
        .and_then(Object::as_dict_mut)
        .map_err(describe)?;
    node.set("Resources", resources);
    node.set("Contents", contents);
    Ok(())
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
    for mark in &edits.marks {
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

    fn fixture_document() -> Document {
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
        let LoadOutcome::Ready { pages } = engine.load(1, bytes.to_vec(), None).unwrap() else {
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
        Edits { marks, pages }
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
            let LoadOutcome::Ready { pages } = outcome else {
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
        let content = texts(&result);
        assert!(content[0].contains("Annotation 3"));
        assert!(content[1].contains("Annotation 1"));
        assert!(!content.join(" ").contains("Annotation 2"));
    }

    #[test]
    fn multi_line_notes_keep_every_line() {
        let pages = vec![entry(1, None), entry(2, None), entry(3, None)];
        let result = export_pdf(
            &fixture(),
            &edits(pages, vec![note(1, "First line\nSecond résumé")]),
        )
        .unwrap();
        let content = &texts(&result)[0];
        assert!(
            content.contains("First line") && content.contains("Second résumé"),
            "{content}"
        );
    }

    #[test]
    fn highlight_and_pen_marks_export_a_readable_pdf() {
        let marks = vec![
            Mark {
                id: "h".into(),
                page: 1,
                color: InkColor::Yellow,
                kind: MarkKind::Highlight {
                    text: "Original".into(),
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
                page: 2,
                color: InkColor::Mint,
                kind: MarkKind::Pen {
                    points: vec![Point { x: 20.0, y: 20.0 }, Point { x: 100.0, y: 100.0 }],
                },
            },
        ];
        let pages = vec![entry(1, Some(0)), entry(2, Some(90)), entry(3, Some(0))];
        let result = export_pdf(&fixture(), &edits(pages, marks)).unwrap();
        assert_eq!(Document::load_mem(&result).unwrap().get_pages().len(), 3);
        assert!(texts(&result)[0].contains("Original page 1"));
    }

    #[test]
    fn original_content_cannot_leak_state_into_marks() {
        let mut document = fixture_document();
        let first = document.get_pages()[&1];
        let skewed = document.add_object(Stream::new(
            Dictionary::new(),
            b"q 0.5 0 0 0.5 0 0 cm\n".to_vec(),
        ));
        document
            .get_object_mut(first)
            .and_then(Object::as_dict_mut)
            .unwrap()
            .set("Contents", skewed);
        let result = export_pdf(
            &save(&mut document),
            &edits(
                vec![entry(1, None), entry(2, None), entry(3, None)],
                vec![note(1, "Placed")],
            ),
        )
        .unwrap();
        let restored = Document::load_mem(&result).unwrap();
        let page = restored.get_dictionary(restored.get_pages()[&1]).unwrap();
        let contents = page.get(b"Contents").unwrap().as_array().unwrap();
        assert_eq!(
            contents.len(),
            3,
            "wrapped original between save and restore"
        );
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
