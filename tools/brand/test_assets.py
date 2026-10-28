import hashlib
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from PIL import Image, ImageFont

from build_assets import (
    BRACKETS, THEMES, artwork, build, digest, icon, make_banners, make_icons,
    make_vectors, mark, outline, pil_font, svg,
)
from build_final_board import CLI_BANNER, build as build_board


def test_font():
    builder = FontBuilder(1000, isTTF=True)
    characters = sorted(set("azbridgeRelay controlpnOubhs.Cf"))
    names = {character: "space" if character == " " else f"glyph{ord(character)}" for character in characters}
    builder.setupGlyphOrder([".notdef", *names.values()])
    builder.setupCharacterMap({ord(character): name for character, name in names.items()})
    glyphs = {}
    for name in [".notdef", *names.values()]:
        pen = TTGlyphPen(None)
        if name != "space":
            pen.moveTo((50, 0))
            pen.lineTo((450, 0))
            pen.lineTo((450, 700))
            pen.lineTo((50, 700))
            pen.closePath()
        glyphs[name] = pen.glyph()
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics({name: (500, 50) for name in glyphs})
    builder.setupHorizontalHeader(ascent=800, descent=-200)
    builder.setupNameTable({"familyName": "Test", "styleName": "Regular"})
    builder.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
    builder.setupPost()
    builder.setupMaxp()
    return builder.font


def source_artwork(path):
    entries = []
    for name in ("hero", "connections", "profiles", "gateways"):
        for theme in THEMES:
            filename = f"{name}-{theme}.png"
            Image.new("RGB", (1536, 1024) if name == "hero" else (1024, 1024), "#F3F5F7").save(path / filename)
            entries.append({"asset": filename, "sha256": digest(path / filename)})
    (path / "provenance.json").write_text(json.dumps(entries))


