"""Draw the program icons (original art in the game's paper-cutout style, not taken from the
game) and write them as `.ico` files and as compiled Windows resources (`.res`) that the
game's and the launcher's `build.rs` hand to the linker.

    python tools/make_icons.py            # writes packaging/icons/*.ico and *.res
"""

from __future__ import annotations

import io
import struct
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

SIZES = (16, 24, 32, 48, 64, 128, 256)
BIG = 1024
OUT = Path(__file__).resolve().parent.parent / "packaging" / "icons"

CARD = (176, 132, 78, 255)
CARD_DARK = (150, 110, 62, 255)
INK = (24, 22, 20, 255)
HELMET = (52, 120, 196, 255)


def cardboard(box: tuple[int, int, int, int], radius: int) -> Image.Image:
    img = Image.new("RGBA", (BIG, BIG), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    draw.rounded_rectangle(box, radius, fill=CARD)
    mask = img.getchannel("A")
    x0, y0, x1, y1 = box
    for x in range(x0 + 24, x1 - 12, 36):  # corrugation
        draw.line([(x, y0), (x, y1)], fill=CARD_DARK, width=8)
    img.putalpha(mask)
    return img


def paper(shape: Image.Image, offset: int) -> Image.Image:
    """The shape over its own soft drop shadow, like the game's cut-out sprites."""
    alpha = shape.getchannel("A")
    shadow = Image.new("RGBA", shape.size, (0, 0, 0, 0))
    shadow.putalpha(alpha.point(lambda a: a * 150 // 255))
    shadow = shadow.filter(ImageFilter.GaussianBlur(offset // 2))
    out = Image.new("RGBA", shape.size, (0, 0, 0, 0))
    out.alpha_composite(shadow, (offset, offset))
    out.alpha_composite(shape)
    return out


def stick_figure(gear: bool) -> Image.Image:
    img = Image.new("RGBA", (BIG, BIG), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    w = 52
    head = (330, 250)
    d.ellipse([head[0] - 95, head[1] - 95, head[0] + 95, head[1] + 95], fill=INK)
    d.chord([head[0] - 110, head[1] - 125, head[0] + 110, head[1] + 70], 180, 360, fill=HELMET)
    hip = (360, 640)
    neck = (340, 345)
    d.line([neck, hip], fill=INK, width=w)
    d.line([hip, (250, 900)], fill=INK, width=w, joint="curve")
    d.line([hip, (470, 890)], fill=INK, width=w, joint="curve")
    shoulder = (345, 420)
    d.line([shoulder, (520, 470), (620, 420)], fill=INK, width=w, joint="curve")
    d.line([shoulder, (470, 520), (600, 470)], fill=INK, width=w, joint="curve")
    # Rifle.
    d.polygon([(480, 380), (930, 330), (935, 380), (650, 420), (640, 500), (580, 505), (575, 440), (470, 455)], fill=INK)
    for x, y in (head, hip, (250, 900), (470, 890)):
        d.ellipse([x - w // 2, y - w // 2, x + w // 2, y + w // 2], fill=INK)
    if gear:
        cx, cy, r = 760, 760, 150
        teeth = []
        import math

        for i in range(16):
            a = i * math.pi / 8
            rr = r + (45 if i % 2 == 0 else 0)
            teeth.append((cx + rr * math.cos(a), cy + rr * math.sin(a)))
        d.polygon(teeth, fill=(225, 225, 220, 255))
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=(225, 225, 220, 255))
        d.ellipse([cx - 60, cy - 60, cx + 60, cy + 60], fill=(0, 0, 0, 0))
    return img


def icon(gear: bool) -> Image.Image:
    base = Image.new("RGBA", (BIG, BIG), (0, 0, 0, 0))
    board = cardboard((40, 40, BIG - 70, BIG - 70), 110)
    base.alpha_composite(paper(board, 28))
    base.alpha_composite(paper(stick_figure(gear), 20))
    return base


def ico_bytes(img: Image.Image) -> list[tuple[int, bytes]]:
    """Each size as PNG data (Windows Vista and later read PNG icon images at any size)."""
    images = []
    for size in SIZES:
        buf = io.BytesIO()
        img.resize((size, size), Image.LANCZOS).save(buf, "PNG")
        images.append((size, buf.getvalue()))
    return images


def write_ico(path: Path, images: list[tuple[int, bytes]]) -> None:
    head = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    for size, data in images:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    path.write_bytes(head + entries + b"".join(d for _, d in images))


def res_entry(type_id: int, name_id: int, data: bytes, flags: int) -> bytes:
    header = struct.pack("<HHHH", 0xFFFF, type_id, 0xFFFF, name_id)
    header += struct.pack("<IHHII", 0, flags, 0x0409, 0, 0)
    entry = struct.pack("<II", len(data), 8 + len(header)) + header + data
    return entry + b"\0" * (-len(entry) % 4)


def write_res(path: Path, images: list[tuple[int, bytes]]) -> None:
    """A compiled resource file: the empty leading entry, one RT_ICON per image and the
    RT_GROUP_ICON (id 1, the icon Explorer shows) listing them."""
    out = struct.pack("<II", 0, 32) + struct.pack("<HHHH", 0xFFFF, 0, 0xFFFF, 0) + b"\0" * 16
    group = struct.pack("<HHH", 0, 1, len(images))
    for i, (size, data) in enumerate(images, start=1):
        out += res_entry(3, i, data, 0x1010)
        group += struct.pack("<BBBBHHIH", size % 256, size % 256, 0, 0, 1, 32, len(data), i)
    out += res_entry(14, 1, group, 0x1030)
    path.write_bytes(out)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, gear in (("game", False), ("launcher", True)):
        img = icon(gear)
        images = ico_bytes(img)
        write_ico(OUT / f"{name}.ico", images)
        write_res(OUT / f"{name}.res", images)
        print(f"wrote {name}.ico, {name}.res")


if __name__ == "__main__":
    main()
