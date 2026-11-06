//! Recovers document structure from positioned text, for conversions out of PDF.

use std::collections::HashMap;

use pdfium_render::prelude::*;

/// One run of text on a line, with the style it was drawn in.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub text: String,
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph {
        spans: Vec<Span>,
    },
    ListItem {
        ordered: bool,
        spans: Vec<Span>,
    },
    Table {
        rows: Vec<Vec<String>>,
    },
    Image {
        png: Vec<u8>,
        width: f32,
        height: f32,
    },
}

fn style_of(character: &PdfPageTextChar) -> (f32, bool, bool) {
    let name = character.font_name().to_lowercase();
    let bold = character.font_is_bold_reenforced()
        || ["bold", "black", "heavy", "semibold", "demi"]
            .iter()
            .any(|word| name.contains(word));
    let italic = character.font_is_italic() || name.contains("italic") || name.contains("oblique");
    (character.scaled_font_size().value, bold, italic)
}

/// The upright text runs of a page, top to bottom in drawing order.
pub fn page_lines(page: &PdfPage) -> Result<Vec<TextLine>, String> {
    let text = page.text().map_err(|error| error.to_string())?;
    let segments = text.segments();
    let lines = segments
        .iter()
        .filter_map(|segment| {
            let content = segment.text();
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return None;
            }
            let chars = segment.chars().ok()?;
            let (size, bold, italic) = chars.iter().find_map(|character| {
                let printable = character
                    .unicode_char()
                    .is_some_and(|value| !value.is_whitespace());
                printable.then(|| style_of(&character))
            })?;
            let bounds = segment.bounds();
            Some(TextLine {
                text: trimmed.to_owned(),
                left: bounds.left().value,
                right: bounds.right().value,
                top: bounds.top().value,
                bottom: bounds.bottom().value,
                size,
                bold,
                italic,
            })
        })
        .collect();
    Ok(lines)
}

/// The size most text on the page is set in, weighted by length.
fn body_size(lines: &[TextLine]) -> f32 {
    let mut totals: HashMap<i32, usize> = HashMap::new();
    for line in lines {
        *totals.entry((line.size * 2.0).round() as i32).or_default() += line.text.chars().count();
    }
    totals
        .into_iter()
        .max_by_key(|(size, total)| (*total, -*size))
        .map_or(12.0, |(size, _)| size as f32 / 2.0)
}

/// Groups runs sitting on the same baseline, so tables and columns can be told apart.
fn rows_of(lines: &[TextLine]) -> Vec<Vec<&TextLine>> {
    let mut rows: Vec<Vec<&TextLine>> = Vec::new();
    for line in lines {
        let middle = (line.top + line.bottom) / 2.0;
        match rows.last_mut() {
            Some(row) if row[0].bottom <= middle && middle <= row[0].top => row.push(line),
            _ => rows.push(vec![line]),
        }
    }
    rows.iter_mut()
        .for_each(|row| row.sort_by(|a, b| a.left.total_cmp(&b.left)));
    rows
}

fn is_table_row(row: &[&TextLine]) -> bool {
    row.len() >= 2
        && row
            .windows(2)
            .all(|pair| pair[1].left - pair[0].right > pair[0].size * 1.2)
}

fn same_columns(a: &[&TextLine], b: &[&TextLine]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(first, second)| (first.left - second.left).abs() < first.size * 1.5)
}

fn list_marker(text: &str) -> Option<(bool, &str)> {
    for bullet in [
        "\u{2022}",
        "\u{2023}",
        "\u{25e6}",
        "\u{2043}",
        "\u{00b7}",
        "- ",
        "* ",
        "\u{2013} ",
    ] {
        if let Some(rest) = text.strip_prefix(bullet) {
            return Some((false, rest.trim_start()));
        }
    }
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if (1..=3).contains(&digits)
        && let Some(rest) = text[digits..].strip_prefix(['.', ')'])
        && rest.starts_with(' ')
    {
        return Some((true, rest.trim_start()));
    }
    None
}

fn push_text(spans: &mut Vec<Span>, text: &str, bold: bool, italic: bool) {
    match spans.last_mut() {
        Some(last) if last.bold == bold && last.italic == italic => {
            match last.text.strip_suffix('-') {
                // A word broken across lines.
                Some(head) if text.starts_with(char::is_lowercase) => {
                    last.text = format!("{head}{text}");
                }
                _ => {
                    last.text.push(' ');
                    last.text.push_str(text);
                }
            }
        }
        Some(last) => {
            last.text.push(' ');
            spans.push(Span {
                text: text.to_owned(),
                bold,
                italic,
            });
        }
        None => spans.push(Span {
            text: text.to_owned(),
            bold,
            italic,
        }),
    }
}