class AssetTests(unittest.TestCase):
    def test_mark_has_exact_two_square_brackets_and_safe_svg_title(self):
        self.assertEqual(len(BRACKETS), 2)
        result = ET.fromstring(svg(64, 64, mark("#182B3A", "#0078D4"), "azbridge & relay"))
        self.assertEqual(result.attrib["viewBox"], "0 0 64 64")
        self.assertEqual(len(result.findall("{http://www.w3.org/2000/svg}path")), 2)
        self.assertEqual(result.attrib["aria-label"], "azbridge & relay")

    def test_outline_has_paths_without_font_dependency_and_rejects_missing_glyph(self):
        paths, width = outline(test_font(), "azbridge", 48, 8, 46, "#182B3A")
        self.assertEqual(paths.count("<path"), 8)
        self.assertGreater(width, 100)
        with self.assertRaisesRegex(ValueError, "no glyph"):
            outline(test_font(), "\u2603", 48, 0, 0, "#182B3A")

    def test_all_vector_variants_are_valid_and_wordmarks_are_outlined(self):
        with TemporaryDirectory() as folder:
            path = Path(folder)
            make_vectors(test_font(), path)
            self.assertEqual(len(list(path.glob("*.svg"))), 13)
            for file in path.iterdir():
                root = ET.fromstring(file.read_text())
                self.assertIsNone(root.find(".//{http://www.w3.org/2000/svg}text"))
                self.assertNotIn("font-family", file.read_text())
            self.assertIn("prefers-color-scheme:dark", (path / "favicon.svg").read_text())

    def test_icons_have_expected_sizes_maskable_safe_zone_and_ico_frames(self):
        self.assertEqual(icon(180).size, (180, 180))
        masked = icon(512, maskable=True)
        # All foreground geometry fits within the central mask-safe circle.
        background = masked.getpixel((0, 0))
        for x, y in [(90, 90), (422, 90), (90, 422), (422, 422)]:
            self.assertEqual(masked.getpixel((x, y)), background)
        with TemporaryDirectory() as folder:
            path = Path(folder)
            make_icons(path)
            with Image.open(path / "favicon.ico") as image:
                self.assertEqual(image.ico.sizes(), {(16, 16), (32, 32), (48, 48)})
            with Image.open(path / "icon-maskable-512.png") as image:
                self.assertEqual(image.size, (512, 512))
            manifest = json.loads((path / "site.webmanifest").read_text())
            self.assertEqual(manifest["icons"][-1]["purpose"], "maskable")
            self.assertEqual(manifest["start_url"], "/")

    def test_artwork_converts_all_eight_with_budgets_and_source_hashes(self):
        with TemporaryDirectory() as folder:
            path = Path(folder)
            output = path / "out"
            output.mkdir()
            source_artwork(path)
            entries = artwork(path, output)
            self.assertEqual(len(entries), 8)
            for entry in entries:
                file = output / Path(entry["asset"]).name
                self.assertEqual(entry["sha256"], digest(file))
                self.assertIn("sourceSha256", entry)
                self.assertLess(file.stat().st_size, 150_000 if file.name.startswith("hero") else 40_000)
            with Image.open(output / "hero-light.webp") as image:
                self.assertEqual(image.size, (1200, 800))
            with Image.open(output / "gateways-dark.webp") as image:
                self.assertEqual(image.size, (480, 480))

    def test_artwork_missing_entries_or_changed_source_returns_err(self):
        with TemporaryDirectory() as folder:
            path = Path(folder)
            (path / "provenance.json").write_text("[]")
            with self.assertRaisesRegex(ValueError, "exactly eight"):
                artwork(path, path)
            source_artwork(path)
            (path / "hero-light.png").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "checksum"):
                artwork(path, path)

    def test_over_budget_artwork_returns_err(self):
        with TemporaryDirectory() as folder:
            path = Path(folder)
            source_artwork(path)
            with patch("build_assets.Image.Image.save", side_effect=lambda target, *args, **kwargs: target.write(b"x" * 150_001)):
                with self.assertRaisesRegex(ValueError, "byte budget"):
                    artwork(path, path)

    def test_pil_font_selects_weight_600(self):
        with patch("build_assets.ImageFont.truetype") as loader:
            self.assertEqual(pil_font(Path("font.ttf"), 48), loader.return_value)
            loader.return_value.set_variation_by_axes.assert_called_once_with([600])

    def test_banners_are_outlined_and_social_preview_has_required_dimensions(self):
        with TemporaryDirectory() as folder:
            path = Path(folder)
            for theme in THEMES:
                Image.new("RGB", (1200, 800), THEMES[theme]["background"]).save(path / f"hero-{theme}.webp")
            with patch("build_assets.pil_font", return_value=ImageFont.load_default()):
                make_banners(path, test_font(), Path("unused.ttf"))
            with Image.open(path / "social-preview.png") as image:
                self.assertEqual(image.size, (1280, 640))
            for theme in THEMES:
                text = (path / f"readme-banner-{theme}.svg").read_text()
                ET.fromstring(text)
                self.assertIn("data:image/webp;base64,", text)
                self.assertNotIn("<text", text)

    def test_build_checks_font_sources_and_records_inventory(self):
        with TemporaryDirectory() as folder:
            path = Path(folder)
            concepts = path / "concepts"
            fonts = concepts / "fonts"
            fonts.mkdir(parents=True)
            font = test_font()
            font.save(fonts / "spacegrotesk.ttf")
            (fonts / "spacegrotesk-OFL.txt").write_text("SIL OPEN FONT LICENSE")
            sources = [{"asset": name, "sha256": digest(fonts / name)} for name in ("spacegrotesk.ttf", "spacegrotesk-OFL.txt")]
            (fonts / "sources.json").write_text(json.dumps(sources))
            source_artwork(path)
            concept_name = "offset-span--MAI-Image-2.6--logo.png"
            (concepts / concept_name).write_bytes(b"original concept")
            (concepts / "concepts.json").write_text("{}")
            (concepts / "provenance.json").write_text(json.dumps([
                {"asset": concept_name, "sha256": digest(concepts / concept_name)},
                {"asset": "concepts.json", "deployment": "text-model", "sha256": digest(concepts / "concepts.json")},
            ]))
            output = path / "brand"
            with patch("build_assets.instantiateVariableFont", side_effect=lambda value, _: value), patch(
                "build_assets.pil_font", return_value=ImageFont.load_default(),
            ):
                build(path, concepts, output)
            provenance = json.loads((output / "provenance.json").read_text())
            self.assertEqual(len(provenance["inventory"]), 30)
            for entry in provenance["inventory"]:
                self.assertEqual(entry["sha256"], hashlib.sha256((output / entry["asset"]).read_bytes()).hexdigest())
            board = path / "review.html"
            build_board(output, board)
            self.assertIn("data:image/svg+xml;base64,", board.read_text())
            self.assertNotIn("FINAL_REVIEW_DATA", board.read_text())
            self.assertTrue(CLI_BANNER.isascii())
            self.assertLessEqual(max(map(len, CLI_BANNER.splitlines())), 72)
            (output / "assets" / "mark-light.svg").write_text("changed")
            with self.assertRaisesRegex(ValueError, "Final asset checksum"):
                build_board(output, board)
            (output / "provenance.json").write_text('{"inventory":[]}')
            with self.assertRaisesRegex(ValueError, "Expected 29"):
                build_board(output, board)
            records = (concepts / "provenance.json").read_text()
            (concepts / "provenance.json").write_text("[]")
            with self.assertRaisesRegex(ValueError, "source concept provenance"):
                build(path, concepts, output)
            (concepts / "provenance.json").write_text(records)
            (fonts / "spacegrotesk.ttf").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "Font source checksum"):
                build(path, concepts, output)
            (fonts / "sources.json").write_text("[]")
            with self.assertRaisesRegex(ValueError, "Missing pinned"):
                build(path, concepts, output)


if __name__ == "__main__":
    unittest.main()
