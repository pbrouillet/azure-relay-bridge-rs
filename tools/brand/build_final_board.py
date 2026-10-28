"""Embed every verified final asset in a self-contained Gate 2 review board."""

import argparse
import base64
import json
from pathlib import Path

from build_assets import digest


CLI_BANNER = (
    "  +------           azbridge\n"
    "  |                 Relay control plane\n"
    "             |\n"
    "       ------+      Outbound paths. Clear control.\n"
)
MIME = {".svg": "image/svg+xml", ".png": "image/png", ".webp": "image/webp", ".ico": "image/x-icon"}


def build(folder: Path, output: Path) -> None:
    provenance = json.loads((folder / "provenance.json").read_text(encoding="utf-8"))
    assets = {}
    for entry in provenance["inventory"]:
        path = folder / entry["asset"]
        if digest(path) != entry["sha256"]:
            raise ValueError(f"Final asset checksum mismatch: {path.name}")
        if path.suffix not in MIME:
            continue
        assets[path.name] = {
            "uri": f"data:{MIME[path.suffix]};base64," + base64.b64encode(path.read_bytes()).decode("ascii"),
            "inline": path.read_text(encoding="utf-8") if path.suffix == ".svg" else None,
            "bytes": path.stat().st_size,
        }
    if len(assets) != 29:
        raise ValueError(f"Expected 29 image assets plus manifest, found {len(assets)} images")
    data = json.dumps({
        "assets": assets, "provenance": provenance, "cliBanner": CLI_BANNER,
        "manifest": json.loads((folder / "assets" / "site.webmanifest").read_text(encoding="utf-8")),
    }, ensure_ascii=True).replace("<", "\\u003c")
    template = Path(__file__).with_name("final-board.html").read_text(encoding="utf-8")
    concept_template = Path(__file__).with_name("board.html").read_text(encoding="utf-8")
    theme = concept_template.split("<style>", 1)[1].split("</style>", 1)[0]
    html = template.replace("/* REVIEW_THEME */", theme).replace("FINAL_REVIEW_DATA", data)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(html, encoding="utf-8")
    print(f"Final review board: {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    build(args.input, args.output)
