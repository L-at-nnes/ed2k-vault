"""Renders assets/icon.png and assets/icon.ico (run: python assets/make_icon.py)."""
from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 1024


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def render() -> Image.Image:
    top, bottom = (27, 42, 58), (14, 22, 33)
    bg = Image.new("RGBA", (SIZE, SIZE))
    px = bg.load()
    for y in range(SIZE):
        row = lerp(top, bottom, y / (SIZE - 1)) + (255,)
        for x in range(SIZE):
            px[x, y] = row

    mask = Image.new("L", (SIZE, SIZE), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, SIZE - 1, SIZE - 1), radius=230, fill=255)
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    img.paste(bg, (0, 0), mask)

    d = ImageDraw.Draw(img)
    cream, amber = (236, 230, 216, 255), (242, 169, 59, 255)
    c = SIZE // 2

    # vault door: thick ring
    outer, inner = 372, 292
    d.ellipse((c - outer, c - outer, c + outer, c + outer), fill=cream)
    d.ellipse((c - inner, c - inner, c + inner, c + inner), fill=(18, 30, 43, 255))
    # eight bolts around the door
    import math
    for i in range(8):
        a = math.radians(i * 45 + 22.5)
        r = (outer + inner) / 2
        bx, by = c + r * math.cos(a), c + r * math.sin(a)
        d.ellipse((bx - 17, by - 17, bx + 17, by + 17), fill=(18, 30, 43, 255))

    # hash sign in the middle
    bar, span, gap = 46, 190, 78
    for off in (-gap, gap):
        d.rounded_rectangle((c + off - bar // 2, c - span, c + off + bar // 2, c + span), radius=bar // 2, fill=amber)
        d.rounded_rectangle((c - span, c + off - bar // 2, c + span, c + off + bar // 2), radius=bar // 2, fill=amber)
    return img


def main() -> None:
    out = Path(__file__).parent
    img = render()
    img.resize((256, 256), Image.LANCZOS).save(out / "icon.png")
    img.save(out / "icon.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])


if __name__ == "__main__":
    main()
