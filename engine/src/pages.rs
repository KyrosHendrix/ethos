//! Tools that work on whole pages: merging, splitting, extracting, passwords.

use std::collections::BTreeMap;

use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{
    Dictionary, Document, EncryptionState, EncryptionVersion, Object, ObjectId, Permissions,
    dictionary,
};

const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

pub(crate) fn describe(error: lopdf::Error) -> String {
    error.to_string()
}

fn load(bytes: &[u8]) -> Result<Document, String> {
    let document = Document::load_mem(bytes).map_err(describe)?;
    if document.is_encrypted() || document.encryption_state.is_some() {
        return Err("Remove this PDF's password first, then try again.".into());
    }
    Ok(document)
}

pub(crate) fn serialize(document: &mut Document) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    document
        .save_to(&mut output)
        .map_err(|error| error.to_string())?;
    Ok(output)
}

/// Looks up `key` on a page or the nearest ancestor that defines it.
pub(crate) fn inherited(document: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut node = document.get_dictionary(page).ok()?;
    // Parent chains come from the file, so a cycle must not hang the tool.
    for _ in 0..256 {
        if let Ok(value) = node.get(key) {
            return Some(value.clone());
        }
        let parent = node.get(b"Parent").and_then(Object::as_reference).ok()?;
        node = document.get_dictionary(parent).ok()?;
    }
    None
}

/// Joins documents end to end, in order, into one new PDF.
pub fn merge_pdfs(sources: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if sources.is_empty() {
        return Err("Choose at least one PDF.".into());
    }
    let mut merged = Document::with_version("1.7");
    let root = merged.new_object_id();
    let mut next_id = merged.max_id + 1;
    let mut kids: Vec<Object> = Vec::new();
    let mut first_title = None;
    for bytes in sources {
        let mut document = load(bytes)?;
        document.renumber_objects_with(next_id);
        next_id = document.max_id + 1;
        let pages = document.get_pages();
        let page_ids: Vec<ObjectId> = pages.into_values().collect();
        let attributes: Vec<_> = page_ids
            .iter()
            .map(|&page| {
                INHERITABLE
                    .iter()
                    .filter_map(|key| Some((*key, inherited(&document, page, key)?)))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (&page, attributes) in page_ids.iter().zip(attributes) {
            let node = document
                .get_object_mut(page)
                .and_then(Object::as_dict_mut)
                .map_err(describe)?;
            for (key, value) in attributes {
                node.set(key, value);
            }
            node.set("Parent", root);
            // Form widgets point at their source's AcroForm, which does not come along.
            node.remove(b"Annots");
            kids.push(page.into());
        }
        if first_title.is_none() {
            first_title = document
                .trailer
                .get(b"Info")
                .and_then(Object::as_reference)
                .and_then(|info| document.get_dictionary(info))
                .and_then(|info| info.get(b"Title"))
                .ok()
                .cloned();
        }
        let reachable = reachable_from(&document, &page_ids);
        merged.objects.extend(
            document
                .objects
                .into_iter()
                .filter(|(id, _)| reachable.contains_key(id)),
        );
        merged.max_id = merged.max_id.max(next_id);
    }
    merged.objects.insert(
        root,
        dictionary! { "Type" => "Pages", "Count" => kids.len() as i64, "Kids" => kids }.into(),
    );
    let catalog = merged.add_object(dictionary! { "Type" => "Catalog", "Pages" => root });
    merged.trailer.set("Root", catalog);
    let mut info = dictionary! { "Creator" => Object::string_literal("ethos") };
    if let Some(title) = first_title {
        info.set("Title", title);
    }
    let info = merged.add_object(info);
    merged.trailer.set("Info", info);
    serialize(&mut merged)
}

/// Every object the pages need, without following `Parent` back into the old page tree.
fn reachable_from(document: &Document, pages: &[ObjectId]) -> BTreeMap<ObjectId, ()> {
    fn walk(document: &Document, object: &Object, seen: &mut BTreeMap<ObjectId, ()>) {
        match object {
            Object::Reference(id) => {
                if seen.insert(*id, ()).is_none()
                    && let Ok(target) = document.get_object(*id)
                {
                    walk(document, target, seen);
                }
            }
            Object::Array(items) => items.iter().for_each(|item| walk(document, item, seen)),
            Object::Dictionary(node) => walk_node(document, node, seen),
            Object::Stream(stream) => walk_node(document, &stream.dict, seen),
            _ => {}
        }
    }
    fn walk_node(document: &Document, node: &Dictionary, seen: &mut BTreeMap<ObjectId, ()>) {
        for (key, value) in node {
            if key != b"Parent" {
                walk(document, value, seen);
            }
        }
    }
    let mut seen = BTreeMap::new();
    for &page in pages {
        seen.insert(page, ());
        if let Ok(node) = document.get_dictionary(page) {
            walk_node(document, node, &mut seen);
        }
    }
    seen
}

/// A new PDF holding the 1-based `pages`, in the given order.
pub fn extract_pages(bytes: &[u8], pages: &[u32]) -> Result<Vec<u8>, String> {
    let entries = pages
        .iter()
        .map(|&source| ethos_core::PageEntry {
            source,
            rotation: None,
        })
        .collect();
    let edits = ethos_core::Edits {
        pages: entries,
        ..Default::default()
    };
    crate::export_pdf(bytes, &edits)
}

/// Parses a range list such as `1-3, 5, 8-` against a document of `count` pages.
pub fn parse_ranges(text: &str, count: u32) -> Result<Vec<Vec<u32>>, String> {
    let invalid = || format!("Use page numbers from 1 to {count}, like 1-3, 5, 8-{count}.");
    let mut ranges = Vec::new();
    for part in text
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let (start, end) = match part.split_once('-') {
            Some((start, end)) => (start.trim(), end.trim()),
            None => (part, part),
        };
        let number = |text: &str, fallback: u32| {
            if text.is_empty() {
                Ok(fallback)
            } else {
                text.parse::<u32>().map_err(|_| invalid())
            }
        };
        let (start, end) = (number(start, 1)?, number(end, count)?);
        if start == 0 || end > count || start > end {
            return Err(invalid());
        }
        ranges.push((start..=end).collect());
    }
    if ranges.is_empty() {
        return Err(invalid());
    }
    Ok(ranges)
}

/// Pages grouped into consecutive runs of `size`.
pub fn every_n_pages(count: u32, size: u32) -> Vec<Vec<u32>> {
    let pages: Vec<u32> = (1..=count).collect();
    pages
        .chunks(size.max(1) as usize)
        .map(<[u32]>::to_vec)
        .collect()
}

/// A copy that asks for `password` to open.
pub fn protect_pdf(bytes: &[u8], password: &str) -> Result<Vec<u8>, String> {
    if password.is_empty() {
        return Err("Enter a password.".into());
    }
    let mut document = load(bytes)?;
    if document.trailer.get(b"ID").is_err() {
        let mut seed = [0_u8; 16];
        getrandom::fill(&mut seed).map_err(|error| error.to_string())?;
        let id = Object::String(seed.to_vec(), lopdf::StringFormat::Hexadecimal);
        document.trailer.set("ID", vec![id.clone(), id]);
    }
    let filter: std::sync::Arc<dyn CryptFilter> = std::sync::Arc::new(Aes256CryptFilter);
    let mut key = [0_u8; 32];
    getrandom::fill(&mut key).map_err(|error| error.to_string())?;
    let version = EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: password,
        user_password: password,
        permissions: Permissions::all(),
    };
    let state = EncryptionState::try_from(version).map_err(describe)?;
    document.encrypt(&state).map_err(describe)?;
    serialize(&mut document)
}

