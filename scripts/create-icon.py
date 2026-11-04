"""Render the existing ethos vector mark for Windows app packaging."""
from pathlib import Path
from xml.etree import ElementTree
from PIL import Image, ImageDraw

root = Path(__file__).resolve().parent.parent
target = root / "build"
target.mkdir(exist_ok=True)
scale = 32
image = Image.new("RGBA", (32 * scale, 32 * scale))
draw = ImageDraw.Draw(image)
# shortcut: rectangle-only SVG rasterizer; extend it if the approved mark uses other shapes.
rectangles = list(ElementTree.parse(root / "public" / "ethos.svg").getroot())
assert rectangles and all(element.tag.endswith("}rect") for element in rectangles)
for rectangle in rectangles:
    x = float(rectangle.get("x", "0")) * scale
    y = float(rectangle.get("y", "0")) * scale
    width = float(rectangle.attrib["width"]) * scale
    height = float(rectangle.attrib["height"]) * scale
    radius = float(rectangle.get("rx", "0")) * scale
    draw.rounded_rectangle((x, y, x + width, y + height), radius=radius, fill=rectangle.attrib["fill"])
image = image.resize((512, 512), Image.Resampling.LANCZOS)
image.save(target / "icon.png")
image.save(root / "public" / "ethos.png")
image.save(target / "icon.ico", sizes=[(size, size) for size in (16, 24, 32, 48, 64, 128, 256)])
assert Image.open(target / "icon.png").size == (512, 512)
assert Image.open(target / "icon.ico").format == "ICO"
assert image.getpixel((64, 256))[:3] == (40, 77, 60)
assert image.getpixel((152, 256))[:3] == (250, 249, 244)
