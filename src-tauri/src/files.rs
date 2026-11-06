//! Bounded file reads, the recent list and copy writes. No path here comes from a trusted source.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ethos_core::{MAX_FILE_SIZE, RecentFile};
use rusty_leveldb::{DB, LdbIterator, Options};
use same_file::is_same_file;
use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 8;

/// A recent document. The file format matches earlier ethos releases.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub opened_at: f64,
}

impl Recent {
    pub fn summary(&self) -> RecentFile {
        RecentFile {
            id: self.id.clone(),
            name: self.name.clone(),
            opened_at: self.opened_at,
        }
    }
}

pub struct Source {
    pub path: PathBuf,
}

pub struct Opened {
    pub name: String,
    pub bytes: Vec<u8>,
    pub source: Source,
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

pub fn read_pdf(path: &Path) -> Result<Opened, String> {
    const TOO_LARGE: &str = "Choose a PDF smaller than 200 MB.";
    if !is_pdf(path) {
        return Err("Choose a PDF file.".into());
    }
    let info = fs::metadata(path).map_err(|error| error.to_string())?;
    if !info.is_file() || info.len() > MAX_FILE_SIZE {
        return Err(TOO_LARGE.into());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_FILE_SIZE {
        return Err(TOO_LARGE.into());
    }
    let head = &bytes[..bytes.len().min(1024)];
    if !head.windows(5).any(|part| part == b"%PDF-") {
        return Err("This file does not contain a valid PDF header.".into());
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or("Choose a PDF file.")?;
    let path = dunce::canonicalize(path).map_err(|error| error.to_string())?;
    Ok(Opened {
        name,
        bytes,
        source: Source { path },
    })
}

pub fn remember(recent: &mut Vec<Recent>, opened: &Opened) {
    let path = &opened.source.path;
    let id = recent
        .iter()
        .find(|item| item.path == *path)
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), |item| item.id.clone());
    recent.retain(|item| item.path != *path);
    let opened_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_millis() as f64);
    recent.insert(
        0,
        Recent {
            id,
            name: opened.name.clone(),
            path: path.clone(),
            opened_at,
        },
    );
    recent.truncate(MAX_RECENT);
}

pub fn write_recents(directory: &Path, recent: &[Recent]) -> Result<(), String> {
    let text = serde_json::to_string(recent).map_err(|error| error.to_string())?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let target = directory.join("recent.json");
    let temporary = directory.join("recent.json.tmp");
    fs::write(&temporary, text).map_err(|error| error.to_string())?;
    fs::rename(temporary, target).map_err(|error| error.to_string())
}

