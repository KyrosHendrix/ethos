//! Writers for conversions out of PDF: Markdown, plain text and Word.

use std::fmt::Write as _;
use std::io::Write as _;

use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::structure::{Block, Span};

/// One finished output file, named relative to the file the person chose.
pub struct OutputFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

fn escape_markdown(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|'
        ) {
            output.push('\\');
        }
        output.push(character);
    }
    output
}

fn markdown_spans(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|span| {
            let text = escape_markdown(span.text.trim());
            let (open, close) = match (span.bold, span.italic) {
                (true, true) => ("***", "***"),
                (true, false) => ("**", "**"),
                (false, true) => ("*", "*"),
                (false, false) => ("", ""),
            };
            format!("{open}{text}{close}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn plain_spans(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|span| span.text.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Markdown for `pages`, with pictures as separate files in a folder beside it.
pub fn to_markdown(pages: &[Vec<Block>], stem: &str) -> Vec<OutputFile> {
    let folder = format!("{stem} images");
    let mut files = Vec::new();
    let mut output = String::new();
    let mut picture = 0;
    for block in pages.iter().flatten() {
        match block {
            Block::Heading { level, text } => {
                let hashes = "#".repeat(usize::from(*level));
                let _ = writeln!(output, "{hashes} {}\n", escape_markdown(text));
            }
            Block::Paragraph { spans } => {
                let _ = writeln!(output, "{}\n", markdown_spans(spans));
            }
            Block::ListItem { ordered, spans } => {
                let marker = if *ordered { "1." } else { "-" };
                let _ = writeln!(output, "{marker} {}", markdown_spans(spans));
            }
            Block::Table { rows } => {
                for (index, row) in rows.iter().enumerate() {
                    let cells: Vec<_> = row.iter().map(|cell| escape_markdown(cell)).collect();
                    let _ = writeln!(output, "| {} |", cells.join(" | "));
                    if index == 0 {
                        let _ = writeln!(output, "|{}", " --- |".repeat(row.len()));
                    }
                }
                output.push('\n');
            }
            Block::Image { png, .. } => {
                picture += 1;
                let name = format!("{folder}/picture-{picture}.png");
                let link = name.replace(' ', "%20");
                let _ = writeln!(output, "![Picture {picture}]({link})\n");
                files.push(OutputFile {
                    name,
                    bytes: png.clone(),
                });
            }
        }
    }
    files.insert(
        0,
        OutputFile {
            name: format!("{stem}.md"),
            bytes: output.into_bytes(),
        },
    );
    files
}

/// Plain text for `pages`, keeping paragraphs and leaving pictures out.
pub fn to_text(pages: &[Vec<Block>]) -> String {
    let mut output = String::new();
    for block in pages.iter().flatten() {
        match block {
            Block::Heading { text, .. } => {
                let _ = writeln!(output, "{text}\n");
            }
            Block::Paragraph { spans } => {
                let _ = writeln!(output, "{}\n", plain_spans(spans));
            }
            Block::ListItem { ordered, spans } => {
                let marker = if *ordered { "1." } else { "\u{2022}" };
                let _ = writeln!(output, "{marker} {}", plain_spans(spans));
            }
            Block::Table { rows } => {
                for row in rows {
                    let _ = writeln!(output, "{}", row.join("\t"));
                }
                output.push('\n');
            }
            Block::Image { .. } => {}
        }
    }
    output
}

/// Removes forbidden XML 1.0 characters before escaping DOCX text content.
fn escape_xml(text: &str) -> String {
    text.chars()
        .filter(|&c| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn word_runs(spans: &[Span]) -> String {
    let mut output = String::new();
    for (index, span) in spans.iter().enumerate() {
        let mut style = String::new();
        if span.bold {
            style.push_str("<w:b/>");
        }
        if span.italic {
            style.push_str("<w:i/>");
        }
        let separator = if index == 0 { "" } else { " " };
        let text = escape_xml(span.text.trim());
        let _ = write!(
            output,
            r#"<w:r><w:rPr>{style}</w:rPr><w:t xml:space="preserve">{separator}{text}</w:t></w:r>"#
        );
    }
    output
}

fn word_picture(id: usize, width: f32, height: f32) -> String {
    const EMU_PER_POINT: f32 = 12700.0;
    const MAX_WIDTH: f32 = 468.0;
    let scale = (MAX_WIDTH / width).min(1.0);
    let cx = (width * scale * EMU_PER_POINT) as u64;
    let cy = (height * scale * EMU_PER_POINT) as u64;
    format!(
        r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{id}" name="Picture {id}"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{id}" name="Picture {id}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImage{id}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
    )
}

fn word_table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(1);
    let width = 9360 / columns;
    let mut output = String::from(
        r#"<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/><w:tblBorders><w:top w:val="single" w:sz="4" w:color="BFBFBF"/><w:left w:val="single" w:sz="4" w:color="BFBFBF"/><w:bottom w:val="single" w:sz="4" w:color="BFBFBF"/><w:right w:val="single" w:sz="4" w:color="BFBFBF"/><w:insideH w:val="single" w:sz="4" w:color="BFBFBF"/><w:insideV w:val="single" w:sz="4" w:color="BFBFBF"/></w:tblBorders></w:tblPr><w:tblGrid>"#,
    );
    for _ in 0..columns {
        let _ = write!(output, r#"<w:gridCol w:w="{width}"/>"#);
    }
    output.push_str("</w:tblGrid>");
    for row in rows {
        output.push_str("<w:tr>");
        for index in 0..columns {
            let text = escape_xml(row.get(index).map_or("", String::as_str));
            let _ = write!(
                output,
                r#"<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/></w:tcPr><w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p></w:tc>"#
            );
        }
        output.push_str("</w:tr>");
    }
    output.push_str("</w:tbl><w:p/>");
    output
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/></Types>"#;

const PACKAGE_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="264" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="320" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="40"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="100"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="30"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="200" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/></w:rPr></w:style></w:styles>"#;

const NUMBERING: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="&#8226;"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num></w:numbering>"#;

/// An editable Word document for `pages`.
pub fn to_docx(pages: &[Vec<Block>]) -> Result<Vec<u8>, String> {
    let mut body = String::new();
    let mut pictures = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        for block in page {
            match block {
                Block::Heading { level, text } => {
                    let _ = write!(
                        body,
                        r#"<w:p><w:pPr><w:pStyle w:val="Heading{level}"/></w:pPr><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
                        escape_xml(text)
                    );
                }
                Block::Paragraph { spans } => {
                    let _ = write!(body, "<w:p>{}</w:p>", word_runs(spans));
                }
                Block::ListItem { ordered, spans } => {
                    let list = if *ordered { 2 } else { 1 };
                    let _ = write!(
                        body,
                        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="{list}"/></w:numPr></w:pPr>{}</w:p>"#,
                        word_runs(spans)
                    );
                }
                Block::Table { rows } => body.push_str(&word_table(rows)),
                Block::Image { png, width, height } => {
                    pictures.push(png);
                    body.push_str(&word_picture(pictures.len(), *width, *height));
                }
            }
        }
        if index + 1 < pages.len() {
            body.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
        }
    }
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><w:body>{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let mut relationships = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdNumbering" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>"#,
    );
    for index in 1..=pictures.len() {
        let _ = write!(
            relationships,
            r#"<Relationship Id="rIdImage{index}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image{index}.png"/>"#
        );
    }
    relationships.push_str("</Relationships>");

    let failed = |error: zip::result::ZipError| error.to_string();
    let mut archive = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    let mut part = |name: &str, content: &[u8]| -> Result<(), String> {
        archive.start_file(name, options).map_err(failed)?;
        archive
            .write_all(content)
            .map_err(|error| error.to_string())
    };
    part("[Content_Types].xml", CONTENT_TYPES.as_bytes())?;
    part("_rels/.rels", PACKAGE_RELS.as_bytes())?;
    part("word/document.xml", document.as_bytes())?;
    part("word/styles.xml", STYLES.as_bytes())?;
    part("word/numbering.xml", NUMBERING.as_bytes())?;
    part("word/_rels/document.xml.rels", relationships.as_bytes())?;
    for (index, png) in pictures.iter().enumerate() {
        part(&format!("word/media/image{}.png", index + 1), png)?;
    }
    Ok(archive.finish().map_err(failed)?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn span(text: &str, bold: bool) -> Span {
        Span {
            text: text.to_owned(),
            bold,
            italic: false,
        }
    }

    fn sample() -> Vec<Vec<Block>> {
        vec![
            vec![
                Block::Heading {
                    level: 1,
                    text: "Report & plan".into(),
                },
                Block::Paragraph {
                    spans: vec![span("Opening", true), span("words with *stars*.", false)],
                },
                Block::ListItem {
                    ordered: false,
                    spans: vec![span("A point", false)],
                },
                Block::ListItem {
                    ordered: true,
                    spans: vec![span("A step", false)],
                },
            ],
            vec![
                Block::Table {
                    rows: vec![
                        vec!["Name".into(), "Qty".into()],
                        vec!["Tea".into(), "2".into()],
                    ],
                },
                Block::Image {
                    png: vec![1, 2, 3],
                    width: 100.0,
                    height: 50.0,
                },
            ],
        ]
    }

    #[test]
    fn markdown_keeps_structure_and_escapes_syntax() {
        let files = to_markdown(&sample(), "Report");
        let text = String::from_utf8(files[0].bytes.clone()).unwrap();
        assert_eq!(files[0].name, "Report.md");
        assert!(text.contains("# Report & plan\n"), "{text}");
        assert!(
            text.contains(r"**Opening** words with \*stars\*."),
            "{text}"
        );
        assert!(text.contains("- A point\n1. A step\n"), "{text}");
        assert!(
            text.contains("| Name | Qty |\n| --- | --- |\n| Tea | 2 |"),
            "{text}"
        );
        assert!(
            text.contains("![Picture 1](Report%20images/picture-1.png)"),
            "{text}"
        );
        assert_eq!(files[1].name, "Report images/picture-1.png");
    }

    #[test]
    fn plain_text_drops_styling_and_pictures() {
        let text = to_text(&sample());
        assert!(text.contains("Opening words with *stars*."));
        assert!(text.contains("Name\tQty"));
        assert!(!text.contains("**"));
    }

    fn part(docx: &[u8], name: &str) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(docx)).unwrap();
        let mut text = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    #[test]
    fn word_documents_are_well_formed_packages() {
        let docx = to_docx(&sample()).unwrap();
        for name in [
            "[Content_Types].xml",
            "_rels/.rels",
            "word/document.xml",
            "word/styles.xml",
            "word/numbering.xml",
            "word/_rels/document.xml.rels",
        ] {
            roxmltree::Document::parse(&part(&docx, name)).unwrap();
        }
        let document = part(&docx, "word/document.xml");
        assert!(document.contains("Report &amp; plan"));
        assert!(document.contains(r#"<w:pStyle w:val="Heading1"/>"#));
        assert!(document.contains("<w:tbl>"));
        assert!(document.contains(r#"r:embed="rIdImage1""#));
        assert!(part(&docx, "word/_rels/document.xml.rels").contains("media/image1.png"));
    }

    #[test]
    fn word_export_removes_forbidden_xml_characters() {
        let text = "Valid\ttext\n\r & < > \" café 🦀\u{2}\u{c}\u{fffe}\u{ffff}";
        let blocks = vec![vec![Block::Paragraph {
            spans: vec![span(text, false)],
        }]];
        let docx = to_docx(&blocks).unwrap();
        let xml = part(&docx, "word/document.xml");
        roxmltree::Document::parse(&xml).unwrap();
        assert!(xml.contains("café 🦀"));
        assert!(xml.contains("&amp; &lt; &gt; &quot;"));
        assert!(!xml.contains(['\u{2}', '\u{c}', '\u{fffe}', '\u{ffff}']));
    }
}
