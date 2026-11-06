//! Word to PDF: reads a `.docx`, lays it out, and draws it with embedded system fonts.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::PathBuf;

use pdfium_render::prelude::*;
use roxmltree::{Document as Xml, Node};
use ttf_parser::Face;
use zip::ZipArchive;

const WORD_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const RELATIONSHIP_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const DRAWING_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const WP_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const TWIPS_PER_POINT: f32 = 20.0;
const EMU_PER_POINT: f32 = 12700.0;
const LINE_SPACING: f32 = 1.22;

#[derive(Clone, Debug, PartialEq)]
struct Style {
    font: String,
    size: f32,
    bold: bool,
    italic: bool,
    color: [u8; 3],
}

impl Default for Style {
    fn default() -> Self {
        Self {
            font: "Calibri".into(),
            size: 11.0,
            bold: false,
            italic: false,
            color: [0, 0, 0],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Inline {
    Text {
        text: String,
        style: Style,
    },
    Picture {
        media: String,
        width: f32,
        height: f32,
    },
    LineBreak,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
struct Paragraph {
    inlines: Vec<Inline>,
    align: Align,
    before: f32,
    after: f32,
    indent: f32,
    hanging: Option<String>,
    page_break: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Item {
    Paragraph(Paragraph),
    Table {
        widths: Vec<f32>,
        rows: Vec<Vec<Vec<Paragraph>>>,
    },
}

#[derive(Clone, Debug, PartialEq)]
struct Sheet {
    width: f32,
    height: f32,
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
}

struct Parsed {
    items: Vec<Item>,
    sheet: Sheet,
    media: HashMap<String, Vec<u8>>,
}

#[derive(Clone, Default)]
struct StyleDefinition {
    based_on: Option<String>,
    font: Option<String>,
    size: Option<f32>,
    bold: Option<bool>,
    italic: Option<bool>,
    color: Option<[u8; 3]>,
    before: Option<f32>,
    after: Option<f32>,
}

fn word<'a>(node: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    node.children()
        .find(|child| child.has_tag_name((WORD_NS, name)))
}

fn value<'a>(node: Node<'a, 'a>) -> Option<&'a str> {
    node.attribute((WORD_NS, "val"))
}

fn flag(node: Node, name: &str) -> Option<bool> {
    word(node, name).map(|node| !matches!(value(node), Some("0" | "false")))
}

fn parse_color(text: &str) -> Option<[u8; 3]> {
    if text.len() != 6 {
        return None;
    }
    let part = |start: usize| u8::from_str_radix(text.get(start..start + 2)?, 16).ok();
    Some([part(0)?, part(2)?, part(4)?])
}

fn run_definition(properties: Node, definition: &mut StyleDefinition) {
    if let Some(fonts) = word(properties, "rFonts")
        && let Some(font) = fonts.attribute((WORD_NS, "ascii"))
    {
        definition.font = Some(font.to_owned());
    }
    if let Some(size) = word(properties, "sz")
        .and_then(value)
        .and_then(|size| size.parse::<f32>().ok())
    {
        definition.size = Some(size / 2.0);
    }
    if let Some(bold) = flag(properties, "b") {
        definition.bold = Some(bold);
    }
    if let Some(italic) = flag(properties, "i") {
        definition.italic = Some(italic);
    }
    if let Some(color) = word(properties, "color")
        .and_then(value)
        .and_then(parse_color)
    {
        definition.color = Some(color);
    }
}

fn spacing(paragraph: Node, definition: &mut StyleDefinition) {
    if let Some(spacing) = word(paragraph, "spacing") {
        let read = |name: &str| {
            spacing
                .attribute((WORD_NS, name))
                .and_then(|value| value.parse::<f32>().ok())
                .map(|value| value / TWIPS_PER_POINT)
        };
        definition.before = read("before").or(definition.before);
        definition.after = read("after").or(definition.after);
    }
}

fn read_styles(xml: Option<&str>) -> (StyleDefinition, HashMap<String, StyleDefinition>) {
    let mut defaults = StyleDefinition::default();
    let mut styles = HashMap::new();
    let Some(document) = xml.and_then(|xml| Xml::parse(xml).ok()) else {
        return (defaults, styles);
    };
    for node in document.descendants() {
        if node.has_tag_name((WORD_NS, "rPrDefault"))
            && let Some(properties) = word(node, "rPr")
        {
            run_definition(properties, &mut defaults);
        }
        if node.has_tag_name((WORD_NS, "pPrDefault"))
            && let Some(properties) = word(node, "pPr")
        {
            spacing(properties, &mut defaults);
        }
        if node.has_tag_name((WORD_NS, "style"))
            && let Some(id) = node.attribute((WORD_NS, "styleId"))
        {
            let mut definition = StyleDefinition {
                based_on: word(node, "basedOn").and_then(value).map(str::to_owned),
                ..StyleDefinition::default()
            };
            if let Some(properties) = word(node, "rPr") {
                run_definition(properties, &mut definition);
            }
            if let Some(properties) = word(node, "pPr") {
                spacing(properties, &mut definition);
            }
            styles.insert(id.to_owned(), definition);
        }
    }
    (defaults, styles)
}

/// Applies a style chain from the root down, so nearer styles win.
fn resolve(
    id: Option<&str>,
    defaults: &StyleDefinition,
    styles: &HashMap<String, StyleDefinition>,
) -> StyleDefinition {
    let mut chain = vec![defaults.clone()];
    let mut next = id.map(str::to_owned).or_else(|| Some("Normal".to_owned()));
    let mut guard = 0;
    let mut own = Vec::new();
    while let Some(current) = next.take().filter(|_| guard < 16) {
        guard += 1;
        if let Some(definition) = styles.get(&current) {
            next = definition.based_on.clone();
            own.push(definition.clone());
        }
    }
    chain.extend(own.into_iter().rev());
    let mut merged = StyleDefinition::default();
    for definition in chain {
        merged.font = definition.font.or(merged.font);
        merged.size = definition.size.or(merged.size);
        merged.bold = definition.bold.or(merged.bold);
        merged.italic = definition.italic.or(merged.italic);
        merged.color = definition.color.or(merged.color);
        merged.before = definition.before.or(merged.before);
        merged.after = definition.after.or(merged.after);
    }
    merged
}

fn apply(style: &mut Style, definition: &StyleDefinition) {
    if let Some(font) = &definition.font {
        style.font.clone_from(font);
    }
    style.size = definition.size.unwrap_or(style.size);
    style.bold = definition.bold.unwrap_or(style.bold);
    style.italic = definition.italic.unwrap_or(style.italic);
    style.color = definition.color.unwrap_or(style.color);
}

/// `true` for numbered list ids, `false` for bullets, read from `numbering.xml`.
fn read_numbering(xml: Option<&str>) -> HashMap<String, bool> {
    let mut abstracts = HashMap::new();
    let mut lists = HashMap::new();
    let Some(document) = xml.and_then(|xml| Xml::parse(xml).ok()) else {
        return lists;
    };
    for node in document.descendants() {
        if node.has_tag_name((WORD_NS, "abstractNum"))
            && let Some(id) = node.attribute((WORD_NS, "abstractNumId"))
        {
            let numbered = word(node, "lvl")
                .and_then(|level| word(level, "numFmt"))
                .and_then(value)
                .is_some_and(|format| format != "bullet");
            abstracts.insert(id.to_owned(), numbered);
        }
    }
    for node in document.descendants() {
        if node.has_tag_name((WORD_NS, "num"))
            && let Some(id) = node.attribute((WORD_NS, "numId"))
            && let Some(numbered) = word(node, "abstractNumId")
                .and_then(value)
                .and_then(|abstract_id| abstracts.get(abstract_id))
        {
            lists.insert(id.to_owned(), *numbered);
        }
    }
    lists
}

/// Runs that belong to this paragraph. A paragraph's tree also holds things that are not its
/// own text: text boxes have their own paragraphs, deleted revisions are gone, and
/// `mc:Fallback` repeats `mc:Choice` for older readers.
fn collect_runs<'a>(node: Node<'a, 'a>, runs: &mut Vec<Node<'a, 'a>>) {
    for child in node.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "r" => {
                runs.push(child);
            }
            "del" | "Fallback" | "txbxContent" | "pPr" => {}
            _ => collect_runs(child, runs),
        }
    }
}

/// Text box contents anchored in this paragraph, each once.
fn collect_text_boxes<'a>(node: Node<'a, 'a>, boxes: &mut Vec<Node<'a, 'a>>) {
    for child in node.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "txbxContent" => boxes.push(child),
            "Fallback" | "del" => {}
            _ => collect_text_boxes(child, boxes),
        }
    }
}

