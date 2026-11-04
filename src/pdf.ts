import type { PDFDocument as PdfDocument } from "pdf-lib";

export const inkColors = {
  yellow: "#f4ce56",
  mint: "#78bda3",
  blue: "#86aee8",
  rose: "#db96ac",
};
export const strokeColors = {
  yellow: "#9c720b",
  mint: "#34785c",
  blue: "#365f9d",
  rose: "#9c4263",
};
export type InkColor = keyof typeof inkColors;
export type Point = { x: number; y: number };
export type Rect = Point & { width: number; height: number };
type MarkBase = { id: string; page: number; color: InkColor };
export type Mark = MarkBase &
  (
    | { kind: "highlight"; rects: Rect[]; text: string }
    | { kind: "pen"; points: Point[] }
    | { kind: "text"; point: Point; text: string }
  );
export type PageEntry = { source: number; rotation: number };
export type Edits = { marks: Mark[]; pages: PageEntry[] };

export async function wrapTextNote(text: string, width: number) {
  const { PDFDocument, StandardFonts } = await import("pdf-lib");
  const pdf = await PDFDocument.create();
  const font = await pdf.embedFont(StandardFonts.Helvetica);
  const lines: string[] = [];
  let line = "";
  try {
    for (const word of text.trim().split(/\s+/)) {
      const candidate = line ? `${line} ${word}` : word;
      if (font.widthOfTextAtSize(candidate, 12) <= width) {
        line = candidate;
        continue;
      }
      if (line) {
        lines.push(line);
        line = "";
      }
      for (const character of word) {
        if (line && font.widthOfTextAtSize(line + character, 12) > width) {
          lines.push(line);
          line = "";
        }
        line += character;
      }
    }
    if (line) lines.push(line);
    font.encodeText(lines.join("\n").replace(/\n/g, " "));
  } catch {
    throw new Error(
      "Text notes currently support Western European characters. Other writing systems need an embedded font.",
    );
  }
  return lines.join("\n");
}

export async function exportPdf(bytes: Uint8Array, edits: Edits) {
  const { PDFDocument, StandardFonts, rgb, degrees } = await import("pdf-lib");
  function pdfColor(hex: string) {
    return rgb(
      parseInt(hex.slice(1, 3), 16) / 255,
      parseInt(hex.slice(3, 5), 16) / 255,
      parseInt(hex.slice(5, 7), 16) / 255,
    );
  }
  let original: PdfDocument;
  try {
    original = await PDFDocument.load(bytes);
  } catch (error) {
    if (
      error instanceof Error &&
      error.message.toLowerCase().includes("encrypt")
    )
      throw new Error(
        "Protected PDFs can be read here, but exporting them is not supported yet.",
      );
    throw error;
  }
  const output = original;
  const font = await output.embedFont(StandardFonts.Helvetica);
  const sourcePages = original.getPages();
  const pages = edits.pages.map((entry) => sourcePages[entry.source - 1]);
  for (let i = output.getPageCount() - 1; i >= 0; i--) output.removePage(i);
  for (const [index, page] of pages.entries()) {
    const entry = edits.pages[index];
    output.addPage(page);
    page.setRotation(degrees(entry.rotation));
    for (const mark of edits.marks.filter(
      (item) => item.page === entry.source,
    )) {
      const color = pdfColor(
        mark.kind === "highlight"
          ? inkColors[mark.color]
          : strokeColors[mark.color],
      );
      if (mark.kind === "highlight")
        for (const rect of mark.rects)
          page.drawRectangle({ ...rect, color, opacity: 0.32 });
      if (mark.kind === "pen")
        for (let i = 1; i < mark.points.length; i++)
          page.drawLine({
            start: mark.points[i - 1],
            end: mark.points[i],
            thickness: 1.8,
            color,
            opacity: 0.95,
          });
      if (mark.kind === "text") {
        try {
          page.drawText(mark.text, {
            ...mark.point,
            size: 12,
            font,
            color,
            lineHeight: 17,
          });
        } catch {
          throw new Error(
            "This note contains characters the export font cannot display. Use Western European text for now.",
          );
        }
      }
    }
  }
  output.setTitle(original.getTitle() ?? "Document");
  output.setAuthor(original.getAuthor() ?? "");
  output.setSubject(original.getSubject() ?? "");
  output.setCreator("ethos");
  return output.save();
}