fn heading_level(line: &TextLine, body: f32) -> Option<u8> {
    let ratio = line.size / body;
    let short = line.text.chars().count() < 90;
    match () {
        () if ratio >= 1.8 => Some(1),
        () if ratio >= 1.35 => Some(2),
        () if ratio >= 1.12 && short => Some(3),
        () if line.bold && short && !line.text.ends_with(['.', ',', ';']) && ratio >= 1.0 => {
            Some(3)
        }
        () => None,
    }
}

/// Turns one page's lines into blocks, each paired with how far down the page it starts.
pub fn structure_lines(lines: &[TextLine]) -> Vec<(f32, Block)> {
    let body = body_size(lines);
    let rows = rows_of(lines);
    let mut blocks: Vec<(f32, Block)> = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        let row = &rows[index];
        let top = row[0].top;
        if is_table_row(row) {
            let mut end = index + 1;
            while end < rows.len() && is_table_row(&rows[end]) && same_columns(row, &rows[end]) {
                end += 1;
            }
            if end - index >= 2 {
                let cells = rows[index..end]
                    .iter()
                    .map(|row| row.iter().map(|cell| cell.text.clone()).collect())
                    .collect();
                blocks.push((top, Block::Table { rows: cells }));
                index = end;
                continue;
            }
        }
        let first = row[0];
        if row.len() == 1
            && let Some(level) = heading_level(first, body)
        {
            let mut text = first.text.clone();
            index += 1;
            // A title often wraps onto a second line at the same size.
            while let Some(next) = rows.get(index).filter(|next| {
                next.len() == 1
                    && (next[0].size - first.size).abs() < 0.5
                    && next[0].bold == first.bold
                    && first.top - next[0].top < first.size * 1.6
            }) {
                text.push(' ');
                text.push_str(&next[0].text);
                index += 1;
            }
            blocks.push((top, Block::Heading { level, text }));
            continue;
        }
        let marker = list_marker(&first.text);
        let mut spans = Vec::new();
        let opening = marker.map_or(first.text.as_str(), |(_, rest)| rest);
        push_text(&mut spans, opening, first.bold, first.italic);
        row[1..]
            .iter()
            .for_each(|cell| push_text(&mut spans, &cell.text, cell.bold, cell.italic));
        let mut previous = first;
        index += 1;
        while let Some(next) = rows.get(index) {
            let line = next[0];
            let gap = previous.top - line.top;
            let continues = (line.size - first.size).abs() < 0.6
                && gap > 0.0
                && gap < first.size * 2.0
                && list_marker(&line.text).is_none()
                && !is_table_row(next)
                && heading_level(line, body).is_none_or(|_| line.size <= body * 1.05 && !line.bold);
            if !continues {
                break;
            }
            next.iter()
                .for_each(|cell| push_text(&mut spans, &cell.text, cell.bold, cell.italic));
            previous = line;
            index += 1;
        }
        let block = match marker {
            Some((ordered, _)) => Block::ListItem { ordered, spans },
            None => Block::Paragraph { spans },
        };
        blocks.push((top, block));
    }
    blocks
}

/// Pictures on a page as PNG crops of the rendered page, skipping full-page scans of text.
pub fn page_images(page: &PdfPage) -> Result<Vec<(f32, Block)>, String> {
    const DPI_SCALE: f32 = 2.0;
    let mut rects = Vec::new();
    for object in page.objects().iter() {
        if object.object_type() == PdfPageObjectType::Image
            && let Ok(bounds) = object.bounds()
        {
            let (width, height) = (bounds.width().value, bounds.height().value);
            if width > 12.0 && height > 12.0 {
                rects.push((bounds.left().value, bounds.top().value, width, height));
            }
        }
    }
    if rects.is_empty() {
        return Ok(Vec::new());
    }
    let view = page
        .boundaries()
        .bounding()
        .map_err(|error| error.to_string())?
        .bounds;
    let (left, top) = (view.left().value, view.top().value);
    let (page_width, page_height) = (view.width().value, view.height().value);
    let (width, height) = (
        (page_width * DPI_SCALE) as u32,
        (page_height * DPI_SCALE) as u32,
    );
    let config = PdfRenderConfig::new().set_target_size(width as Pixels, height as Pixels);
    let bitmap = page
        .render_with_config(&config)
        .map_err(|error| error.to_string())?;
    let (width, height) = (bitmap.width() as u32, bitmap.height() as u32);
    let picture = image::RgbaImage::from_raw(width, height, bitmap.as_rgba_bytes())
        .ok_or("This page could not be converted.")?;
    let mut blocks = Vec::new();
    for (x, y, drawn_width, drawn_height) in rects {
        let crop_x = ((x - left) * DPI_SCALE).max(0.0) as u32;
        let crop_y = ((top - y) * DPI_SCALE).max(0.0) as u32;
        let crop_width = ((drawn_width * DPI_SCALE) as u32).min(width.saturating_sub(crop_x));
        let crop_height = ((drawn_height * DPI_SCALE) as u32).min(height.saturating_sub(crop_y));
        if crop_width == 0 || crop_height == 0 {
            continue;
        }
        let cropped = image::imageops::crop_imm(&picture, crop_x, crop_y, crop_width, crop_height);
        let png = crate::media::encode_image(
            cropped.to_image().into_raw(),
            crop_width,
            crop_height,
            false,
        )?;
        blocks.push((
            y,
            Block::Image {
                png,
                width: drawn_width,
                height: drawn_height,
            },
        ));
    }
    Ok(blocks)
}