/// The paragraphs and tables under `parent`, looking through content controls.
fn block_nodes<'a>(parent: Node<'a, 'a>) -> Vec<Node<'a, 'a>> {
    let mut blocks = Vec::new();
    for child in parent.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "p" | "tbl" => blocks.push(child),
            "sdt" => {
                if let Some(content) = word(child, "sdtContent") {
                    blocks.extend(block_nodes(content));
                }
            }
            _ => {}
        }
    }
    blocks
}

struct Reader<'a> {
    defaults: &'a StyleDefinition,
    styles: &'a HashMap<String, StyleDefinition>,
    numbering: &'a HashMap<String, bool>,
    relationships: &'a HashMap<String, String>,
    counters: HashMap<String, u32>,
    /// Paragraphs from text boxes, waiting to follow the paragraph that anchors them.
    extras: Vec<Paragraph>,
}

impl Reader<'_> {
    fn paragraph(&mut self, node: Node) -> Paragraph {
        let properties = word(node, "pPr");
        let style_id = properties
            .and_then(|node| word(node, "pStyle"))
            .and_then(value);
        let mut definition = resolve(style_id, self.defaults, self.styles);
        if let Some(properties) = properties {
            spacing(properties, &mut definition);
        }
        let mut base = Style::default();
        apply(&mut base, &definition);
        let align = match properties.and_then(|node| word(node, "jc")).and_then(value) {
            Some("center") => Align::Center,
            Some("right" | "end") => Align::Right,
            _ => Align::Left,
        };
        let mut indent = properties
            .and_then(|node| word(node, "ind"))
            .and_then(|ind| ind.attribute((WORD_NS, "left")))
            .and_then(|left| left.parse::<f32>().ok())
            .map_or(0.0, |left| left / TWIPS_PER_POINT);
        let list = properties
            .and_then(|node| word(node, "numPr"))
            .and_then(|numbering| word(numbering, "numId"))
            .and_then(value);
        let hanging = list.filter(|id| *id != "0").map(|id| {
            if indent == 0.0 {
                indent = 18.0;
            }
            if self.numbering.get(id).copied().unwrap_or(false) {
                let counter = self.counters.entry(id.to_owned()).or_default();
                *counter += 1;
                format!("{counter}.")
            } else {
                "\u{2022}".to_owned()
            }
        });
        let mut paragraph = Paragraph {
            inlines: Vec::new(),
            align,
            before: definition.before.unwrap_or(0.0),
            after: definition.after.unwrap_or(0.0),
            indent,
            hanging,
            page_break: properties.is_some_and(|node| word(node, "pageBreakBefore").is_some()),
        };
        let mut runs = Vec::new();
        collect_runs(node, &mut runs);
        for run in runs {
            let mut style = base.clone();
            if let Some(properties) = word(run, "rPr") {
                let mut own = StyleDefinition::default();
                run_definition(properties, &mut own);
                if let Some(id) = word(properties, "rStyle").and_then(value)
                    && let Some(character) = self.styles.get(id)
                {
                    apply(&mut style, character);
                }
                apply(&mut style, &own);
            }
            for child in run.children().filter(Node::is_element) {
                match child.tag_name().name() {
                    "t" => paragraph.inlines.push(Inline::Text {
                        text: child.text().unwrap_or_default().to_owned(),
                        style: style.clone(),
                    }),
                    "tab" => paragraph.inlines.push(Inline::Text {
                        text: "    ".to_owned(),
                        style: style.clone(),
                    }),
                    "br" if child.attribute((WORD_NS, "type")) == Some("page") => {
                        paragraph.page_break = true;
                    }
                    "br" => paragraph.inlines.push(Inline::LineBreak),
                    "drawing" => paragraph.inlines.extend(self.picture(child)),
                    _ => {}
                }
            }
        }
        let mut boxes = Vec::new();
        collect_text_boxes(node, &mut boxes);
        for inner in boxes {
            let paragraphs = block_nodes(inner)
                .into_iter()
                .filter(|child| child.has_tag_name((WORD_NS, "p")))
                .collect::<Vec<_>>();
            for child in paragraphs {
                let extra = self.paragraph(child);
                self.extras.push(extra);
            }
        }
        paragraph
    }

    fn picture(&self, drawing: Node) -> Option<Inline> {
        let extent = drawing
            .descendants()
            .find(|node| node.has_tag_name((WP_NS, "extent")))?;
        let size = |name: &str| extent.attribute(name)?.parse::<f32>().ok();
        let blip = drawing
            .descendants()
            .find(|node| node.has_tag_name((DRAWING_NS, "blip")))?;
        let target = self
            .relationships
            .get(blip.attribute((RELATIONSHIP_NS, "embed"))?)?;
        Some(Inline::Picture {
            media: format!("word/{target}"),
            width: size("cx")? / EMU_PER_POINT,
            height: size("cy")? / EMU_PER_POINT,
        })
    }

    fn table(&mut self, node: Node) -> Item {
        let widths: Vec<f32> = word(node, "tblGrid")
            .map(|grid| {
                grid.children()
                    .filter(|child| child.has_tag_name((WORD_NS, "gridCol")))
                    .filter_map(|column| column.attribute((WORD_NS, "w"))?.parse::<f32>().ok())
                    .map(|width| width / TWIPS_PER_POINT)
                    .collect()
            })
            .unwrap_or_default();
        let rows = node
            .children()
            .filter(|child| child.has_tag_name((WORD_NS, "tr")))
            .map(|row| {
                row.children()
                    .filter(|child| child.has_tag_name((WORD_NS, "tc")))
                    .map(|cell| {
                        block_nodes(cell)
                            .into_iter()
                            .filter(|child| child.has_tag_name((WORD_NS, "p")))
                            .map(|paragraph| self.paragraph(paragraph))
                            .collect()
                    })
                    .collect()
            })
            .collect();
        Item::Table { widths, rows }
    }
}

