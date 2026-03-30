#!/usr/bin/env python3
from __future__ import annotations

import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont


ROOT = Path(__file__).resolve().parents[1]
BRANDING = ROOT / "branding"
GENERATED = BRANDING / "generated"
ICON_DIR = GENERATED / "icon"
SOCIAL_DIR = GENERATED / "social"
ICONSET_DIR = ROOT / "apps" / "syncsteward-macos" / "Bundle" / "AppIcon.iconset"

FONT_REGULAR = "/System/Library/Fonts/Avenir Next.ttc"
FONT_BOLD = "/System/Library/Fonts/Avenir Next.ttc"

PALETTE = {
    "ink": (8, 15, 22, 255),
    "ink_2": (15, 33, 43, 255),
    "ink_3": (24, 49, 63, 255),
    "teal": (40, 210, 193, 255),
    "teal_2": (24, 170, 172, 255),
    "amber": (245, 171, 63, 255),
    "amber_2": (214, 107, 43, 255),
    "cream": (247, 244, 236, 255),
    "mist": (198, 214, 224, 255),
    "slate": (113, 140, 158, 255),
}


def ensure_dirs() -> None:
    for path in [ICON_DIR, SOCIAL_DIR, ICONSET_DIR]:
        path.mkdir(parents=True, exist_ok=True)


def lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t


def mix(c1: tuple[int, int, int, int], c2: tuple[int, int, int, int], t: float) -> tuple[int, int, int, int]:
    return tuple(int(round(lerp(c1[i], c2[i], t))) for i in range(4))


def make_linear_gradient(size: tuple[int, int], start, end, diagonal: bool = True) -> Image.Image:
    width, height = size
    gradient = Image.new("RGBA", size)
    draw = ImageDraw.Draw(gradient)
    for y in range(height):
        for x in range(width):
            if diagonal:
                t = ((x / max(1, width - 1)) * 0.6) + ((y / max(1, height - 1)) * 0.4)
            else:
                t = y / max(1, height - 1)
            t = max(0.0, min(1.0, t))
            draw.point((x, y), fill=mix(start, end, t))
    return gradient


def rounded_mask(size: tuple[int, int], radius: int) -> Image.Image:
    mask = Image.new("L", size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size[0] - 1, size[1] - 1), radius=radius, fill=255)
    return mask


