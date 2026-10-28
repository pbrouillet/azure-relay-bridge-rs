"""Build review-stage brand assets; never copies assets into the application."""

import argparse
import base64
from datetime import datetime, timezone
import hashlib
import html
import io
import json
from pathlib import Path
import shutil

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from PIL import Image, ImageDraw, ImageFont

from generate import write_json


TAGLINE = "Outbound paths. Clear control."
INK = "#182B3A"
BLUE = "#0078D4"
CHALK = "#F3F5F7"
THEMES = {
    "light": {"background": CHALK, "ink": INK, "blue": BLUE},
    "dark": {"background": "#1F1F1F", "ink": CHALK, "blue": "#479EF5"},
}
BRACKETS = [
    [(8, 40), (8, 8), (40, 8), (40, 16), (16, 16), (16, 40)],
    [(24, 56), (56, 56), (56, 24), (48, 24), (48, 48), (24, 48)],
]


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def mark(ink: str, blue: str) -> str:
    return "".join(
        f'<path fill="{color}" d="M' + " ".join(f"{x},{y}" for x, y in points) + 'Z"/>'
        for points, color in zip(BRACKETS, (ink, blue), strict=True)
    )


def svg(width: float, height: float, content: str, title: str = "azbridge") -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width:g} {height:g}" '
        f'role="img" aria-label="{html.escape(title, quote=True)}">'
        f"<title>{html.escape(title)}</title>{content}</svg>\n"
    )


def outline(font: TTFont, text: str, size: float, x: float, baseline: float, color: str) -> tuple[str, float]:
    glyphs = font.getGlyphSet()
    cmap = font.getBestCmap()
    scale = size / font["head"].unitsPerEm
    start = x
    paths = []
    for character in text:
        name = cmap.get(ord(character))
        if name is None:
            raise ValueError(f"Wordmark font has no glyph for {character!r}")
        pen = SVGPathPen(glyphs, ntos=lambda number: f"{number:.3f}".rstrip("0").rstrip(".") or "0")
        glyphs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, x, baseline)))
        if pen.getCommands():
            paths.append(f'<path fill="{color}" d="{pen.getCommands()}"/>')
        x += glyphs[name].width * scale - size * 0.02
    return "".join(paths), x - start + size * 0.02


def make_vectors(font: TTFont, output: Path) -> None:
    for theme, colors in {
        **THEMES, "mono": {"ink": "currentColor", "blue": "currentColor"},
    }.items():
        ink, blue = colors["ink"], colors["blue"]
        (output / f"mark-{theme}.svg").write_text(svg(64, 64, mark(ink, blue)), encoding="utf-8")
        word, width = outline(font, "azbridge", 48, 8, 46, ink)
        (output / f"wordmark-{theme}.svg").write_text(svg(width + 16, 64, word), encoding="utf-8")
        word, width = outline(font, "azbridge", 48, 80, 45, ink)
        descriptor, _ = outline(font, "Relay control plane", 12, 82, 64, ink)
        content = f'<g transform="translate(0 8)">{mark(ink, blue)}</g>{word}{descriptor}'
        (output / f"lockup-horizontal-{theme}.svg").write_text(
            svg(width + 92, 80, content, "azbridge - Relay control plane"), encoding="utf-8",
        )
        _, word_width = outline(font, "azbridge", 44, 0, 0, ink)
        word, _ = outline(font, "azbridge", 44, (280 - word_width) / 2, 172, ink)
        _, descriptor_width = outline(font, "Relay control plane", 14, 0, 0, ink)
        descriptor, _ = outline(font, "Relay control plane", 14, (280 - descriptor_width) / 2, 196, ink)
        content = f'<g transform="translate(76 0) scale(2)">{mark(ink, blue)}</g>{word}{descriptor}'
        (output / f"lockup-stacked-{theme}.svg").write_text(
            svg(280, 212, content, "azbridge - Relay control plane"), encoding="utf-8",
        )
    adaptive = (
        '<style>.ink{fill:#182B3A}.blue{fill:#0078D4}'
        '@media(prefers-color-scheme:dark){.ink{fill:#F3F5F7}.blue{fill:#479EF5}}</style>'
        + mark(INK, BLUE).replace(f'fill="{INK}"', 'class="ink"').replace(f'fill="{BLUE}"', 'class="blue"')
    )
    (output / "favicon.svg").write_text(svg(64, 64, adaptive), encoding="utf-8")


