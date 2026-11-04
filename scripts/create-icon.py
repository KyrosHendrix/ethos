"""Render the ethos mark: a pixel-snapped bitmap per icon size, plus the SVG used in-app.

Small sizes are drawn on their own pixel grid instead of downscaled, so the bars stay
crisp on the taskbar at every Windows display scale.
"""
from pathlib import Path
from PIL import Image, ImageDraw

root = Path(__file__).resolve().parent.parent
build = root / "build"
public = root / "public"
build.mkdir(exist_ok=True)

TOP, BOTTOM = (50, 94, 73), (36, 70, 54)  # forest, lit from above
PAPER = (250, 249, 244)
SUPER = 4  # supersampling factor; geometry is snapped before scaling up
# Windows picks 16/20/24/30/32/36/40/48/60/64/72/96/256 across display scales.
SIZES = (16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 96, 128, 256)


def squircle(size: float, inset: float, n: float = 4.2, steps: int = 256):
    """Superellipse outline: the continuous-curvature corner Apple uses for app tiles."""
    import math

    half = size / 2 - inset
    points = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        points.append(
            (
                size / 2 + half * math.copysign(abs(c) ** (2 / n), c),
                size / 2 + half * math.copysign(abs(s) ** (2 / n), s),
            )
        )
    return points


def glyph(size: int):
    """Margin E bars in 32-unit design space, snapped to whole pixels at this size."""
    k = size / 32
    t = max(1, round(2.2 * k))
    left = round(9 * k)
    top = round(9 * k)
    height = round(14 * k)
    if (height - t) % 2:  # keep the middle bar exactly centred
        height += 1
    gap = max(1, round(2.6 * k))
    x = left + t + gap
    long = round(23.2 * k) - x
    short = round(long * 0.68)
    middle = top + (height - t) // 2
    return t, [
        (left, top, t, height),
        (x, top, long, t),
        (x, middle, short, t),
        (x, top + height - t, long, t),
    ]


def render(size: int) -> Image.Image:
    big = size * SUPER
    inset = 0 if size <= 24 else size / 32  # tiny sizes use every pixel
    mask = Image.new("L", (big, big))
    ImageDraw.Draw(mask).polygon(squircle(big, inset * SUPER), fill=255)
    gradient = Image.new("RGB", (1, big))
    for y in range(big):
        f = y / (big - 1)
        gradient.putpixel((0, y), tuple(round(a + (b - a) * f) for a, b in zip(TOP, BOTTOM)))
    tile = Image.new("RGBA", (big, big))
    tile.paste(gradient.resize((big, big)), mask=mask)
    draw = ImageDraw.Draw(tile)
    if size >= 48:  # a hairline of light on the top edge reads as material, not flat paint
        rim = Image.new("L", (big, big))
        ImageDraw.Draw(rim).polygon(squircle(big, inset * SUPER), fill=255)
        ImageDraw.Draw(rim).polygon(
            [(x, y + SUPER * size / 128) for x, y in squircle(big, inset * SUPER)], fill=0
        )
        tile.paste((255, 255, 255, 40), mask=Image.eval(rim, lambda v: v * 40 // 255))
    t, bars = glyph(size)
    for x, y, w, h in bars:
        draw.rounded_rectangle(
            (x * SUPER, y * SUPER, (x + w) * SUPER - 1, (y + h) * SUPER - 1),
            radius=t * SUPER / 2 if t >= 2 else 0,
            fill=PAPER,
        )
    return tile.resize((size, size), Image.Resampling.BOX)


frames = {size: render(size) for size in SIZES}
frames[256].save(
    build / "icon.ico",
    sizes=[(s, s) for s in SIZES],
    append_images=[frames[s] for s in SIZES if s != 256],
)
large = render(512)
large.save(build / "icon.png")
large.save(public / "ethos.png")
frames[256].save(public / "ethos.ico", sizes=[(s, s) for s in SIZES], append_images=[frames[s] for s in SIZES if s != 256])

# In-app mark and favicon share the same geometry.
outline = " ".join(f"{x:.2f},{y:.2f}" for x, y in squircle(64, 0, steps=96))
_, bars = glyph(64)
rects = "\n".join(
    f'  <rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{h / 2 if h < w else w / 2}" fill="#faf9f4"/>'
    for x, y, w, h in bars
)
(public / "ethos.svg").write_text(
    f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" fill="none">
  <defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#325e49"/><stop offset="1" stop-color="#244636"/></linearGradient></defs>
  <polygon points="{outline}" fill="url(#g)"/>
{rects}
</svg>
""",
    encoding="utf-8",
)

# Self-check: every frame exists at its true size and the small bars land on whole pixels.
ico = Image.open(build / "icon.ico")
assert set(ico.info["sizes"]) >= {(s, s) for s in SIZES}
for s in (16, 24, 32):
    ico.size = (s, s)
    frame = ico.convert("RGBA")
    t, bars = glyph(s)
    x, y, w, h = bars[0]
    assert frame.getpixel((x, y + h // 2))[:3] == PAPER, s
    assert frame.getpixel((x - 1, y + h // 2))[:3] != PAPER, s
print("icons:", ", ".join(str(s) for s in SIZES))