def draw_background(size: int) -> Image.Image:
    image = make_linear_gradient((size, size), PALETTE["ink"], PALETTE["ink_3"])
    mask = rounded_mask((size, size), radius=int(size * 0.22))
    bg = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    bg.paste(image, (0, 0), mask)

    draw = ImageDraw.Draw(bg)
    inset = int(size * 0.045)
    draw.rounded_rectangle(
        (inset, inset, size - inset, size - inset),
        radius=int(size * 0.18),
        outline=(255, 255, 255, 18),
        width=max(2, size // 128),
    )

    grid = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    gdraw = ImageDraw.Draw(grid)
    step = max(22, size // 16)
    for pos in range(-size, size * 2, step):
        gdraw.line((pos, 0, pos - size, size), fill=(255, 255, 255, 10), width=max(1, size // 256))
    bg = Image.alpha_composite(bg, grid.filter(ImageFilter.GaussianBlur(radius=max(1, size // 220))))

    glow = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    gdraw = ImageDraw.Draw(glow)
    gdraw.ellipse(
        (int(size * 0.12), int(size * 0.08), int(size * 0.88), int(size * 0.82)),
        fill=(39, 195, 193, 40),
    )
    gdraw.ellipse(
        (int(size * 0.22), int(size * 0.26), int(size * 0.92), int(size * 0.96)),
        fill=(245, 171, 63, 36),
    )
    return Image.alpha_composite(bg, glow.filter(ImageFilter.GaussianBlur(radius=max(8, size // 24))))


def point_on_circle(cx: float, cy: float, radius: float, angle_deg: float) -> tuple[float, float]:
    radians = math.radians(angle_deg)
    return cx + (radius * math.cos(radians)), cy + (radius * math.sin(radians))


def arrow_head(tip: tuple[float, float], angle_deg: float, length: float, width: float):
    radians = math.radians(angle_deg)
    dx = math.cos(radians)
    dy = math.sin(radians)
    px = -dy
    py = dx
    return [
        tip,
        (tip[0] - (dx * length) + (px * width), tip[1] - (dy * length) + (py * width)),
        (tip[0] - (dx * length) - (px * width), tip[1] - (dy * length) - (py * width)),
    ]


def shield_points(size: int) -> list[tuple[int, int]]:
    return [
        (int(size * 0.50), int(size * 0.21)),
        (int(size * 0.68), int(size * 0.28)),
        (int(size * 0.70), int(size * 0.49)),
        (int(size * 0.62), int(size * 0.67)),
        (int(size * 0.50), int(size * 0.78)),
        (int(size * 0.38), int(size * 0.67)),
        (int(size * 0.30), int(size * 0.49)),
        (int(size * 0.32), int(size * 0.28)),
    ]


def render_mark(size: int) -> Image.Image:
    image = draw_background(size)
    overlay = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(overlay)

    cx = cy = size / 2
    ring_radius = size * 0.245
    ring_width = max(12, size // 16)

    draw.arc(
        (
            cx - ring_radius,
            cy - ring_radius,
            cx + ring_radius,
            cy + ring_radius,
        ),
        start=133,
        end=348,
        fill=PALETTE["teal"],
        width=ring_width,
    )
    draw.arc(
        (
            cx - ring_radius,
            cy - ring_radius,
            cx + ring_radius,
            cy + ring_radius,
        ),
        start=-27,
        end=186,
        fill=PALETTE["amber"],
        width=ring_width,
    )

    teal_tip = point_on_circle(cx, cy, ring_radius, 348)
    amber_tip = point_on_circle(cx, cy, ring_radius, 186)
    draw.polygon(arrow_head(teal_tip, 78, ring_width * 0.9, ring_width * 0.45), fill=PALETTE["teal"])
    draw.polygon(arrow_head(amber_tip, 276, ring_width * 0.9, ring_width * 0.45), fill=PALETTE["amber"])

    glow = overlay.filter(ImageFilter.GaussianBlur(radius=max(8, size // 48)))
    image = Image.alpha_composite(image, glow)
    image = Image.alpha_composite(image, overlay)

    shield = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    sdraw = ImageDraw.Draw(shield)
    shield_poly = shield_points(size)
    sdraw.polygon(shield_poly, fill=PALETTE["cream"])
    sdraw.line(shield_poly + [shield_poly[0]], fill=(255, 255, 255, 110), width=max(2, size // 96))
    shield = shield.filter(ImageFilter.GaussianBlur(radius=max(2, size // 96)))
    image = Image.alpha_composite(image, shield)

    crisp = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    cdraw = ImageDraw.Draw(crisp)
    cdraw.polygon(shield_poly, fill=(247, 244, 236, 230))
    cdraw.line(shield_poly + [shield_poly[0]], fill=(255, 255, 255, 170), width=max(2, size // 128))

    # interior steward path/check
    path_width = max(10, size // 34)
    cdraw.line(
        [
            (int(size * 0.41), int(size * 0.50)),
            (int(size * 0.48), int(size * 0.58)),
            (int(size * 0.60), int(size * 0.40)),
        ],
        fill=PALETTE["ink"],
        width=path_width,
        joint="curve",
    )

    cdraw.arc(
        (int(size * 0.37), int(size * 0.34), int(size * 0.63), int(size * 0.60)),
        start=210,
        end=15,
        fill=(18, 57, 72, 180),
        width=max(4, size // 96),
    )
    image = Image.alpha_composite(image, crisp)

    highlight = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    hdraw = ImageDraw.Draw(highlight)
    hdraw.ellipse(
        (int(size * 0.22), int(size * 0.12), int(size * 0.52), int(size * 0.32)),
        fill=(255, 255, 255, 45),
    )
    image = Image.alpha_composite(image, highlight.filter(ImageFilter.GaussianBlur(radius=max(8, size // 28))))
    return image


def font(path: str, size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(path, size=size)


def render_social_card(width: int, height: int, square: bool = False) -> Image.Image:
    card = make_linear_gradient((width, height), (8, 15, 22, 255), (17, 43, 57, 255))

    # Background atmosphere
    atmosphere = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    adraw = ImageDraw.Draw(atmosphere)
    adraw.ellipse((int(width * 0.04), int(height * 0.12), int(width * 0.46), int(height * 0.88)), fill=(35, 202, 190, 42))
    adraw.ellipse((int(width * 0.45), int(height * 0.10), int(width * 1.05), int(height * 0.90)), fill=(245, 171, 63, 34))
    card = Image.alpha_composite(card, atmosphere.filter(ImageFilter.GaussianBlur(radius=max(18, width // 28))))

    pattern = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    pdraw = ImageDraw.Draw(pattern)
    spacing = max(36, width // 30)
    for x in range(-height, width + height, spacing):
        pdraw.line((x, 0, x - height, height), fill=(255, 255, 255, 10), width=max(1, width // 800))
    card = Image.alpha_composite(card, pattern)
    draw = ImageDraw.Draw(card)

    icon_size = int(min(width, height) * (0.34 if square else 0.42))
    mark = render_mark(icon_size)
    if square:
        icon_x = (width - icon_size) // 2
        icon_y = int(height * 0.11)
        card.alpha_composite(mark, (icon_x, icon_y))
        title_font = font(FONT_BOLD, int(width * 0.085))
        subtitle_font = font(FONT_REGULAR, int(width * 0.032))
        title = "SyncSteward"
        subtitle = "Safety-first sync for the folders you actually care about"
        title_bbox = draw.textbbox((0, 0), title, font=title_font)
        subtitle_bbox = draw.multiline_textbbox((0, 0), subtitle, font=subtitle_font, spacing=8)
        draw.text(((width - (title_bbox[2] - title_bbox[0])) / 2, int(height * 0.56)), title, fill=PALETTE["cream"], font=title_font)
        draw.multiline_text(
            ((width - (subtitle_bbox[2] - subtitle_bbox[0])) / 2, int(height * 0.68)),
            subtitle,
            fill=PALETTE["mist"],
            font=subtitle_font,
            spacing=8,
            align="center",
        )
    else:
        icon_x = int(width * 0.08)
        icon_y = (height - icon_size) // 2
        card.alpha_composite(mark, (icon_x, icon_y))

        badge_font = font(FONT_BOLD, int(height * 0.045))
        title_font = font(FONT_BOLD, int(height * 0.15))
        subtitle_font = font(FONT_REGULAR, int(height * 0.058))

        badge = "SAFETY-FIRST SYNC"
        badge_bbox = draw.textbbox((0, 0), badge, font=badge_font)
        text_left = int(width * 0.47)
        badge_box = (
            text_left,
            int(height * 0.22),
            text_left + (badge_bbox[2] - badge_bbox[0]) + int(width * 0.03),
            int(height * 0.22) + (badge_bbox[3] - badge_bbox[1]) + int(height * 0.045),
        )
        draw.rounded_rectangle(badge_box, radius=int(height * 0.04), fill=(255, 255, 255, 24))
        draw.text(
            (badge_box[0] + int(width * 0.015), badge_box[1] + int(height * 0.014)),
            badge,
            fill=PALETTE["teal"],
            font=badge_font,
        )

        draw.text((text_left, int(height * 0.38)), "SyncSteward", fill=PALETTE["cream"], font=title_font)
        draw.multiline_text(
            (text_left, int(height * 0.58)),
            "Safer sync for the folders\nyou actually care about",
            fill=PALETTE["mist"],
            font=subtitle_font,
            spacing=int(height * 0.02),
        )

    return card


def save_icon_variants(master: Image.Image) -> None:
    icon_sizes = [1024, 512, 256, 192, 180, 128, 64, 48, 32, 16]
    for size in icon_sizes:
        master.resize((size, size), Image.Resampling.LANCZOS).save(ICON_DIR / f"syncsteward-icon-{size}.png")
    master.save(ICON_DIR / "syncsteward-icon-master-1024.png")
    master.save(ICON_DIR / "syncsteward-github-avatar-1024.png")

    iconset_map = {
        "icon_16x16.png": 16,
        "icon_16x16@2x.png": 32,
        "icon_32x32.png": 32,
        "icon_32x32@2x.png": 64,
        "icon_128x128.png": 128,
        "icon_128x128@2x.png": 256,
        "icon_256x256.png": 256,
        "icon_256x256@2x.png": 512,
        "icon_512x512.png": 512,
        "icon_512x512@2x.png": 1024,
    }
    for name, size in iconset_map.items():
        master.resize((size, size), Image.Resampling.LANCZOS).save(ICONSET_DIR / name)


def save_social_variants() -> None:
    variants = {
        "syncsteward-github-social-1280x640.png": (1280, 640, False),
        "syncsteward-open-graph-1200x630.png": (1200, 630, False),
        "syncsteward-social-square-1200.png": (1200, 1200, True),
        "syncsteward-social-wide-1600x900.png": (1600, 900, False),
    }
    for filename, (width, height, square) in variants.items():
        render_social_card(width, height, square=square).save(SOCIAL_DIR / filename)


def main() -> None:
    ensure_dirs()
    master = render_mark(1024)
    save_icon_variants(master)
    save_social_variants()


if __name__ == "__main__":
    main()