def icon(size: int, maskable: bool = False) -> Image.Image:
    image = Image.new("RGBA", (size * 4, size * 4), CHALK)
    draw = ImageDraw.Draw(image)
    scale = size * 4 / 64
    shrink = 0.70 if maskable else 1
    for points, color in zip(BRACKETS, (INK, BLUE), strict=True):
        draw.polygon(
            [((32 + (x - 32) * shrink) * scale, (32 + (y - 32) * shrink) * scale) for x, y in points],
            fill=color,
        )
    return image.resize((size, size), Image.Resampling.LANCZOS)


def make_icons(output: Path) -> None:
    icon(64).save(output / "favicon.ico", sizes=[(16, 16), (32, 32), (48, 48)])
    for size, name in [(180, "apple-touch-icon.png"), (192, "icon-192.png"), (512, "icon-512.png")]:
        icon(size).save(output / name, optimize=True)
    icon(512, maskable=True).save(output / "icon-maskable-512.png", optimize=True)
    write_json(output / "site.webmanifest", {
        "id": "/", "name": "azbridge - Relay control plane", "short_name": "azbridge",
        "description": TAGLINE, "start_url": "/", "scope": "/", "display": "standalone",
        "background_color": CHALK, "theme_color": BLUE,
        "icons": [
            {"src": "/brand/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any"},
            {"src": "/brand/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any"},
            {"src": "/brand/icon-maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable"},
        ],
    })


def artwork(source: Path, output: Path) -> list[dict]:
    entries = json.loads((source / "provenance.json").read_text(encoding="utf-8"))
    expected = {f"{name}-{theme}.png" for name in ("hero", "connections", "profiles", "gateways") for theme in THEMES}
    if {entry["asset"] for entry in entries} != expected or len(entries) != 8:
        raise ValueError("Final artwork requires exactly eight recorded light/dark images")
    for entry in entries:
        path = source / entry["asset"]
        if digest(path) != entry["sha256"]:
            raise ValueError(f"Original artwork checksum mismatch: {path.name}")
        image = Image.open(path).convert("RGB")
        hero = path.name.startswith("hero-")
        image = image.resize((1200, 800) if hero else (480, 480), Image.Resampling.LANCZOS)
        destination = output / path.with_suffix(".webp").name
        buffer = io.BytesIO()
        image.save(buffer, "WEBP", quality=88, method=6)
        limit = 150_000 if hero else 40_000
        if len(buffer.getvalue()) > limit:
            raise ValueError(f"Artwork exceeds its byte budget: {destination.name}")
        destination.write_bytes(buffer.getvalue())
        entry["sourceAsset"] = entry.pop("asset")
        entry["sourceSha256"] = entry.pop("sha256")
        entry["asset"] = f"assets/{destination.name}"
        entry["sha256"] = digest(destination)
        entry["postProcessing"] = f"Resized to {image.width}x{image.height}; WebP quality=88 method=6"
    return entries


def pil_font(font_path: Path, size: int) -> ImageFont.FreeTypeFont:
    font = ImageFont.truetype(str(font_path), size)
    font.set_variation_by_axes([600])
    return font


