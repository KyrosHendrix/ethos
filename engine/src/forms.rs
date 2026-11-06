//! Interactive form fields: reading them for the reader, and writing filled values.

use std::collections::HashMap;

use ethos_core::{FieldKind, FieldValue, FormField, Rect, encode_win_ansi};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use crate::pages::describe;

const MAX_DEPTH: usize = 32;

fn text_of(node: &Dictionary, key: &[u8]) -> Option<String> {
    let bytes = node.get(key).ok()?.as_str().ok()?;
    Some(match bytes.strip_prefix(&[0xfe, 0xff]) {
        Some(wide) => {
            let units: Vec<u16> = wide
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes(*pair))
                .collect();
            String::from_utf16_lossy(&units)
        }
        None => bytes.iter().map(|byte| char::from(*byte)).collect(),
    })
}

fn inherited<'a>(document: &'a Document, node: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    let mut current = node;
    for _ in 0..MAX_DEPTH {
        if let Ok(value) = current.get(key) {
            return document.dereference(value).ok().map(|(_, value)| value);
        }
        let parent = current.get(b"Parent").and_then(Object::as_reference).ok()?;
        current = document.get_dictionary(parent).ok()?;
    }
    None
}

fn full_name(document: &Document, node: &Dictionary) -> String {
    let mut parts = Vec::new();
    let mut current = Some(node);
    let mut depth = 0;
    while let Some(field) = current.filter(|_| depth < MAX_DEPTH) {
        depth += 1;
        if let Some(part) = text_of(field, b"T") {
            parts.push(part);
        }
        current = field
            .get(b"Parent")
            .and_then(Object::as_reference)
            .ok()
            .and_then(|parent| document.get_dictionary(parent).ok());
    }
    parts.reverse();
    parts.join(".")
}

fn rect_of(document: &Document, node: &Dictionary) -> Option<Rect> {
    let rect = document
        .dereference(node.get(b"Rect").ok()?)
        .ok()?
        .1
        .as_array()
        .ok()?;
    let numbers: Vec<f64> = rect
        .iter()
        .filter_map(|value| {
            value
                .as_float()
                .map(f64::from)
                .or_else(|_| value.as_i64().map(|value| value as f64))
                .ok()
        })
        .collect();
    let [left, bottom, right, top] = numbers[..] else {
        return None;
    };
    Some(Rect {
        x: left.min(right),
        y: bottom.min(top),
        width: (right - left).abs(),
        height: (top - bottom).abs(),
    })
}

/// The name of a checkbox or radio widget's "on" state, from its appearance dictionary.
fn on_state(document: &Document, widget: &Dictionary) -> Option<String> {
    let appearance = document
        .dereference(widget.get(b"AP").ok()?)
        .ok()?
        .1
        .as_dict()
        .ok()?;
    let normal = document
        .dereference(appearance.get(b"N").ok()?)
        .ok()?
        .1
        .as_dict()
        .ok()?;
    normal
        .iter()
        .map(|(key, _)| String::from_utf8_lossy(key).into_owned())
        .find(|key| key != "Off")
}

fn name_value(document: &Document, node: &Dictionary) -> Option<String> {
    match inherited(document, node, b"V")? {
        Object::Name(name) => Some(String::from_utf8_lossy(name).into_owned()),
        Object::String(..) => {
            let mut holder = Dictionary::new();
            holder.set("V", inherited(document, node, b"V")?.clone());
            text_of(&holder, b"V")
        }
        _ => None,
    }
}

fn choices(document: &Document, node: &Dictionary) -> Vec<String> {
    let Some(Object::Array(options)) = inherited(document, node, b"Opt") else {
        return Vec::new();
    };
    options
        .iter()
        .filter_map(|option| {
            let option = document.dereference(option).ok()?.1;
            let mut holder = Dictionary::new();
            match option {
                Object::Array(pair) => holder.set("V", pair.last()?.clone()),
                other => holder.set("V", other.clone()),
            }
            text_of(&holder, b"V")
        })
        .collect()
}

fn widget_field(document: &Document, widget: &Dictionary, page: u32) -> Option<FormField> {
    let kind_name = inherited(document, widget, b"FT")?.as_name().ok()?;
    let flags = inherited(document, widget, b"Ff")
        .and_then(|flags| flags.as_i64().ok())
        .unwrap_or(0);
    if flags & 1 != 0 {
        return None; // Read-only.
    }
    let value = name_value(document, widget).unwrap_or_default();
    let kind = match kind_name {
        b"Tx" => FieldKind::Text {
            multiline: flags & (1 << 12) != 0,
        },
        b"Btn" if flags & (1 << 16) != 0 => return None, // Push button.
        b"Btn" => {
            let on = on_state(document, widget).unwrap_or_else(|| "Yes".to_owned());
            if flags & (1 << 15) != 0 {
                FieldKind::Radio { on }
            } else {
                FieldKind::Check { on }
            }
        }
        b"Ch" => FieldKind::Choice {
            options: choices(document, widget),
        },
        _ => return None,
    };
    Some(FormField {
        name: full_name(document, widget),
        kind,
        page,
        rect: rect_of(document, widget)?,
        value,
    })
}

