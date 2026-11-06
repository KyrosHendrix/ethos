use lopdf::{Dictionary, Document, Object, Stream, dictionary};

struct Chapter {
    label: &'static str,
    title: [&'static str; 2],
    text: [&'static str; 5],
}

const CHAPTERS: [Chapter; 4] = [
    Chapter {
        label: "A NOTE ON ATTENTION",
        title: ["A little more room", "to think."],
        text: [
            "The best tools give you space. Space to read closely, follow an idea,",
            "and make something your own. This is the thinking behind ethos.",
            "",
            "A document is more than a file. It can be the beginning of a project,",
            "a question worth sitting with, or a conversation with a different mind.",
        ],
    },
    Chapter {
        label: "01 / MAKE IT YOURS",
        title: ["Follow the thought.", "Leave a mark."],
        text: [
            "Select a passage and highlight what matters. Add a text note beside",
            "an idea. Draw a connection with the pen. You can undo every mark.",
            "",
            "Your marks are kept for you, even after you close the document.",
            "The original document stays exactly as you found it.",
        ],
    },
    Chapter {
        label: "02 / LESS, BUT CONSIDERED",
        title: ["Only what you need.", "When you need it."],
        text: [
            "Use the page sidebar to move through a document. Search for a word.",
            "Fit the page to your workspace, or zoom in to see the finer details.",
            "",
            "Pages render as you reach them. Close a document to release it.",
            "A quiet workspace begins with tools that know when to step back.",
        ],
    },
    Chapter {
        label: "03 / SAVE A COPY",
        title: ["Keep your work.", "Keep the original."],
        text: [
            "Save a copy to keep your annotations and page changes.",
            "Choose a new filename. Your original document stays untouched.",
            "",
            "Your marks travel with the copy as real annotations.",
            "Open that copy again and you can keep editing them.",
        ],
    },
];

type Color = [f64; 3];
const MUTED: Color = [0.43, 0.45, 0.44];
const DARK: Color = [0.13, 0.18, 0.16];
const GREEN: Color = [0.25, 0.43, 0.36];
const PAPER: Color = [0.99, 0.985, 0.965];
const RULE: Color = [0.8, 0.82, 0.78];

const REGULAR: &str = "Regular";
const BOLD: &str = "Bold";
const SERIF: &str = "Serif";

fn rectangle(page: &mut String, [r, g, b]: Color, x: f64, y: f64, width: f64, height: f64) {
    page.push_str(&format!("{r} {g} {b} rg {x} {y} {width} {height} re f\n"));
}

fn text(page: &mut String, font: &str, size: f64, [r, g, b]: Color, x: f64, y: f64, line: &str) {
    page.push_str(&format!(
        "BT {r} {g} {b} rg /{font} {size} Tf {x} {y} Td ({line}) Tj ET\n"
    ));
}

fn page_content(index: usize, chapter: &Chapter) -> String {
    let mut page = String::new();
    rectangle(&mut page, PAPER, 0.0, 0.0, 612.0, 792.0);
    text(&mut page, BOLD, 22.0, DARK, 56.0, 726.0, "ethos");
    text(
        &mut page,
        REGULAR,
        9.0,
        MUTED,
        380.0,
        730.0,
        "FIELD NOTES / VOL. 001",
    );
    let [r, g, b] = RULE;
    page.push_str(&format!("{r} {g} {b} RG 0.6 w 56 702 m 556 702 l S\n"));
    text(&mut page, BOLD, 10.0, GREEN, 56.0, 624.0, chapter.label);
    for (line, title) in chapter.title.iter().enumerate() {
        text(
            &mut page,
            SERIF,
            55.0,
            DARK,
            53.0,
            553.0 - line as f64 * 61.0,
            title,
        );
    }
    for (line, body) in chapter
        .text
        .iter()
        .enumerate()
        .filter(|(_, body)| !body.is_empty())
    {
        text(
            &mut page,
            REGULAR,
            12.0,
            MUTED,
            56.0,
            405.0 - line as f64 * 22.0,
            body,
        );
    }
    rectangle(&mut page, GREEN, 56.0, 144.0, 3.0, 73.0);
    let quote = if index == 0 {
        "Good ideas deserve your full attention."
    } else {
        "Read. Reflect. Make it yours."
    };
    text(&mut page, SERIF, 16.0, DARK, 76.0, 184.0, quote);
    text(
        &mut page,
        REGULAR,
        10.0,
        MUTED,
        76.0,
        160.0,
        "A quieter way to work with documents.",
    );
    text(
        &mut page,
        REGULAR,
        8.0,
        MUTED,
        56.0,
        49.0,
        "THE ETHOS READER",
    );
    text(
        &mut page,
        REGULAR,
        9.0,
        DARK,
        540.0,
        49.0,
        &format!("{:02}", index + 1),
    );
    page
}

fn font(document: &mut Document, name: &str) -> Object {
    document
        .add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => name,
            "Encoding" => "WinAnsiEncoding",
        })
        .into()
}

/// Builds the four-page welcome document offered from the empty library.
pub fn create_sample() -> Result<Vec<u8>, String> {
    let mut document = Document::with_version("1.7");
    let pages = document.new_object_id();
    let mut fonts = Dictionary::new();
    fonts.set(REGULAR, font(&mut document, "Helvetica"));
    fonts.set(BOLD, font(&mut document, "Helvetica-Bold"));
    fonts.set(SERIF, font(&mut document, "Times-Roman"));
    let kids: Vec<Object> = CHAPTERS
        .iter()
        .enumerate()
        .map(|(index, chapter)| {
            let content = Stream::new(Dictionary::new(), page_content(index, chapter).into_bytes());
            let content = document.add_object(content);
            document
                .add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages,
                    "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                    "Contents" => content,
                    "Resources" => dictionary! { "Font" => fonts.clone() },
                })
                .into()
        })
        .collect();
    document.objects.insert(
        pages,
        dictionary! {
            "Type" => "Pages",
            "Count" => kids.len() as i64,
            "Kids" => kids,
        }
        .into(),
    );
    let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    let info = document.add_object(dictionary! {
        "Title" => Object::string_literal("A little more room to think"),
        "Author" => Object::string_literal("ethos"),
    });
    document.trailer.set("Root", catalog);
    document.trailer.set("Info", info);
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}