def make_banners(output: Path, font: TTFont, font_path: Path) -> None:
    for theme, colors in THEMES.items():
        image_path = output / f"hero-{theme}.webp"
        encoded = base64.b64encode(image_path.read_bytes()).decode("ascii")
        word, _ = outline(font, "azbridge", 94, 64, 196, colors["ink"])
        descriptor, _ = outline(font, "Relay control plane", 24, 68, 244, colors["ink"])
        tagline, _ = outline(font, TAGLINE, 29, 68, 318, colors["ink"])
        content = (
            f'<rect width="1280" height="480" fill="{colors["background"]}"/>'
            f'<image x="570" y="0" width="720" height="480" href="data:image/webp;base64,{encoded}"/>'
            f'<g transform="translate(60 40) scale(1.1)">{mark(colors["ink"], colors["blue"])}</g>'
            + word + descriptor + tagline
        )
        (output / f"readme-banner-{theme}.svg").write_text(
            svg(1280, 480, content, f"azbridge - {TAGLINE}"), encoding="utf-8",
        )
    image = Image.new("RGB", (1280, 640), CHALK)
    hero = Image.open(output / "hero-light.webp").convert("RGB").resize((768, 512), Image.Resampling.LANCZOS)
    image.paste(hero, (500, 64))
    image.paste(icon(96).convert("RGB"), (54, 54))
    draw = ImageDraw.Draw(image)
    draw.text((64, 188), "azbridge", font=pil_font(font_path, 100), fill=INK)
    draw.text((68, 320), "Relay control plane", font=pil_font(font_path, 28), fill=INK)
    draw.text((68, 396), "Outbound paths.", font=pil_font(font_path, 34), fill=INK)
    draw.text((68, 444), "Clear control.", font=pil_font(font_path, 34), fill=INK)
    image.save(output / "social-preview.png", optimize=True)


def build(source: Path, concepts: Path, destination: Path) -> None:
    font_path = concepts / "fonts" / "spacegrotesk.ttf"
    font_sources = [
        entry for entry in json.loads((font_path.parent / "sources.json").read_text(encoding="utf-8"))
        if entry["asset"] in ("spacegrotesk.ttf", "spacegrotesk-OFL.txt")
    ]
    if len(font_sources) != 2:
        raise ValueError("Missing pinned Space Grotesk font/license provenance")
    for entry in font_sources:
        if digest(font_path.parent / entry["asset"]) != entry["sha256"].lower():
            raise ValueError(f"Font source checksum mismatch: {entry['asset']}")
    concept_records = json.loads((concepts / "provenance.json").read_text(encoding="utf-8"))
    concept_name = "offset-span--MAI-Image-2.6--logo.png"
    logo_source = next((entry for entry in concept_records if entry["asset"] == concept_name), None)
    strategy_source = next((entry for entry in concept_records if entry["asset"] == "concepts.json"), None)
    if (not logo_source or not strategy_source
            or digest(concepts / concept_name) != logo_source["sha256"]
            or digest(concepts / "concepts.json") != strategy_source["sha256"]):
        raise ValueError("Missing or changed source concept provenance")
    output = destination / "assets"
    output.mkdir(parents=True, exist_ok=True)
    font = instantiateVariableFont(TTFont(font_path), {"wght": 600})
    make_vectors(font, output)
    make_icons(output)
    entries = artwork(source, output)
    make_banners(output, font, font_path)
    shutil.copyfile(font_path.parent / "spacegrotesk-OFL.txt", destination / "SpaceGrotesk-OFL.txt")
    inventory = [
        {"asset": f"assets/{path.name}", "sha256": digest(path), "bytes": path.stat().st_size}
        for path in sorted(output.iterdir()) if path.is_file()
    ]
    write_json(destination / "provenance.json", {
        "brand": "azbridge", "direction": "Offset Span", "tagline": TAGLINE,
        "descriptor": "Relay control plane", "wordmark": {"family": "Space Grotesk", "weight": 600},
        "createdAt": datetime.now(timezone.utc).isoformat(),
        "logo": {
            "method": "Hand-drawn original SVG bracket geometry; font glyphs converted to paths with FontTools",
            "sourceGeneration": logo_source,
        },
        "strategyGeneration": strategy_source,
        "fonts": font_sources, "artwork": entries, "inventory": inventory,
        "notice": "Generated artwork is not a Microsoft endorsement. No trademark clearance claimed. "
                  "Raster conversion may strip embedded generation metadata; retain this provenance.",
    })
    print(f"Built {len(inventory)} review-stage assets in {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--concepts", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    build(args.source, args.concepts, args.output)
