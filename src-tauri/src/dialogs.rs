//! Native file dialogs, replaced with scripted answers for the end-to-end test.

use std::path::PathBuf;

use tauri::AppHandle;

use crate::Answer;

/// A named file type offered by a dialog.
pub struct Filter {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}

const PDF_FILTER: Filter = Filter {
    name: "PDF documents",
    extensions: &["pdf"],
};

#[cfg(not(feature = "e2e"))]
mod native {
    use tauri::async_runtime::spawn_blocking;
    use tauri::{AppHandle, Manager};
    use tauri_plugin_dialog::{DialogExt, FileDialogBuilder, FilePath};

    use super::{Answer, Filter, PathBuf};

    fn builder(
        app: &AppHandle,
        title: &str,
        filter: Option<&Filter>,
    ) -> FileDialogBuilder<tauri::Wry> {
        let mut dialog = app.dialog().file().set_title(title);
        if let Some(filter) = filter {
            dialog = dialog.add_filter(filter.name, filter.extensions);
        }
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        dialog
    }

    fn into_path(path: FilePath) -> Answer<PathBuf> {
        path.into_path().map_err(|error| error.to_string())
    }

    pub async fn pick_one(
        app: &AppHandle,
        title: &str,
        filter: &Filter,
    ) -> Answer<Option<PathBuf>> {
        let dialog = builder(app, title, Some(filter));
        let picked = spawn_blocking(move || dialog.blocking_pick_file())
            .await
            .map_err(|error| error.to_string())?;
        picked.map(into_path).transpose()
    }

    pub async fn pick_many(app: &AppHandle, title: &str, filter: &Filter) -> Answer<Vec<PathBuf>> {
        let dialog = builder(app, title, Some(filter));
        let picked = spawn_blocking(move || dialog.blocking_pick_files())
            .await
            .map_err(|error| error.to_string())?;
        picked
            .unwrap_or_default()
            .into_iter()
            .map(into_path)
            .collect()
    }

    pub async fn pick_folder(app: &AppHandle, title: &str) -> Answer<Option<PathBuf>> {
        let dialog = builder(app, title, None);
        let picked = spawn_blocking(move || dialog.blocking_pick_folder())
            .await
            .map_err(|error| error.to_string())?;
        picked.map(into_path).transpose()
    }

    pub async fn pick_destination(
        app: &AppHandle,
        suggested: String,
        filter: &Filter,
        title: &str,
    ) -> Answer<Option<PathBuf>> {
        let dialog = builder(app, title, Some(filter)).set_file_name(suggested);
        let picked = spawn_blocking(move || dialog.blocking_save_file())
            .await
            .map_err(|error| error.to_string())?;
        picked.map(into_path).transpose()
    }
}

/// Reads the answers the end-to-end test wrote. An empty file cancels the dialog.
#[cfg(feature = "e2e")]
mod native {
    use tauri::AppHandle;

    use super::{Answer, Filter, PathBuf};
    use crate::data_dir;

    fn script(app: &AppHandle, file: &str) -> Answer<Vec<PathBuf>> {
        let answer = std::fs::read_to_string(data_dir(app).join(file))
            .map_err(|_| format!("Native dialog called without a script: {file}"))?;
        Ok(answer
            .lines()
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect())
    }

    pub async fn pick_one(
        app: &AppHandle,
        _title: &str,
        _filter: &Filter,
    ) -> Answer<Option<PathBuf>> {
        Ok(script(app, "open-dialog.txt")?.into_iter().next())
    }

    pub async fn pick_many(
        app: &AppHandle,
        _title: &str,
        _filter: &Filter,
    ) -> Answer<Vec<PathBuf>> {
        script(app, "open-dialog.txt")
    }

    pub async fn pick_folder(app: &AppHandle, _title: &str) -> Answer<Option<PathBuf>> {
        Ok(script(app, "folder-dialog.txt")?.into_iter().next())
    }

    pub async fn pick_destination(
        app: &AppHandle,
        suggested: String,
        _filter: &Filter,
        _title: &str,
    ) -> Answer<Option<PathBuf>> {
        std::fs::write(data_dir(app).join("save-suggestion.txt"), suggested)
            .map_err(|error| error.to_string())?;
        Ok(script(app, "save-dialog.txt")?.into_iter().next())
    }
}

pub use native::{pick_destination, pick_folder};

pub async fn pick_pdf(app: &AppHandle) -> Answer<Option<PathBuf>> {
    native::pick_one(app, "Open a PDF", &PDF_FILTER).await
}

pub async fn pick_files(app: &AppHandle, filter: &Filter, title: &str) -> Answer<Vec<PathBuf>> {
    native::pick_many(app, title, filter).await
}