fn read_sheet(body: Node) -> Sheet {
    let mut sheet = Sheet {
        width: 612.0,
        height: 792.0,
        top: 72.0,
        right: 72.0,
        bottom: 72.0,
        left: 72.0,
    };
    let Some(section) = word(body, "sectPr") else {
        return sheet;
    };
    let read = |node: Node, name: &str| {
        node.attribute((WORD_NS, name))
            .and_then(|value| value.parse::<f32>().ok())
            .map(|value| value / TWIPS_PER_POINT)
    };
    if let Some(size) = word(section, "pgSz") {
        sheet.width = read(size, "w").unwrap_or(sheet.width);
        sheet.height = read(size, "h").unwrap_or(sheet.height);
    }
    if let Some(margin) = word(section, "pgMar") {
        sheet.top = read(margin, "top").unwrap_or(sheet.top);
        sheet.right = read(margin, "right").unwrap_or(sheet.right);
        sheet.bottom = read(margin, "bottom").unwrap_or(sheet.bottom);
        sheet.left = read(margin, "left").unwrap_or(sheet.left);
    }
    sheet
}

fn parse(bytes: &[u8]) -> Result<Parsed, String> {
    let unreadable = "This Word file could not be read. Only .docx files are supported.";
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| unreadable)?;
    let mut part = |name: &str| -> Option<String> {
        let mut text = String::new();
        archive.by_name(name).ok()?.read_to_string(&mut text).ok()?;
        Some(text)
    };
    let document = part("word/document.xml").ok_or(unreadable)?;
    let styles = part("word/styles.xml");
    let numbering = part("word/numbering.xml");
    let relationships = part("word/_rels/document.xml.rels");
    let (defaults, styles) = read_styles(styles.as_deref());
    let numbering = read_numbering(numbering.as_deref());
    let relationships: HashMap<String, String> = relationships
        .as_deref()
        .and_then(|xml| Xml::parse(xml).ok())
        .map(|xml| {
            xml.descendants()
                .filter(|node| {
                    node.has_tag_name("Relationship") || node.tag_name().name() == "Relationship"
                })
                .filter_map(|node| {
                    Some((
                        node.attribute("Id")?.to_owned(),
                        node.attribute("Target")?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let xml = Xml::parse(&document).map_err(|_| unreadable)?;
    let body = xml
        .descendants()
        .find(|node| node.has_tag_name((WORD_NS, "body")))
        .ok_or(unreadable)?;
    let mut reader = Reader {
        defaults: &defaults,
        styles: &styles,
        numbering: &numbering,
        relationships: &relationships,
        counters: HashMap::new(),
        extras: Vec::new(),
    };
    let mut items = Vec::new();
    for child in block_nodes(body) {
        if child.tag_name().name() == "tbl" {
            items.push(reader.table(child));
        } else {
            items.push(Item::Paragraph(reader.paragraph(child)));
        }
        items.extend(reader.extras.drain(..).map(Item::Paragraph));
    }
    let sheet = read_sheet(body);
    let mut media = HashMap::new();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| unreadable)?;
    for index in 0..archive.len() {
        let Ok(mut file) = archive.by_index(index) else {
            continue;
        };
        if file.name().starts_with("word/media/") {
            let name = file.name().to_owned();
            let mut content = Vec::new();
            if file.read_to_end(&mut content).is_ok() {
                media.insert(name, content);
            }
        }
    }
    Ok(Parsed {
        items,
        sheet,
        media,
    })
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct FontKey {
    file: &'static str,
}

/// Picks an installed font with the same character as the requested one.
fn font_file(family: &str, bold: bool, italic: bool) -> FontKey {
    let family = family.to_lowercase();
    let pick = |regular, b, i, bi| match (bold, italic) {
        (true, true) => bi,
        (true, false) => b,
        (false, true) => i,
        (false, false) => regular,
    };
    let file = if family.contains("times")
        || family.contains("cambria")
        || family.contains("georgia")
    {
        pick("times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf")
    } else if family.contains("courier") || family.contains("consolas") || family.contains("mono") {
        pick("cour.ttf", "courbd.ttf", "couri.ttf", "courbi.ttf")
    } else if family.contains("calibri") {
        pick(
            "calibri.ttf",
            "calibrib.ttf",
            "calibrii.ttf",
            "calibriz.ttf",
        )
    } else {
        pick("arial.ttf", "arialbd.ttf", "ariali.ttf", "arialbi.ttf")
    };
    FontKey { file }
}

fn font_folder() -> PathBuf {
    std::env::var_os("WINDIR")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("Fonts")
}

/// Loaded font files, shared by measuring and drawing.
#[derive(Default)]
struct Fonts {
    files: HashMap<FontKey, Vec<u8>>,
}

impl Fonts {
    fn load(&mut self, key: &FontKey) -> Result<&[u8], String> {
        if !self.files.contains_key(key) {
            let folder = font_folder();
            let bytes = std::fs::read(folder.join(key.file))
                .or_else(|_| std::fs::read(folder.join("arial.ttf")))
                .map_err(|_| {
                    "ethos could not find a font to draw this document with.".to_owned()
                })?;
            self.files.insert(key.clone(), bytes);
        }
        Ok(&self.files[key])
    }

    fn width(&mut self, key: &FontKey, text: &str, size: f32) -> Result<f32, String> {
        let bytes = self.load(key)?;
        let face = Face::parse(bytes, 0).map_err(|error| error.to_string())?;
        let units: u32 = text
            .chars()
            .map(|character| {
                face.glyph_index(character)
                    .and_then(|glyph| face.glyph_hor_advance(glyph))
                    .map_or(500, u32::from)
            })
            .sum();
        Ok(units as f32 * size / f32::from(face.units_per_em()))
    }
}

/// One thing to draw, in points from the page's top-left corner.
#[derive(Clone, Debug, PartialEq)]
enum Op {
    Text {
        x: f32,
        baseline: f32,
        text: String,
        key: FontKey,
        size: f32,
        color: [u8; 3],
    },
    Picture {
        x: f32,
        top: f32,
        width: f32,
        height: f32,
        media: String,
    },
    Rule {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
}

struct Layout<'a> {
    fonts: &'a mut Fonts,
    sheet: Sheet,
    pages: Vec<Vec<Op>>,
    y: f32,
}

#[derive(Clone)]
struct Fragment {
    op: Op,
    width: f32,
}

impl Layout<'_> {
    fn bottom_limit(&self) -> f32 {
        self.sheet.height - self.sheet.bottom
    }

    fn new_page(&mut self) {
        self.pages.push(Vec::new());
        self.y = self.sheet.top;
    }

    /// Breaks a paragraph into positioned lines inside `width`, starting at `left`.
    fn lines(
        &mut self,
        paragraph: &Paragraph,
        left: f32,
        width: f32,
    ) -> Result<Vec<(Vec<Fragment>, f32)>, String> {
        let available = (width - paragraph.indent).max(40.0);
        let mut lines: Vec<(Vec<Fragment>, f32)> = Vec::new();
        let mut line: Vec<Fragment> = Vec::new();
        let mut used = 0.0_f32;
        let mut height = 0.0_f32;
        let finish = |line: &mut Vec<Fragment>,
                      used: &mut f32,
                      height: &mut f32,
                      lines: &mut Vec<(Vec<Fragment>, f32)>,
                      fallback: f32| {
            let start = match paragraph.align {
                Align::Left => 0.0,
                Align::Center => (available - *used) / 2.0,
                Align::Right => available - *used,
            };
            let mut x = left + paragraph.indent + start;
            let placed: Vec<Fragment> = line
                .drain(..)
                .map(|mut fragment| {
                    match &mut fragment.op {
                        Op::Text { x: at, .. } | Op::Picture { x: at, .. } => *at = x,
                        Op::Rule { .. } => {}
                    }
                    x += fragment.width;
                    fragment
                })
                .collect();
            lines.push((placed, height.max(fallback)));
            *used = 0.0;
            *height = 0.0;
        };
        let mut last_size = 11.0;
        for inline in &paragraph.inlines {
            match inline {
                Inline::LineBreak => finish(
                    &mut line,
                    &mut used,
                    &mut height,
                    &mut lines,
                    last_size * LINE_SPACING,
                ),
                Inline::Picture {
                    media,
                    width: drawn,
                    height: tall,
                } => {
                    let scale = (available / drawn).min(1.0);
                    let (drawn, tall) = (drawn * scale, tall * scale);
                    if used + drawn > available && !line.is_empty() {
                        finish(
                            &mut line,
                            &mut used,
                            &mut height,
                            &mut lines,
                            last_size * LINE_SPACING,
                        );
                    }
                    line.push(Fragment {
                        op: Op::Picture {
                            x: 0.0,
                            top: 0.0,
                            width: drawn,
                            height: tall,
                            media: media.clone(),
                        },
                        width: drawn,
                    });
                    used += drawn;
                    height = height.max(tall);
                }
                Inline::Text { text, style } => {
                    last_size = style.size;
                    let key = font_file(&style.font, style.bold, style.italic);
                    let line_height = style.size * LINE_SPACING;
                    // Splitting after spaces keeps them with the word they follow.
                    for word in text.split_inclusive(' ') {
                        let measured = self.fonts.width(&key, word.trim_end(), style.size)?;
                        if used + measured > available && !line.is_empty() {
                            finish(&mut line, &mut used, &mut height, &mut lines, line_height);
                        }
                        let advance = self.fonts.width(&key, word, style.size)?;
                        line.push(Fragment {
                            op: Op::Text {
                                x: 0.0,
                                baseline: 0.0,
                                text: word.to_owned(),
                                key: key.clone(),
                                size: style.size,
                                color: style.color,
                            },
                            width: advance,
                        });
                        used += advance;
                        height = height.max(line_height);
                    }
                }
            }
        }
        if !line.is_empty() || lines.is_empty() {
            finish(
                &mut line,
                &mut used,
                &mut height,
                &mut lines,
                last_size * LINE_SPACING,
            );
        }
        Ok(lines)
    }

    fn place(&mut self, fragments: Vec<Fragment>, top: f32, height: f32) {
        let page = self.pages.last_mut().expect("layout starts with a page");
        for fragment in fragments {
            let mut op = fragment.op;
            match &mut op {
                Op::Text { baseline, size, .. } => *baseline = top + height - *size * 0.26,
                Op::Picture {
                    top: at,
                    height: tall,
                    ..
                } => *at = top + height - *tall,
                Op::Rule { .. } => {}
            }
            page.push(op);
        }
    }

    fn paragraph(&mut self, paragraph: &Paragraph) -> Result<(), String> {
        if paragraph.page_break && self.pages.last().is_some_and(|page| !page.is_empty()) {
            self.new_page();
        }
        let left = self.sheet.left;
        let width = self.sheet.width - self.sheet.left - self.sheet.right;
        let lines = self.lines(paragraph, left, width)?;
        self.y += paragraph.before;
        for (index, (fragments, height)) in lines.into_iter().enumerate() {
            if self.y + height > self.bottom_limit() {
                self.new_page();
            }
            if index == 0
                && let Some(marker) = &paragraph.hanging
            {
                let key = font_file("Arial", false, false);
                let size = 11.0;
                let baseline = self.y + height - size * 0.26;
                let page = self.pages.last_mut().expect("layout starts with a page");
                page.push(Op::Text {
                    x: left + paragraph.indent - 16.0,
                    baseline,
                    text: marker.clone(),
                    key,
                    size,
                    color: [0, 0, 0],
                });
            }
            self.place(fragments, self.y, height);
            self.y += height;
        }
        self.y += paragraph.after;
        Ok(())
    }

    fn table(&mut self, widths: &[f32], rows: &[Vec<Vec<Paragraph>>]) -> Result<(), String> {
        const PADDING: f32 = 5.0;
        let total = self.sheet.width - self.sheet.left - self.sheet.right;
        let columns = rows
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(1)
            .max(widths.len())
            .max(1);
        let sum: f32 = widths.iter().sum();
        let widths: Vec<f32> = if sum > 0.0 && widths.len() == columns {
            widths
                .iter()
                .map(|width| width * (total / sum).min(1.0))
                .collect()
        } else {
            vec![total / columns as f32; columns]
        };
        for row in rows {
            let mut cells = Vec::new();
            let mut row_height = 0.0_f32;
            let mut x = self.sheet.left;
            for (index, cell) in row.iter().enumerate() {
                let width = widths.get(index).copied().unwrap_or(0.0);
                let mut lines = Vec::new();
                let mut height = PADDING * 2.0;
                for paragraph in cell {
                    let laid = self.lines(paragraph, x + PADDING, width - 2.0 * PADDING)?;
                    height += laid.iter().map(|(_, height)| height).sum::<f32>();
                    lines.extend(laid);
                }
                row_height = row_height.max(height);
                cells.push((x, width, lines));
                x += width;
            }
            if self.y + row_height > self.bottom_limit() {
                self.new_page();
            }
            let top = self.y;
            let right = self.sheet.left + widths.iter().sum::<f32>();
            let rule = |x1, y1, x2, y2| Op::Rule { x1, y1, x2, y2 };
            let page = self.pages.last_mut().expect("layout starts with a page");
            page.push(rule(self.sheet.left, top, right, top));
            page.push(rule(
                self.sheet.left,
                top + row_height,
                right,
                top + row_height,
            ));
            page.push(rule(
                self.sheet.left,
                top,
                self.sheet.left,
                top + row_height,
            ));
            for (x, width, _) in &cells {
                page.push(rule(x + width, top, x + width, top + row_height));
            }
            for (_, _, lines) in cells {
                let mut y = top + PADDING;
                for (fragments, height) in lines {
                    self.place(fragments, y, height);
                    y += height;
                }
            }
            self.y = top + row_height;
        }
        self.y += 8.0;
        Ok(())
    }
}

fn lay_out(parsed: &Parsed, fonts: &mut Fonts) -> Result<Vec<Vec<Op>>, String> {
    let mut layout = Layout {
        fonts,
        sheet: parsed.sheet.clone(),
        pages: vec![Vec::new()],
        y: parsed.sheet.top,
    };
    for item in &parsed.items {
        match item {
            Item::Paragraph(paragraph) => layout.paragraph(paragraph)?,
            Item::Table { widths, rows } => layout.table(widths, rows)?,
        }
    }
    Ok(layout.pages)
}

/// Converts `.docx` bytes to a PDF.
///
/// shortcut: headers, footers, floating shapes and fonts without a matching system file are
/// not reproduced, and characters a font lacks draw as blanks. Add a font fallback chain
/// and section parsing if documents need them.
pub fn word_to_pdf(pdfium: &Pdfium, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let failed = |error: PdfiumError| error.to_string();
    let parsed = parse(bytes)?;
    let mut fonts = Fonts::default();
    let pages = lay_out(&parsed, &mut fonts)?;
    let mut document = pdfium.create_new_pdf().map_err(failed)?;
    let mut tokens: HashMap<FontKey, PdfFontToken> = HashMap::new();
    for key in pages.iter().flatten().filter_map(|op| match op {
        Op::Text { key, .. } => Some(key.clone()),
        _ => None,
    }) {
        if let std::collections::hash_map::Entry::Vacant(slot) = tokens.entry(key.clone()) {
            let data = fonts.load(&key)?.to_vec();
            let token = document
                .fonts_mut()
                .load_true_type_from_bytes(&data, true)
                .map_err(failed)?;
            slot.insert(token);
        }
    }
    let size = PdfPagePaperSize::Custom(
        PdfPoints::new(parsed.sheet.width),
        PdfPoints::new(parsed.sheet.height),
    );
    let height = parsed.sheet.height;
    for ops in pages {
        let mut page = document
            .pages_mut()
            .create_page_at_end(size)
            .map_err(failed)?;
        for op in ops {
            match op {
                Op::Text {
                    x,
                    baseline,
                    text,
                    key,
                    size,
                    color,
                } => {
                    let text = text.trim_end();
                    if text.is_empty() {
                        continue;
                    }
                    let mut object =
                        PdfPageTextObject::new(&document, text, tokens[&key], PdfPoints::new(size))
                            .map_err(failed)?;
                    object
                        .set_fill_color(PdfColor::new(color[0], color[1], color[2], 255))
                        .map_err(failed)?;
                    object
                        .translate(PdfPoints::new(x), PdfPoints::new(height - baseline))
                        .map_err(failed)?;
                    page.objects_mut().add_text_object(object).map_err(failed)?;
                }
                Op::Rule { x1, y1, x2, y2 } => {
                    let mut path = PdfPagePathObject::new_line(
                        &document,
                        PdfPoints::new(x1),
                        PdfPoints::new(height - y1),
                        PdfPoints::new(x2),
                        PdfPoints::new(height - y2),
                        PdfColor::new(191, 191, 191, 255),
                        PdfPoints::new(0.5),
                    )
                    .map_err(failed)?;
                    path.set_stroke_color(PdfColor::new(191, 191, 191, 255))
                        .map_err(failed)?;
                    page.objects_mut().add_path_object(path).map_err(failed)?;
                }
                Op::Picture {
                    x,
                    top,
                    width,
                    height: tall,
                    media,
                } => {
                    let Some(data) = parsed.media.get(&media) else {
                        continue;
                    };
                    let Ok(picture) = image::load_from_memory(data) else {
                        continue;
                    };
                    let rgba = picture.to_rgba8();
                    let (pixels_wide, pixels_tall) = rgba.dimensions();
                    let mut bgra: Vec<u8> = rgba
                        .chunks(4)
                        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
                        .collect();
                    let bitmap = PdfBitmap::from_bytes(
                        pixels_wide as Pixels,
                        pixels_tall as Pixels,
                        PdfBitmapFormat::BGRA,
                        &mut bgra,
                    )
                    .map_err(failed)?;
                    let mut object = PdfPageImageObject::new(&document).map_err(failed)?;
                    object.set_bitmap(&bitmap).map_err(failed)?;
                    object.scale(width, tall).map_err(failed)?;
                    object
                        .translate(PdfPoints::new(x), PdfPoints::new(height - top - tall))
                        .map_err(failed)?;
                    page.objects_mut()
                        .add_image_object(object)
                        .map_err(failed)?;
                }
            }
        }
    }
    document.save_to_bytes().map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::to_docx;
    use crate::reader::tests::engine;
    use crate::structure::{Block, Span};
    use ethos_core::LoadOutcome;

    fn span(text: &str) -> Span {
        Span {
            text: text.to_owned(),
            bold: false,
            italic: false,
        }
    }

    fn sample(paragraphs: usize) -> Vec<u8> {
        let mut page = vec![
            Block::Heading {
                level: 1,
                text: "Quarterly report".into(),
            },
            Block::ListItem {
                ordered: false,
                spans: vec![span("A bullet point")],
            },
            Block::ListItem {
                ordered: true,
                spans: vec![span("A numbered step")],
            },
            Block::Table {
                rows: vec![
                    vec!["Name".into(), "Qty".into()],
                    vec!["Tea".into(), "2".into()],
                ],
            },
        ];
        for _ in 0..paragraphs {
            page.push(Block::Paragraph {
                spans: vec![span("Words that wrap across the width of the page, again and again, until the line runs out of room.")],
            });
        }
        to_docx(&[page]).unwrap()
    }

    fn convert(docx: Vec<u8>) -> Vec<String> {
        let engine = engine();
        let pdf = engine
            .with_pdfium(move |pdfium| word_to_pdf(pdfium, &docx))
            .unwrap();
        let LoadOutcome::Ready { pages, .. } = engine.load(1, pdf, None).unwrap() else {
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
    fn parsing_reads_styles_lists_and_tables() {
        let parsed = parse(&sample(1)).unwrap();
        let Item::Paragraph(heading) = &parsed.items[0] else {
            panic!("heading");
        };
        let Inline::Text { style, .. } = &heading.inlines[0] else {
            panic!("text");
        };
        assert!(style.bold);
        assert_eq!(style.size, 20.0);
        assert!(heading.before > 0.0);
        let Item::Paragraph(step) = &parsed.items[2] else {
            panic!("list");
        };
        assert_eq!(step.hanging.as_deref(), Some("1."));
        assert!(matches!(&parsed.items[3], Item::Table { rows, .. } if rows.len() == 2));
    }

    #[test]
    fn word_documents_become_readable_pdfs() {
        let text = convert(sample(1));
        assert_eq!(text.len(), 1);
        let page = text[0].split_whitespace().collect::<Vec<_>>().join(" ");
        for expected in [
            "Quarterly report",
            "A bullet point",
            "1.",
            "A numbered step",
            "Tea",
            "again and again",
        ] {
            assert!(page.contains(expected), "{expected} in {page}");
        }
    }

    #[test]
    fn long_documents_flow_onto_more_pages() {
        assert!(convert(sample(60)).len() >= 2);
    }

    /// A package whose body is `body`, for shapes the writer never produces.
    fn package(body: &str) -> Vec<u8> {
        use std::io::Write;
        let document = format!(
            r#"<w:document xmlns:w="{WORD_NS}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><w:body>{body}</w:body></w:document>"#
        );
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(document.as_bytes()).unwrap();
        archive.finish().unwrap().into_inner()
    }

    fn texts(bytes: &[u8]) -> Vec<String> {
        parse(bytes)
            .unwrap()
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Paragraph(paragraph) => Some(
                    paragraph
                        .inlines
                        .iter()
                        .filter_map(|inline| match inline {
                            Inline::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                Item::Table { .. } => None,
            })
            .collect()
    }

    #[test]
    fn text_boxes_appear_once_after_their_paragraph() {
        let body = r#"<w:p><w:r><w:t>Anchor</w:t></w:r><w:r><mc:AlternateContent><mc:Choice><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></mc:Choice><mc:Fallback><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></mc:Fallback></mc:AlternateContent></w:r></w:p>"#;
        assert_eq!(texts(&package(body)), ["Anchor", "Boxed"]);
    }

    #[test]
    fn content_controls_keep_their_text_and_deletions_stay_gone() {
        let body = r#"<w:sdt><w:sdtContent><w:p><w:sdt><w:sdtContent><w:r><w:t>Filled</w:t></w:r></w:sdtContent></w:sdt><w:del><w:r><w:t>Gone</w:t></w:r></w:del></w:p></w:sdtContent></w:sdt>"#;
        assert_eq!(texts(&package(body)), ["Filled"]);
    }

    #[test]
    fn files_that_are_not_word_documents_fail_clearly() {
        let error = parse(b"not a zip").err().unwrap();
        assert!(error.contains("Word file"));
    }
}
