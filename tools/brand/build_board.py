"""Embed verified generated concepts and local OFL font specimens in one HTML board."""

import argparse
import base64
import hashlib
import json
from pathlib import Path

from generate import validate_concepts


FONTS = [
    ("Space Grotesk", "spacegrotesk", "Geometric and distinctly engineered; an open, compact wordmark."),
    ("IBM Plex Sans", "ibmplexsans", "Practical and humanist; strong legibility with restrained personality."),
    ("Sora", "sora", "Broad and approachable; a softer counterpoint to square interface controls."),
]


def build(folder: Path) -> Path:
    brief = validate_concepts(json.loads((folder / "concepts.json").read_text(encoding="utf-8")))
    provenance = json.loads((folder / "provenance.json").read_text(encoding="utf-8"))
    models = []
    images = {}
    for entry in provenance:
        path = folder / entry["asset"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]:
            raise ValueError(f"Source artifact checksum mismatch: {path.name}")
        if not entry["asset"].endswith(".png"):
            continue
        if entry["deployment"] not in models:
            models.append(entry["deployment"])
        direction = next(item for item in brief["directions"] if entry["asset"].startswith(item["id"] + "--"))
        entry["directionName"] = direction["name"]
        preview = (folder / entry["preview"]).read_bytes()
        images[entry["preview"]] = "data:image/webp;base64," + base64.b64encode(preview).decode("ascii")
    if len(images) != 18 or len(models) != 3:
        raise ValueError(f"Concept review requires 18 images / three models; found {len(images)} / {len(models)}")
    fonts = []
    styles = []
    for name, identifier, note in FONTS:
        path = folder / "fonts" / f"{identifier}.ttf"
        license_text = (path.parent / f"{identifier}-OFL.txt").read_text(encoding="utf-8")
        if "SIL OPEN FONT LICENSE" not in license_text:
            raise ValueError(f"Missing OFL license for {name}")
        encoded = base64.b64encode(path.read_bytes()).decode("ascii")
        styles.append(f"@font-face{{font-family:'{name}';font-style:normal;font-weight:100 900;src:url(data:font/ttf;base64,{encoded}) format('truetype');font-display:swap;}}")
        fonts.append({"name": name, "note": note, "license": license_text})
    data = json.dumps({
        "brief": brief, "provenance": provenance, "models": models,
        "images": images, "fonts": fonts,
    }, ensure_ascii=True).replace("<", "\\u003c")
    template = Path(__file__).with_name("board.html").read_text(encoding="utf-8")
    html = template.replace("/* EMBEDDED_FONTS */", "\n".join(styles)).replace("EMBEDDED_REVIEW_DATA", data)
    output = folder / "azbridge-brand-review.html"
    output.write_text(html, encoding="utf-8")
    print(f"Review board: {output}")
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    build(parser.parse_args().input)