/// A missing or damaged recent list starts empty; individual bad entries are dropped.
pub fn read_recents(directory: &Path) -> Vec<Recent> {
    fs::read_to_string(directory.join("recent.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<serde_json::Value>>(&text).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .take(MAX_RECENT)
        .collect()
}

/// Finds the `ethos-theme` value in a browser storage database. Packaged releases stored it
/// under `file://`; any other origin is a development run and only used as a fallback.
fn theme_in_database(path: &Path) -> Option<String> {
    const KEY: &[u8] = b"\x00\x01ethos-theme";
    let options = Options {
        create_if_missing: false,
        ..Options::default()
    };
    let mut database = DB::open(path, options).ok()?;
    let mut entries = database.new_iter().ok()?;
    let mut found = None;
    while entries.advance() {
        let Some((key, value)) = entries.current() else {
            continue;
        };
        if !key.ends_with(KEY) {
            continue;
        }
        // The first byte names the encoding; dropping zeros reads UTF-16 and Latin-1 alike.
        let text: String = value
            .iter()
            .skip(1)
            .filter(|byte| **byte != 0)
            .map(|byte| char::from(*byte))
            .collect();
        let packaged = key.starts_with(b"_file://");
        if packaged || found.is_none() {
            found = Some(text);
        }
        if packaged {
            break;
        }
    }
    found.filter(|theme| theme == "dark" || theme == "light")
}

/// The appearance saved by Electron releases, read from their browser storage.
pub fn read_saved_theme(directory: &Path) -> Option<String> {
    let source = directory.join("Local Storage").join("leveldb");
    let entries: Vec<_> = fs::read_dir(source).ok()?.flatten().collect();
    // Opening a database can write recovery files, so the old profile is only ever read.
    let copy = std::env::temp_dir().join(format!("ethos-theme-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&copy).ok()?;
    for entry in entries.iter().filter(|entry| entry.file_name() != "LOCK") {
        fs::copy(entry.path(), copy.join(entry.file_name())).ok()?;
    }
    let theme = theme_in_database(&copy);
    // A leftover temporary copy holds only a preference and the OS cleans it up.
    let _ = fs::remove_dir_all(&copy);
    theme
}

pub fn suggested_copy_name(name: &str) -> String {
    let base = Path::new(name).file_name().map_or_else(
        || name.to_owned(),
        |base| base.to_string_lossy().into_owned(),
    );
    let stem = if base.to_lowercase().ends_with(".pdf") {
        &base[..base.len() - 4]
    } else {
        &base
    };
    format!("{stem} - edited.pdf")
}

/// Atomically writes `bytes` to `destination` unless that would replace an original PDF.
pub fn write_copy(destination: &Path, bytes: &[u8], sources: &[PathBuf]) -> Result<(), String> {
    if !destination.is_absolute() || !is_pdf(destination) {
        return Err("Choose an absolute PDF destination.".into());
    }
    if destination.exists() && !destination.is_file() {
        return Err("A folder uses this name. Choose a different filename.".into());
    }
    let canonical = dunce::canonicalize(destination).unwrap_or_else(|_| destination.to_path_buf());
    let folded = canonical.to_string_lossy().to_lowercase();
    for source in sources {
        if folded == source.to_string_lossy().to_lowercase()
            || is_same_file(destination, source).unwrap_or(false)
        {
            return Err("Choose a different filename to preserve your original PDF.".into());
        }
    }
    let mut temporary = destination.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let temporary = PathBuf::from(temporary);
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .and_then(|mut file| std::io::Write::write_all(&mut file, bytes))
        .and_then(|()| fs::rename(&temporary, destination));
    if let Err(error) = written {
        // The temporary file may never have been created; the write error is what matters.
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

pub fn pdf_argument(mut args: impl Iterator<Item = String>) -> Option<PathBuf> {
    args.find(|arg| arg.to_lowercase().ends_with(".pdf"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ethos-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn rejects_files_that_are_not_pdfs() {
        let folder = directory("reject");
        let text = folder.join("note.txt");
        fs::write(&text, "%PDF-").unwrap();
        assert_eq!(read_pdf(&text).err().unwrap(), "Choose a PDF file.");
        let fake = folder.join("fake.pdf");
        fs::write(&fake, "not a pdf").unwrap();
        assert!(read_pdf(&fake).err().unwrap().contains("valid PDF header"));
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn recents_keep_ids_move_to_the_front_and_cap_at_eight() {
        let folder = directory("recent");
        let mut recent = Vec::new();
        for index in 0..10 {
            let path = folder.join(format!("{index}.pdf"));
            fs::write(&path, "%PDF-1.7").unwrap();
            remember(&mut recent, &read_pdf(&path).unwrap());
        }
        assert_eq!(recent.len(), 8);
        assert_eq!(recent[0].name, "9.pdf");
        let id = recent[3].id.clone();
        let path = recent[3].path.clone();
        remember(&mut recent, &read_pdf(&path).unwrap());
        assert_eq!(recent[0].id, id);
        assert_eq!(recent.len(), 8);
        write_recents(&folder, &recent).unwrap();
        assert_eq!(read_recents(&folder), recent);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn reads_the_recent_list_written_by_earlier_releases() {
        let folder = directory("legacy");
        let legacy = r#"[{"id":"a","name":"a.pdf","path":"C:\\a.pdf","openedAt":1.5},{"id":3}]"#;
        fs::write(folder.join("recent.json"), legacy).unwrap();
        let recent = read_recents(&folder);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].opened_at, 1.5);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn damaged_recent_lists_start_empty() {
        let folder = directory("damaged");
        fs::write(folder.join("recent.json"), "{").unwrap();
        assert!(read_recents(&folder).is_empty());
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn copies_never_replace_an_original() {
        let folder = directory("protect");
        let original = folder.join("Original.pdf");
        fs::write(&original, "source").unwrap();
        let sources = [dunce::canonicalize(&original).unwrap()];
        let same_case = write_copy(&original, b"copy", &sources).unwrap_err();
        assert!(same_case.contains("preserve your original"));
        let other_case = write_copy(&folder.join("ORIGINAL.PDF"), b"copy", &sources).unwrap_err();
        assert!(other_case.contains("preserve your original"));
        let link = folder.join("link.pdf");
        fs::hard_link(&original, &link).unwrap();
        assert!(write_copy(&link, b"copy", &sources).is_err());
        assert_eq!(fs::read(&original).unwrap(), b"source");
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn copies_replace_other_files_and_leave_no_temporary_file() {
        let folder = directory("copy");
        let target = folder.join("copy.pdf");
        fs::write(&target, "old").unwrap();
        write_copy(&target, b"new", &[]).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn copies_refuse_folders_and_relative_paths() {
        let folder = directory("folder");
        let named = folder.join("folder.pdf");
        fs::create_dir(&named).unwrap();
        assert!(
            write_copy(&named, b"x", &[])
                .unwrap_err()
                .contains("A folder")
        );
        assert!(write_copy(Path::new("copy.pdf"), b"x", &[]).is_err());
        assert!(write_copy(&folder.join("copy.txt"), b"x", &[]).is_err());
        fs::remove_dir_all(folder).unwrap();
    }

    fn storage(folder: &Path, entries: &[(&[u8], &[u8])], compact: bool) {
        let path = folder.join("Local Storage").join("leveldb");
        fs::create_dir_all(&path).unwrap();
        let mut database = DB::open(&path, Options::default()).unwrap();
        for (key, value) in entries {
            database.put(key, value).unwrap();
        }
        database.flush().unwrap();
        if compact {
            database.compact_range(b"\x00", b"\xff").unwrap();
        }
        database.close().unwrap();
    }

    const PACKAGED: &[u8] = b"_file://\x00\x01ethos-theme";
    const DEVELOPMENT: &[u8] = b"_http://localhost:5175\x00\x01ethos-theme";

    #[test]
    fn missing_storage_has_no_theme() {
        let folder = directory("no-theme");
        assert_eq!(read_saved_theme(&folder), None);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn reads_the_theme_from_compacted_storage() {
        let folder = directory("compacted");
        storage(&folder, &[(PACKAGED, b"\x01dark")], true);
        let tables = fs::read_dir(folder.join("Local Storage").join("leveldb"))
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "ldb")
            })
            .count();
        assert!(tables > 0, "the fixture must exercise table files");
        assert_eq!(read_saved_theme(&folder).as_deref(), Some("dark"));
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn the_packaged_origin_beats_development_runs() {
        let folder = directory("origins");
        storage(
            &folder,
            &[(PACKAGED, b"\x01light"), (DEVELOPMENT, b"\x01dark")],
            false,
        );
        assert_eq!(read_saved_theme(&folder).as_deref(), Some("light"));
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn development_runs_are_the_fallback() {
        let folder = directory("development");
        storage(
            &folder,
            &[
                (b"_x\x00\x01other", b"\x01dark"),
                (DEVELOPMENT, b"\x01dark"),
            ],
            false,
        );
        assert_eq!(read_saved_theme(&folder).as_deref(), Some("dark"));
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn unknown_theme_values_are_ignored() {
        let folder = directory("unknown");
        storage(&folder, &[(PACKAGED, b"\x01sepia")], false);
        assert_eq!(read_saved_theme(&folder), None);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn suggests_an_edited_copy_name() {
        assert_eq!(suggested_copy_name("Report.PDF"), "Report - edited.pdf");
        assert_eq!(
            suggested_copy_name("C:\\a\\Report.pdf"),
            "Report - edited.pdf"
        );
        assert_eq!(suggested_copy_name("Report"), "Report - edited.pdf");
    }

    #[test]
    fn finds_the_first_pdf_argument() {
        let args = ["--flag", "C:\\a.PDF", "b.pdf"].map(String::from);
        assert_eq!(
            pdf_argument(args.into_iter()),
            Some(PathBuf::from("C:\\a.PDF"))
        );
    }
}