/// Every fillable widget in the document, by page.
pub fn read_fields(bytes: &[u8]) -> Vec<FormField> {
    if !bytes.windows(8).any(|part| part == b"AcroForm") {
        return Vec::new();
    }
    let Ok(document) = Document::load_mem(bytes) else {
        return Vec::new();
    };
    if document.is_encrypted() || document.encryption_state.is_some() {
        return Vec::new();
    }
    let mut fields = Vec::new();
    for (number, page) in document.get_pages() {
        let Ok(node) = document.get_dictionary(page) else {
            continue;
        };
        let Some(annotations) = node
            .get(b"Annots")
            .ok()
            .and_then(|value| document.dereference(value).ok())
            .and_then(|(_, value)| value.as_array().ok())
        else {
            continue;
        };
        for item in annotations {
            let widget = document
                .dereference(item)
                .ok()
                .and_then(|(_, object)| object.as_dict().ok())
                .filter(|node| {
                    node.get(b"Subtype").and_then(Object::as_name).ok() == Some(&b"Widget"[..])
                });
            if let Some(field) = widget.and_then(|widget| widget_field(&document, widget, number)) {
                fields.push(field);
            }
        }
    }
    fields
}

fn all_widgets(document: &Document) -> Vec<ObjectId> {
    let mut ids = Vec::new();
    for page in document.get_pages().into_values() {
        let annotations = document
            .get_dictionary(page)
            .ok()
            .and_then(|node| node.get(b"Annots").ok())
            .and_then(|value| document.dereference(value).ok())
            .and_then(|(_, value)| value.as_array().ok());
        for item in annotations.into_iter().flatten() {
            if let Ok(id) = item.as_reference() {
                ids.push(id);
            }
        }
    }
    ids
}

/// The object holding a widget's value: its parent when the widget is a bare kid.
fn value_holder(document: &Document, widget: ObjectId) -> ObjectId {
    let node = document.get_dictionary(widget).ok();
    let without_name = node.is_some_and(|node| !node.has(b"T"));
    match node.and_then(|node| node.get(b"Parent").and_then(Object::as_reference).ok()) {
        Some(parent) if without_name => parent,
        _ => widget,
    }
}

fn text_object(text: &str) -> Object {
    let mut bytes = vec![0xfe, 0xff];
    bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    Object::String(bytes, lopdf::StringFormat::Hexadecimal)
}

/// A simple appearance, so viewers that do not regenerate fields still show the value.
fn text_appearance(
    document: &mut Document,
    rect: Rect,
    value: &str,
    multiline: bool,
) -> Option<ObjectId> {
    let size = if multiline {
        11.0
    } else {
        (rect.height * 0.62).clamp(8.0, 12.0)
    };
    let mut content = format!("/Tx BMC q BT /Helv {size:.2} Tf 0 g ");
    let first = if multiline {
        rect.height - size - 3.0
    } else {
        (rect.height - size) / 2.0 + 1.5
    };
    content.push_str(&format!("{} TL 3 {first:.2} Td", size * 1.2));
    for (index, line) in value.split('\n').enumerate() {
        let encoded = encode_win_ansi(line)?;
        let hex: String = encoded.iter().map(|byte| format!("{byte:02x}")).collect();
        content.push_str(if index == 0 { " " } else { " T* " });
        content.push_str(&format!("<{hex}> Tj"));
    }
    content.push_str(" ET Q EMC\n");
    let font = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let form = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), Object::Real(rect.width as f32), Object::Real(rect.height as f32)],
            "Resources" => dictionary! { "Font" => dictionary! { "Helv" => font } },
        },
        content.into_bytes(),
    );
    Some(document.add_object(form))
}