export async function createSample() {
  const { PDFDocument, StandardFonts, rgb } = await import("pdf-lib");
  const pdf = await PDFDocument.create();
  const regular = await pdf.embedFont(StandardFonts.Helvetica);
  const bold = await pdf.embedFont(StandardFonts.HelveticaBold);
  const serif = await pdf.embedFont(StandardFonts.TimesRoman);
  const muted = rgb(0.43, 0.45, 0.44);
  const dark = rgb(0.13, 0.18, 0.16);
  const green = rgb(0.25, 0.43, 0.36);
  const chapters = [
    {
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
    {
      label: "01 / MAKE IT YOURS",
      title: ["Follow the thought.", "Leave a mark."],
      text: [
        "Select a passage and highlight what matters. Add a text note beside",
        "an idea. Draw a connection with the pen. You can undo every mark.",
        "",
        "Your annotations stay in this session until you save a copy.",
        "The original document stays exactly as you found it.",
      ],
    },
    {
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
    {
      label: "03 / SAVE A COPY",
      title: ["Keep your work.", "Keep the original."],
      text: [
        "Save a copy to keep your annotations and page changes.",
        "Choose a new filename. Your original document stays untouched.",
        "",
        "Your marks become part of the exported page content.",
        "Save before closing to keep the work from this session.",
      ],
    },
  ];
  chapters.forEach((chapter, index) => {
    const page = pdf.addPage([612, 792]);
    page.drawRectangle({
      x: 0,
      y: 0,
      width: 612,
      height: 792,
      color: rgb(0.99, 0.985, 0.965),
    });
    page.drawText("ethos", {
      x: 56,
      y: 726,
      size: 22,
      font: bold,
      color: dark,
    });
    page.drawText("FIELD NOTES / VOL. 001", {
      x: 380,
      y: 730,
      size: 9,
      font: regular,
      color: muted,
    });
    page.drawLine({
      start: { x: 56, y: 702 },
      end: { x: 556, y: 702 },
      thickness: 0.6,
      color: rgb(0.8, 0.82, 0.78),
    });
    page.drawText(chapter.label, {
      x: 56,
      y: 624,
      size: 10,
      font: bold,
      color: green,
    });
    chapter.title.forEach((line, lineIndex) =>
      page.drawText(line, {
        x: 53,
        y: 553 - lineIndex * 61,
        size: 55,
        font: serif,
        color: dark,
      }),
    );
    chapter.text.forEach((line, lineIndex) =>
      page.drawText(line, {
        x: 56,
        y: 405 - lineIndex * 22,
        size: 12,
        font: regular,
        color: muted,
      }),
    );
    page.drawRectangle({ x: 56, y: 144, width: 3, height: 73, color: green });
    page.drawText(
      index === 0
        ? "Good ideas deserve your full attention."
        : "Read. Reflect. Make it yours.",
      { x: 76, y: 184, size: 16, font: serif, color: dark },
    );
    page.drawText("A quieter way to work with documents.", {
      x: 76,
      y: 160,
      size: 10,
      font: regular,
      color: muted,
    });
    page.drawText("THE ETHOS READER", {
      x: 56,
      y: 49,
      size: 8,
      font: regular,
      color: muted,
    });
    page.drawText(String(index + 1).padStart(2, "0"), {
      x: 540,
      y: 49,
      size: 9,
      font: regular,
      color: dark,
    });
  });
  pdf.setTitle("A little more room to think");
  pdf.setAuthor("ethos");
  return pdf.save();
}
