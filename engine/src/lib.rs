//! PDF parsing, rendering, search and export. Nothing here depends on the desktop shell.

mod batch;
mod compare;
mod convert;
mod export;
mod forms;
mod media;
mod ocr;
mod pages;
mod reader;
mod sample;
mod structure;
mod word;

pub use batch::{
    CANCELLED, compare_documents, document_blocks, document_images, page_count, searchable_copy,
};
pub use compare::compare_lines;
pub use convert::{OutputFile, to_docx, to_markdown, to_text};
pub use export::{export_pdf, redaction_pages, split_marks};
pub use forms::read_fields;
pub use media::{
    PageFit, RedactionPage, apply_redactions, compress_pdf, encode_image, images_to_pdf,
    verify_redactions,
};
pub use ocr::{OcrPage, OcrWord, add_text_layer, recognize};
pub use pages::{
    every_n_pages, extract_pages, merge_pdfs, parse_ranges, protect_pdf, remove_password,
};
pub use pdfium_render::prelude::{PdfDocument, PdfPageRenderRotation, PdfRenderConfig, Pdfium};
pub use reader::{Engine, Frame, page_view};
pub use sample::create_sample;
pub use structure::{Block, page_blocks};
pub use word::word_to_pdf;
