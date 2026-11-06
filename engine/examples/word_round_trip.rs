//! Converts every `.docx` or `.dotx` in a folder to PDF, as a robustness check against files
//! written by real Word. Each PDF and a picture of its first page land beside the output.
//!
//! `cargo run -p ethos-engine --example word_round_trip -- <input folder> <output folder>`

use std::path::{Path, PathBuf};

use ethos_engine::{Engine, document_images, page_count, word_to_pdf};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = &args[..] else {
        return Err("Expected an input folder and an output folder.".into());
    };
    let library = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src-tauri/pdfium/pdfium.dll");
    let engine = Engine::start(library);
    let entries = std::fs::read_dir(input).map_err(|error| error.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|extension| extension == "docx" || extension == "dotx")
        {
            continue;
        }
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let word = std::fs::read(&path).map_err(|error| error.to_string())?;
        match engine.with_pdfium(move |pdfium| word_to_pdf(pdfium, &word)) {
            Ok(pdf) => {
                let pages = page_count(&engine, &pdf)?;
                let pictures = document_images(&engine, &pdf, 60, false, &|_, _| true)?;
                let target = Path::new(output);
                std::fs::write(target.join(format!("{name}.pdf")), pdf)
                    .map_err(|error| error.to_string())?;
                std::fs::write(target.join(format!("{name}.png")), &pictures[0])
                    .map_err(|error| error.to_string())?;
                println!("ok    {name}: {pages} pages");
            }
            Err(error) => println!("FAIL  {name}: {error}"),
        }
    }
    Ok(())
}
