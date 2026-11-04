import test from "node:test";
import assert from "node:assert/strict";
import { PDFDocument, StandardFonts, degrees } from "pdf-lib";
import { getDocument } from "pdfjs-dist/legacy/build/pdf.mjs";
import { createSample, exportPdf, wrapTextNote } from "../src/pdf.ts";

async function fixture() {
  const pdf = await PDFDocument.create();
  const font = await pdf.embedFont(StandardFonts.Helvetica);
  for (let i = 1; i <= 3; i++) {
    const page = pdf.addPage([300 + i * 10, 400]);
    page.drawText(`Original page ${i}`, { x: 20, y: 350, font });
    if (i === 2) page.setRotation(degrees(90));
  }
  pdf.setTitle("Fixture title");
  pdf.setAuthor("Fixture author");
  pdf.setSubject("Fixture subject");
  pdf.setKeywords(["fixture", "roundtrip"]);
  return pdf.save();
}
async function texts(bytes) {
  const task = getDocument({
    data: Uint8Array.from(bytes),
    useSystemFonts: true,
  });
  try {
    const pdf = await task.promise;
    return await Promise.all(
      Array.from({ length: pdf.numPages }, async (_, i) => {
        const content = await (await pdf.getPage(i + 1)).getTextContent();
        return content.items.map((item) => item.str ?? "").join(" ");
      }),
    );
  } finally {
    await task.destroy();
  }
}
const pages = [
  { source: 1, rotation: 0 },
  { source: 2, rotation: 90 },
  { source: 3, rotation: 0 },
];

test("export retains existing form fields and values", async () => {
  const original = await PDFDocument.create();
  const page = original.addPage([400, 500]);
  const field = original.getForm().createTextField("reader-name");
  field.addToPage(page, { x: 30, y: 400, width: 200, height: 24 });
  field.setText("Existing form content");
  const exported = await exportPdf(await original.save(), {
    pages: [{ source: 1, rotation: 0 }],
    marks: [],
  });
  const restored = await PDFDocument.load(exported);
  assert.equal(
    restored.getForm().getTextField("reader-name").getText(),
    "Existing form content",
  );
});
test("sample is a readable four-page PDF with title and author", async () => {
  const bytes = await createSample();
  const pdf = await PDFDocument.load(bytes);
  assert.equal(pdf.getPageCount(), 4);
  assert.equal(pdf.getTitle(), "A little more room to think");
  assert.equal(pdf.getAuthor(), "ethos");
  const content = await texts(bytes);
  assert.match(content[0], /A little more room/);
  assert.doesNotMatch(content.join(" "), /\b(?:Copilot|assistant|AI)\b/i);
  assert.match(content[3], /Save a copy to keep your annotations/);
});
test("export preserves original bytes and metadata", async () => {
  const bytes = await fixture();
  const before = Uint8Array.from(bytes);
  const result = await exportPdf(bytes, { pages, marks: [] });
  assert.deepEqual(bytes, before);
  const pdf = await PDFDocument.load(result);
  assert.equal(pdf.getTitle(), "Fixture title");
  assert.equal(pdf.getAuthor(), "Fixture author");
  assert.equal(pdf.getSubject(), "Fixture subject");
  assert.match(pdf.getKeywords(), /roundtrip/);
  assert.equal(pdf.getCreator(), "ethos");
  assert.deepEqual(await texts(result), await texts(bytes));
});
test("export reorders, rotates and deletes pages by source identity", async () => {
  const bytes = await fixture();
  const result = await exportPdf(bytes, {
    pages: [
      { source: 3, rotation: 270 },
      { source: 1, rotation: 180 },
    ],
    marks: [],
  });
  const pdf = await PDFDocument.load(result);
  assert.equal(pdf.getPageCount(), 2);
  assert.deepEqual(
    pdf.getPages().map((p) => p.getRotation().angle),
    [270, 180],
  );
  assert.deepEqual(
    pdf.getPages().map((p) => p.getWidth()),
    [330, 310],
  );
  assert.deepEqual(await texts(result), ["Original page 3", "Original page 1"]);
  assert.equal((await PDFDocument.load(bytes)).getPageCount(), 3);
});
test("text annotations survive export on reordered source pages and deleted notes disappear", async () => {
  const marks = [1, 2, 3].map((page) => ({
    id: String(page),
    kind: "text",
    page,
    color: "blue",
    point: { x: 20, y: 200 },
    text: `Annotation ${page}`,
  }));
  const result = await exportPdf(await fixture(), {
    pages: [pages[2], pages[0]],
    marks,
  });
  const content = await texts(result);
  assert.match(content[0], /Annotation 3/);
  assert.match(content[1], /Annotation 1/);
  assert.doesNotMatch(content.join(" "), /Annotation 2/);
});
test("highlight and pen marks export into a valid readable PDF", async () => {
  const result = await exportPdf(await fixture(), {
    pages,
    marks: [
      {
        id: "h",
        kind: "highlight",
        page: 1,
        color: "yellow",
        text: "Original",
        rects: [{ x: 20, y: 345, width: 80, height: 15 }],
      },
      {
        id: "p",
        kind: "pen",
        page: 2,
        color: "mint",
        points: [
          { x: 20, y: 20 },
          { x: 100, y: 100 },
        ],
      },
    ],
  });
  assert.equal((await PDFDocument.load(result)).getPageCount(), 3);
  assert.match((await texts(result))[0], /Original page 1/);
});
test("unsupported Unicode produces an actionable export error without changing input", async () => {
  const bytes = await fixture();
  const before = Uint8Array.from(bytes);
  await assert.rejects(
    exportPdf(bytes, {
      pages,
      marks: [
        {
          id: "u",
          kind: "text",
          page: 1,
          color: "rose",
          point: { x: 20, y: 200 },
          text: "你好",
        },
      ],
    }),
    /characters.*font|Western European/i,
  );
  assert.deepEqual(bytes, before);
});
test("text wrapping supports Western European text and rejects unsupported Unicode clearly", async () => {
  assert.match(
    await wrapTextNote("Résumé café and a long thought about reading", 80),
    /\n/,
  );
  await assert.rejects(
    wrapTextNote("你好", 100),
    /Western European.*embedded font/,
  );
});