/// Every block of a page in reading order.
pub fn page_blocks(page: &PdfPage) -> Result<Vec<Block>, String> {
    let lines = page_lines(page)?;
    let mut text = structure_lines(&lines);
    let images = page_images(page)?;
    // Text keeps its drawing order; pictures slot in by their position on the page.
    for (top, block) in images {
        let at = text
            .iter()
            .position(|(other, _)| *other < top)
            .unwrap_or(text.len());
        text.insert(at, (top, block));
    }
    Ok(text.into_iter().map(|(_, block)| block).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, left: f32, top: f32, size: f32) -> TextLine {
        TextLine {
            text: text.to_owned(),
            left,
            right: left + text.len() as f32 * size * 0.5,
            top,
            bottom: top - size,
            size,
            bold: false,
            italic: false,
        }
    }

    fn blocks(lines: &[TextLine]) -> Vec<Block> {
        structure_lines(lines)
            .into_iter()
            .map(|(_, block)| block)
            .collect()
    }

    fn plain(text: &str) -> Vec<Span> {
        vec![Span {
            text: text.to_owned(),
            bold: false,
            italic: false,
        }]
    }

    const BODY: &str = "A long line of ordinary body text that sets the size";

    #[test]
    fn sizes_become_heading_levels() {
        let found = blocks(&[
            line("Title", 50.0, 700.0, 24.0),
            line("Section", 50.0, 660.0, 17.0),
            line(BODY, 50.0, 620.0, 12.0),
        ]);
        assert_eq!(
            found[0],
            Block::Heading {
                level: 1,
                text: "Title".into()
            }
        );
        assert_eq!(
            found[1],
            Block::Heading {
                level: 2,
                text: "Section".into()
            }
        );
        assert_eq!(found[2], Block::Paragraph { spans: plain(BODY) });
    }

    #[test]
    fn wrapped_lines_join_and_blank_lines_separate() {
        let found = blocks(&[
            line("First line of a para-", 50.0, 700.0, 12.0),
            line("graph that continues.", 50.0, 685.0, 12.0),
            line("A new paragraph after a gap.", 50.0, 640.0, 12.0),
        ]);
        assert_eq!(
            found,
            [
                Block::Paragraph {
                    spans: plain("First line of a paragraph that continues.")
                },
                Block::Paragraph {
                    spans: plain("A new paragraph after a gap.")
                },
            ]
        );
    }

    #[test]
    fn list_markers_are_recognized_and_removed() {
        let found = blocks(&[
            line("\u{2022} One point", 50.0, 700.0, 12.0),
            line("\u{2022} Another point", 50.0, 685.0, 12.0),
            line("1. A numbered step", 50.0, 670.0, 12.0),
        ]);
        assert_eq!(
            found,
            [
                Block::ListItem {
                    ordered: false,
                    spans: plain("One point")
                },
                Block::ListItem {
                    ordered: false,
                    spans: plain("Another point")
                },
                Block::ListItem {
                    ordered: true,
                    spans: plain("A numbered step")
                },
            ]
        );
    }

    #[test]
    fn aligned_columns_become_a_table() {
        let found = blocks(&[
            line("Name", 50.0, 700.0, 12.0),
            line("Amount", 300.0, 700.0, 12.0),
            line("Coffee", 50.0, 685.0, 12.0),
            line("4.50", 300.0, 685.0, 12.0),
        ]);
        assert_eq!(
            found,
            [Block::Table {
                rows: vec![
                    vec!["Name".to_owned(), "Amount".to_owned()],
                    vec!["Coffee".to_owned(), "4.50".to_owned()],
                ]
            }]
        );
    }

    #[test]
    fn a_single_aligned_row_stays_a_paragraph() {
        let found = blocks(&[
            line("Left", 50.0, 700.0, 12.0),
            line("Right", 400.0, 700.0, 12.0),
        ]);
        assert!(matches!(found[..], [Block::Paragraph { .. }]));
    }

    #[test]
    fn style_changes_split_spans() {
        let mut bold = line("Bold start of the line", 50.0, 700.0, 12.0);
        bold.bold = true;
        bold.text
            .push_str(" that is long enough to be body text, really.");
        let found = blocks(&[
            bold,
            line("then plain text follows here", 50.0, 685.0, 12.0),
        ]);
        let Block::Paragraph { spans } = &found[0] else {
            panic!("{found:?}");
        };
        assert_eq!(spans.len(), 2);
        assert!(spans[0].bold && !spans[1].bold);
    }
}