/// A copy of a protected PDF that opens without a password.
pub fn remove_password(bytes: &[u8], password: &str) -> Result<Vec<u8>, String> {
    let mut document = Document::load_mem_with_password(bytes, password)
        .map_err(|_| "That password did not unlock this PDF.".to_owned())?;
    if document.is_encrypted() {
        document.decrypt(password).map_err(describe)?;
    }
    document.encryption_state = None;
    document.trailer.remove(b"Encrypt");
    serialize(&mut document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::tests::{fixture, inherited_fixture};
    use crate::reader::tests::engine;
    use ethos_core::LoadOutcome;

    fn texts(bytes: Vec<u8>, password: Option<&str>) -> Vec<String> {
        let engine = engine();
        let outcome = engine.load(1, bytes, password.map(str::to_owned)).unwrap();
        let LoadOutcome::Ready { pages, .. } = outcome else {
            panic!("password prompt");
        };
        (1..=pages.len() as u32)
            .map(|page| {
                let runs = engine.text_runs(1, page).unwrap();
                runs.into_iter()
                    .map(|run| run.text)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    #[test]
    fn merging_keeps_order_and_inherited_geometry() {
        let merged = merge_pdfs(&[fixture(), inherited_fixture(), fixture()]).unwrap();
        let content = texts(merged.clone(), None);
        assert_eq!(content.len(), 8);
        assert_eq!(content[0], "Original page 1");
        assert_eq!(content[3], "Inherited resource content");
        assert_eq!(content[7], "Original page 3");
        let document = Document::load_mem(&merged).unwrap();
        let page = document.get_pages()[&4];
        assert_eq!(
            inherited(&document, page, b"Rotate")
                .unwrap()
                .as_i64()
                .unwrap(),
            90
        );
        assert!(inherited(&document, page, b"CropBox").is_some());
    }

    #[test]
    fn merging_refuses_locked_files_with_a_useful_message() {
        let locked = protect_pdf(&fixture(), "secret").unwrap();
        assert!(merge_pdfs(&[locked]).unwrap_err().contains("password"));
        assert!(merge_pdfs(&[]).is_err());
    }

    #[test]
    fn extraction_keeps_only_the_requested_pages_in_order() {
        let result = extract_pages(&fixture(), &[3, 1]).unwrap();
        assert_eq!(texts(result, None), ["Original page 3", "Original page 1"]);
    }

    #[test]
    fn ranges_cover_lists_and_open_ends() {
        assert_eq!(
            parse_ranges("1-2, 4, 6-", 7).unwrap(),
            vec![vec![1, 2], vec![4], vec![6, 7]]
        );
        assert_eq!(parse_ranges("-2", 7).unwrap(), vec![vec![1, 2]]);
        for bad in ["", "0", "8", "3-2", "a"] {
            assert!(parse_ranges(bad, 7).is_err(), "{bad}");
        }
    }

    #[test]
    fn splitting_every_n_pages_keeps_the_remainder() {
        assert_eq!(every_n_pages(5, 2), vec![vec![1, 2], vec![3, 4], vec![5]]);
    }

    #[test]
    fn a_protected_copy_needs_its_password_and_it_can_be_removed() {
        let locked = protect_pdf(&fixture(), "open sesame").unwrap();
        {
            let engine = engine();
            assert_eq!(
                engine.load(1, locked.clone(), None).unwrap(),
                LoadOutcome::Password { incorrect: false }
            );
        }
        assert_eq!(
            texts(locked.clone(), Some("open sesame"))[0],
            "Original page 1"
        );
        assert!(remove_password(&locked, "wrong").is_err());
        let open = remove_password(&locked, "open sesame").unwrap();
        assert_eq!(texts(open, None)[1], "Original page 2");
    }
}