/// Writes filled values into the document's fields and their widgets.
pub fn apply_fields(document: &mut Document, values: &[FieldValue]) -> Result<(), String> {
    if values.is_empty() {
        return Ok(());
    }
    let wanted: HashMap<&str, &str> = values
        .iter()
        .map(|value| (value.name.as_str(), value.value.as_str()))
        .collect();
    for widget in all_widgets(document) {
        let Some(field) = document
            .get_dictionary(widget)
            .ok()
            .filter(|node| {
                node.get(b"Subtype").and_then(Object::as_name).ok() == Some(&b"Widget"[..])
            })
            .and_then(|node| widget_field(document, node, 0))
        else {
            continue;
        };
        let Some(value) = wanted.get(field.name.as_str()).copied() else {
            continue;
        };
        let holder = value_holder(document, widget);
        let stored: Object = match &field.kind {
            FieldKind::Text { .. } | FieldKind::Choice { .. } => text_object(value),
            FieldKind::Check { .. } | FieldKind::Radio { .. } => {
                Object::Name(if value.is_empty() {
                    b"Off".to_vec()
                } else {
                    value.as_bytes().to_vec()
                })
            }
        };
        document
            .get_object_mut(holder)
            .and_then(Object::as_dict_mut)
            .map_err(describe)?
            .set("V", stored);
        let appearance = match &field.kind {
            FieldKind::Text { multiline } => {
                text_appearance(document, field.rect, value, *multiline)
            }
            FieldKind::Choice { .. } => text_appearance(document, field.rect, value, false),
            _ => None,
        };
        let node = document
            .get_object_mut(widget)
            .and_then(Object::as_dict_mut)
            .map_err(describe)?;
        match &field.kind {
            FieldKind::Check { on } | FieldKind::Radio { on } => {
                let state = if value == on {
                    on.as_bytes().to_vec()
                } else {
                    b"Off".to_vec()
                };
                node.set("AS", Object::Name(state));
            }
            _ => match appearance {
                Some(form) => node.set("AP", dictionary! { "N" => form }),
                None => drop(node.remove(b"AP")),
            },
        }
    }
    let catalog = document
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(describe)?;
    let form = document
        .get_dictionary(catalog)
        .and_then(|catalog| catalog.get(b"AcroForm"))
        .ok()
        .cloned();
    if let Some(Object::Reference(id)) = form {
        document
            .get_object_mut(id)
            .and_then(Object::as_dict_mut)
            .map_err(describe)?
            .set("NeedAppearances", true);
    } else if let Some(mut form) = form.and_then(|form| form.as_dict().ok().cloned()) {
        form.set("NeedAppearances", true);
        document
            .get_object_mut(catalog)
            .and_then(Object::as_dict_mut)
            .map_err(describe)?
            .set("AcroForm", form);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::export::tests::fixture_document;
    use crate::export_pdf;
    use crate::pages::serialize;
    use ethos_core::{Edits, PageEntry};

    pub(crate) fn form_fixture() -> Vec<u8> {
        let mut document = fixture_document();
        let first = document.get_pages()[&1];
        let rect = |y: i64| vec![30.into(), y.into(), 230.into(), (y + 24).into()];
        let on = document.add_object(Stream::new(Dictionary::new(), b"q Q\n".to_vec()));
        let off = document.add_object(Stream::new(Dictionary::new(), b"q Q\n".to_vec()));
        let text = document.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
            "T" => Object::string_literal("name"), "V" => Object::string_literal("Ada"),
            "Rect" => rect(300), "P" => first,
        });
        let check = document.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn",
            "T" => Object::string_literal("agree"), "V" => "Off", "AS" => "Off",
            "Rect" => rect(260), "P" => first,
            "AP" => dictionary! { "N" => dictionary! { "Accepted" => on, "Off" => off } },
        });
        let choice = document.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Ch", "Ff" => 1_i64 << 17,
            "T" => Object::string_literal("size"), "Rect" => rect(220), "P" => first,
            "Opt" => vec![Object::string_literal("Small"), Object::string_literal("Large")],
        });
        let locked = document.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "Ff" => 1,
            "T" => Object::string_literal("locked"), "Rect" => rect(180), "P" => first,
        });
        document
            .get_object_mut(first)
            .and_then(Object::as_dict_mut)
            .unwrap()
            .set(
                "Annots",
                vec![text.into(), check.into(), choice.into(), locked.into()],
            );
        let fields = vec![text.into(), check.into(), choice.into(), locked.into()];
        let form = document.add_object(dictionary! { "Fields" => fields });
        document.catalog_mut().unwrap().set("AcroForm", form);
        serialize(&mut document).unwrap()
    }

    #[test]
    fn fillable_fields_are_listed_and_read_only_ones_are_not() {
        let fields = read_fields(&form_fixture());
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].name, "name");
        assert_eq!(fields[0].value, "Ada");
        assert_eq!(fields[0].kind, FieldKind::Text { multiline: false });
        assert_eq!(
            fields[1].kind,
            FieldKind::Check {
                on: "Accepted".into()
            }
        );
        assert_eq!(
            fields[2].kind,
            FieldKind::Choice {
                options: vec!["Small".into(), "Large".into()]
            }
        );
        assert_eq!(fields[0].page, 1);
    }

    #[test]
    fn documents_without_forms_have_no_fields() {
        assert!(read_fields(&crate::export::tests::fixture()).is_empty());
    }

    #[test]
    fn filled_values_are_written_into_the_saved_copy() {
        let value = |name: &str, value: &str| FieldValue {
            name: name.into(),
            value: value.into(),
        };
        let edits = Edits {
            pages: (1..=3)
                .map(|source| PageEntry {
                    source,
                    rotation: None,
                })
                .collect(),
            fields: vec![
                value("name", "Grace Hopper"),
                value("agree", "Accepted"),
                value("size", "Large"),
            ],
            ..Edits::default()
        };
        let result = export_pdf(&form_fixture(), &edits).unwrap();
        let fields = read_fields(&result);
        assert_eq!(fields[0].value, "Grace Hopper");
        assert_eq!(fields[1].value, "Accepted");
        assert_eq!(fields[2].value, "Large");
    }
}
