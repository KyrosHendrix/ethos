//! Interactive form fields: reading them for the reader, and writing filled values.

use std::collections::{HashMap, HashSet};

use ethos_core::{ChoiceOption, FieldKind, FieldValue, FormField, Rect, encode_win_ansi};
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

/// Decodes choice labels and export values, including indirect option strings.
fn choices(document: &Document, node: &Dictionary) -> Vec<ChoiceOption> {
    let Some(Object::Array(options)) = inherited(document, node, b"Opt") else {
        return Vec::new();
    };
    options
        .iter()
        .filter_map(|option| {
            let option = document.dereference(option).ok()?.1;
            let mut holder = Dictionary::new();
            let (value, label) = match option {
                Object::Array(pair) => (pair.first()?, pair.get(1)?),
                other => (other, other),
            };
            holder.set("V", document.dereference(value).ok()?.1.clone());
            let value = text_of(&holder, b"V")?;
            holder.set("V", document.dereference(label).ok()?.1.clone());
            Some(ChoiceOption {
                value,
                label: text_of(&holder, b"V")?,
            })
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
    let Ok(document) = Document::load_mem(bytes) else {
        return Vec::new();
    };
    if document
        .catalog()
        .and_then(|catalog| catalog.get(b"AcroForm"))
        .is_err()
    {
        return Vec::new();
    }
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

/// Unlinks fields from pages rebuilt by redaction, including shared widgets and empty groups.
/// Refuses inherited values on retained ancestors before mutating the document: their
/// appearances, rich text, and choice data may contain copies that cannot be safely separated.
pub(crate) fn remove_page_fields(
    document: &mut Document,
    pages: &[u32],
    progress: &impl Fn(u32, u32) -> bool,
) -> Result<(), String> {
    let check = || {
        if progress(0, pages.len() as u32) {
            Ok(())
        } else {
            Err(crate::CANCELLED.to_owned())
        }
    };
    check()?;
    if pages.is_empty() {
        return Ok(());
    }
    if let Ok(form) = document
        .catalog()
        .and_then(|catalog| catalog.get(b"AcroForm"))
        && let Ok((_, Object::Dictionary(form))) = document.dereference(form)
        && form.has(b"XFA")
    {
        return Err("Redaction is not supported for XFA forms. Nothing was saved.".into());
    }
    let mut removed = HashSet::new();
    let mut inherited_values = HashSet::new();
    for (number, id) in document.get_pages() {
        check()?;
        if !pages.contains(&number) {
            continue;
        }
        if let Ok(node) = document.get_dictionary(id)
            && let Ok((_, Object::Array(annotations))) = node
                .get(b"Annots")
                .and_then(|value| document.dereference(value))
        {
            for (index, item) in annotations.iter().enumerate() {
                if index % 256 == 0 {
                    check()?;
                }
                if let Ok(id) = item.as_reference()
                    && document.get_dictionary(id).is_ok_and(|widget| {
                        widget.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Widget")
                    })
                {
                    removed.insert(id);
                    removed.insert(value_holder(document, id));
                    let widget = document.get_dictionary(id).map_err(describe)?;
                    for key in [b"V".as_slice(), b"DV"] {
                        if widget.has(key) {
                            continue;
                        }
                        let mut parent = widget.get(b"Parent").and_then(Object::as_reference).ok();
                        for _ in 0..MAX_DEPTH {
                            let Some(id) = parent else { break };
                            let node = document.get_dictionary(id).map_err(describe)?;
                            if node.has(key) {
                                inherited_values.insert(id);
                            }
                            parent = node.get(b"Parent").and_then(Object::as_reference).ok();
                        }
                        if parent.is_some() {
                            return Err("The form field hierarchy could not be safely redacted. Nothing was saved.".into());
                        }
                    }
                }
            }
        }
    }
    if removed.is_empty() {
        return Ok(());
    }
    loop {
        let before = removed.len();
        for (index, (&id, object)) in document.objects.iter().enumerate() {
            if index % 256 == 0 {
                check()?;
            }
            let Ok(node) = object.as_dict() else { continue };
            let parent_removed = node
                .get(b"Parent")
                .and_then(Object::as_reference)
                .is_ok_and(|parent| removed.contains(&parent));
            let empty_group = node
                .get(b"Kids")
                .and_then(|kids| document.dereference(kids))
                .and_then(|(_, kids)| kids.as_array())
                .is_ok_and(|kids| {
                    !kids.is_empty()
                        && kids
                            .iter()
                            .all(|kid| kid.as_reference().is_ok_and(|id| removed.contains(&id)))
                });
            // Page tree nodes also have Kids and Parent; only form nodes may be removed.
            let page_node = matches!(
                node.get(b"Type").and_then(Object::as_name).ok(),
                Some(b"Page" | b"Pages")
            );
            if (parent_removed || empty_group) && !page_node {
                removed.insert(id);
            }
        }
        if removed.len() == before {
            break;
        }
    }
    if inherited_values.iter().any(|id| !removed.contains(id)) {
        return Err("Redaction cannot safely separate shared inherited values from retained form fields. Nothing was saved.".into());
    }
    let mut updates = Vec::new();
    for (index, (&id, object)) in document.objects.iter().enumerate() {
        if index % 256 == 0 {
            check()?;
        }
        if let Ok(node) = object.as_dict() {
            for key in [b"Annots".as_slice(), b"Kids"] {
                if let Ok((_, Object::Array(items))) =
                    node.get(key).and_then(|value| document.dereference(value))
                {
                    let mut items = items.clone();
                    items.retain(|item| !item.as_reference().is_ok_and(|id| removed.contains(&id)));
                    updates.push((id, key, items));
                }
            }
        }
    }
    for (index, (id, key, items)) in updates.into_iter().enumerate() {
        if index % 256 == 0 {
            check()?;
        }
        document
            .get_object_mut(id)
            .and_then(Object::as_dict_mut)
            .map_err(describe)?
            .set(key, items);
    }
    let form = document
        .catalog()
        .and_then(|catalog| catalog.get(b"AcroForm"))
        .ok()
        .cloned();
    if let Some(form) = form {
        let mut form = document
            .dereference(&form)
            .map_err(describe)?
            .1
            .as_dict()
            .map_err(describe)?
            .clone();
        let mut changed = false;
        if let Some(order) = form
            .get(b"CO")
            .ok()
            .and_then(|order| document.dereference(order).ok())
            .and_then(|(_, order)| order.as_array().ok())
        {
            let count = order.len();
            let order: Vec<_> = order
                .iter()
                .filter(|item| !item.as_reference().is_ok_and(|id| removed.contains(&id)))
                .cloned()
                .collect();
            if order.len() < count {
                changed = true;
                if order.is_empty() {
                    form.remove(b"CO");
                } else {
                    form.set("CO", order);
                }
            }
        }
        // AcroForm and its Fields array may be direct or indirect objects.
        let fields = form
            .get(b"Fields")
            .ok()
            .and_then(|fields| document.dereference(fields).ok())
            .and_then(|(_, fields)| fields.as_array().ok())
            .cloned();
        let mut empty = false;
        if let Some(fields) = fields {
            let count = fields.len();
            let fields: Vec<_> = fields
                .into_iter()
                .filter(|field| !field.as_reference().is_ok_and(|id| removed.contains(&id)))
                .collect();
            if fields.len() < count {
                changed = true;
                empty = fields.is_empty();
                form.set("Fields", fields);
            }
        }
        if empty {
            document
                .catalog_mut()
                .map_err(describe)?
                .remove(b"AcroForm");
        } else if changed {
            document
                .catalog_mut()
                .map_err(describe)?
                .set("AcroForm", form);
        }
    }
    for (index, id) in removed.into_iter().enumerate() {
        if index % 256 == 0 {
            check()?;
        }
        document.objects.remove(&id);
    }
    Ok(())
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
            FieldKind::Choice { options } => {
                let label = options
                    .iter()
                    .find(|option| option.value == value)
                    .map_or(value, |option| option.label.as_str());
                text_appearance(document, field.rect, label, false)
            }
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
                options: ["Small", "Large"]
                    .map(|text| ChoiceOption {
                        value: text.into(),
                        label: text.into()
                    })
                    .to_vec()
            }
        );
        assert_eq!(fields[0].page, 1);
    }

    #[test]
    fn documents_without_forms_have_no_fields() {
        assert!(read_fields(&crate::export::tests::fixture()).is_empty());
    }

    #[test]
    fn fields_in_compressed_object_streams_remain_fillable() {
        let mut document = Document::load_mem(&form_fixture()).unwrap();
        let mut compressed = Vec::new();
        document.save_modern(&mut compressed).unwrap();
        assert_eq!(read_fields(&compressed), read_fields(&form_fixture()));
    }

    #[test]
    fn choice_labels_and_export_values_round_trip_separately() {
        let mut document = Document::load_mem(&form_fixture()).unwrap();
        let choice = all_widgets(&document)
            .into_iter()
            .find(|id| {
                text_of(document.get_dictionary(*id).unwrap(), b"T").as_deref() == Some("size")
            })
            .unwrap();
        let export_value = document.add_object(Object::string_literal("L"));
        let display_label = document.add_object(Object::string_literal("Large"));
        document
            .get_object_mut(choice)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set(
                "Opt",
                vec![Object::Array(vec![
                    export_value.into(),
                    display_label.into(),
                ])],
            );
        let values = [FieldValue {
            name: "size".into(),
            value: "L".into(),
        }];
        apply_fields(&mut document, &values).unwrap();
        let bytes = serialize(&mut document).unwrap();
        let field = read_fields(&bytes)
            .into_iter()
            .find(|field| field.name == "size")
            .unwrap();
        assert_eq!(field.value, "L");
        assert_eq!(
            field.kind,
            FieldKind::Choice {
                options: vec![ChoiceOption {
                    value: "L".into(),
                    label: "Large".into()
                }]
            }
        );
        let appearance = document
            .get_dictionary(choice)
            .unwrap()
            .get(b"AP")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"N")
            .unwrap()
            .as_reference()
            .unwrap();
        let stream = document
            .get_object(appearance)
            .unwrap()
            .as_stream()
            .unwrap();
        let content = stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone());
        assert!(
            String::from_utf8(content)
                .unwrap()
                .contains("<4c61726765> Tj")
        );
    }

    #[test]
    fn redaction_unlinks_shared_widgets_and_preserves_unrelated_fields() {
        let mut document = fixture_document();
        let pages = document.get_pages();
        let group = document.new_object_id();
        let sensitive = document.new_object_id();
        let appearance = document.add_object(Stream::new(
            Dictionary::new(),
            b"secret appearance".to_vec(),
        ));
        let mut widgets = Vec::new();
        for page in [pages[&1], pages[&2]] {
            widgets.push(document.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Widget", "Parent" => sensitive, "P" => page,
                "Rect" => vec![30.into(), 300.into(), 230.into(), 324.into()],
                "AP" => dictionary! { "N" => appearance },
            }));
        }
        let retained = document.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "Parent" => group,
            "T" => Object::string_literal("retained"), "V" => Object::string_literal("Public"), "P" => pages[&2],
            "Rect" => vec![30.into(), 260.into(), 230.into(), 284.into()],
        });
        let kids = document.add_object(Object::Array(
            widgets.iter().map(|id| Object::Reference(*id)).collect(),
        ));
        document.objects.insert(sensitive, dictionary! {
            "T" => Object::string_literal("sensitive"), "FT" => "Tx", "Parent" => group,
            "V" => Object::string_literal("Secret"), "DV" => Object::string_literal("Secret"), "Kids" => kids,
        }.into());
        document.objects.insert(group, dictionary! { "T" => Object::string_literal("group"), "Kids" => vec![sensitive.into(), retained.into()] }.into());
        document
            .get_object_mut(pages[&1])
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Annots", vec![widgets[0].into()]);
        let annots = document.add_object(Object::Array(vec![widgets[1].into(), retained.into()]));
        document
            .get_object_mut(pages[&2])
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Annots", annots);
        let fields = document.add_object(Object::Array(vec![group.into()]));
        document.catalog_mut().unwrap().set(
            "AcroForm",
            dictionary! { "Fields" => fields, "CO" => vec![sensitive.into(), retained.into()] },
        );
        remove_page_fields(&mut document, &[1], &|_, _| true).unwrap();
        document.prune_objects();
        let bytes = serialize(&mut document).unwrap();
        let fields = read_fields(&bytes);
        assert_eq!(fields.len(), 1);
        assert_eq!(
            (
                fields[0].name.as_str(),
                fields[0].value.as_str(),
                fields[0].page
            ),
            ("group.retained", "Public", 2)
        );
        let saved = Document::load_mem(&bytes).unwrap();
        assert_eq!(
            saved
                .catalog()
                .unwrap()
                .get(b"AcroForm")
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"CO")
                .unwrap()
                .as_array()
                .unwrap(),
            &vec![Object::Reference(retained)]
        );
        assert!(!saved.objects.contains_key(&sensitive));
        assert!(!saved.objects.contains_key(&appearance));
        for widget in widgets {
            assert!(!saved.objects.contains_key(&widget));
        }
    }

    #[test]
    fn redaction_rejects_shared_inherited_values_without_changing_fields() {
        let mut document = fixture_document();
        let pages = document.get_pages();
        let group = document.new_object_id();
        let appearance = document.add_object(Stream::new(
            Dictionary::new(),
            b"BT (Secret) Tj ET".to_vec(),
        ));
        let rect = vec![30.into(), 300.into(), 230.into(), 324.into()];
        let sensitive = document.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Parent" => group, "P" => pages[&1],
            "T" => Object::string_literal("sensitive"), "Rect" => Object::Array(rect.clone()),
        });
        let retained = document.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Parent" => group, "P" => pages[&2],
            "T" => Object::string_literal("retained"), "Rect" => rect,
            "AP" => dictionary! { "N" => appearance },
        });
        document.objects.insert(group, dictionary! {
            "FT" => "Tx", "V" => Object::string_literal("Secret"), "DV" => Object::string_literal("Default secret"),
            "RV" => Object::string_literal("<p>Secret</p>"), "Opt" => vec![Object::string_literal("Secret")],
            "Kids" => vec![sensitive.into(), retained.into()],
        }.into());
        document
            .get_object_mut(pages[&1])
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Annots", vec![sensitive.into()]);
        document
            .get_object_mut(pages[&2])
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Annots", vec![retained.into()]);
        document
            .catalog_mut()
            .unwrap()
            .set("AcroForm", dictionary! { "Fields" => vec![group.into()] });
        for own_value in [None, Some("Public")] {
            let mut candidate = document.clone();
            if let Some(value) = own_value {
                candidate
                    .get_object_mut(retained)
                    .unwrap()
                    .as_dict_mut()
                    .unwrap()
                    .set("V", Object::string_literal(value));
            }
            let original = candidate.objects.clone();
            let error = remove_page_fields(&mut candidate, &[1], &|_, _| true).unwrap_err();
            assert!(error.contains("shared inherited values"));
            assert_eq!(candidate.objects, original);
        }
        remove_page_fields(&mut document, &[1, 2], &|_, _| true).unwrap();
        document.prune_objects();
        let saved = Document::load_mem(&serialize(&mut document).unwrap()).unwrap();
        assert!(!saved.catalog().unwrap().has(b"AcroForm"));
        for id in [sensitive, retained, group, appearance] {
            assert!(!saved.objects.contains_key(&id));
        }
    }

    #[test]
    fn redaction_rejects_xfa_data_and_leaves_the_source_unchanged() {
        let mut document = Document::load_mem(&form_fixture()).unwrap();
        let xfa = document.add_object(Stream::new(
            Dictionary::new(),
            b"<data><name>Secret</name></data>".to_vec(),
        ));
        let form = document
            .catalog()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_reference()
            .unwrap();
        document
            .get_object_mut(form)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("XFA", xfa);
        let original = document.objects.clone();
        assert!(
            remove_page_fields(&mut document, &[1], &|_, _| true)
                .unwrap_err()
                .contains("XFA")
        );
        assert_eq!(document.objects, original);
    }

    #[test]
    fn redaction_preserves_form_metadata_when_no_fields_are_removed() {
        for fields in [None, Some(Object::Null), Some(Object::Array(Vec::new()))] {
            let mut document = fixture_document();
            let mut form = dictionary! { "NeedAppearances" => true };
            if let Some(fields) = fields {
                form.set("Fields", fields);
            }
            document
                .catalog_mut()
                .unwrap()
                .set("AcroForm", form.clone());
            remove_page_fields(&mut document, &[], &|_, _| true).unwrap();
            remove_page_fields(&mut document, &[1], &|_, _| true).unwrap();
            assert_eq!(
                document
                    .catalog()
                    .unwrap()
                    .get(b"AcroForm")
                    .unwrap()
                    .as_dict()
                    .unwrap(),
                &form
            );
        }
    }

    /// Cancelling during discovery must stop cleanup without touching the form.
    #[test]
    fn field_cleanup_honors_cancellation_before_and_during_scans() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut document = Document::load_mem(&form_fixture()).unwrap();
        let original = document.objects.clone();
        assert_eq!(
            remove_page_fields(&mut document, &[1], &|_, _| false).unwrap_err(),
            crate::CANCELLED
        );
        let calls = AtomicUsize::new(0);
        let result = remove_page_fields(&mut document, &[1], &|_, _| {
            calls.fetch_add(1, Ordering::Relaxed) < 2
        });
        assert_eq!(result.unwrap_err(), crate::CANCELLED);
        assert!(calls.load(Ordering::Relaxed) >= 3);
        assert_eq!(document.objects, original);
    }

    /// Calculation order retains only surviving fields, including indirect CO arrays.
    #[test]
    fn redaction_removes_deleted_fields_from_calculation_order() {
        let mut base = Document::load_mem(&form_fixture()).unwrap();
        let pages = base.get_pages();
        let widgets = all_widgets(&base);
        let retained = widgets[0];
        base.get_object_mut(pages[&1])
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set(
                "Annots",
                widgets[1..]
                    .iter()
                    .map(|id| Object::Reference(*id))
                    .collect::<Vec<_>>(),
            );
        base.get_object_mut(pages[&2])
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Annots", vec![retained.into()]);
        base.get_object_mut(retained)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("P", pages[&2]);
        let form_id = base
            .catalog()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_reference()
            .unwrap();
        for keep_order in [false, true] {
            let mut document = base.clone();
            let mut order = vec![widgets[1].into()];
            if keep_order {
                order.push(retained.into());
            }
            let order_id = document.add_object(Object::Array(order));
            document
                .get_object_mut(form_id)
                .unwrap()
                .as_dict_mut()
                .unwrap()
                .set("CO", order_id);
            remove_page_fields(&mut document, &[1], &|_, _| true).unwrap();
            document.prune_objects();
            let bytes = serialize(&mut document).unwrap();
            let saved = Document::load_mem(&bytes).unwrap();
            let form = saved
                .catalog()
                .unwrap()
                .get(b"AcroForm")
                .unwrap()
                .as_dict()
                .unwrap();
            if keep_order {
                assert_eq!(
                    form.get(b"CO").unwrap().as_array().unwrap(),
                    &vec![Object::Reference(retained)]
                );
            } else {
                assert!(!form.has(b"CO"));
            }
            assert_eq!(read_fields(&bytes).len(), 1);
            assert_eq!(read_fields(&bytes)[0].value, "Ada");
        }
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
