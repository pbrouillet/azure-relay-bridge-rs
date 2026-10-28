"""Generate the approved Offset Span artwork without modifying the application."""

import argparse
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path

from PIL import Image
import yaml

from generate import Foundry, image_bytes, image_request, record


CONFIG = Path(__file__).with_name("final-prompts.yaml")


def generate(client: Foundry, config: dict, output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    provenance = output / "provenance.json"
    entries = json.loads(provenance.read_text(encoding="utf-8")) if provenance.exists() else []
    for name, asset in config["assets"].items():
        for theme, theme_prompt in config["themes"].items():
            filename = f"{name}-{theme}.png"
            path = output / filename
            prompt = "\n\n".join([config["shared"], asset["prompt"], theme_prompt])
            route, body = image_request(config, prompt, "hero" if name == "hero" else "logo")
            body.update(width=asset["width"], height=asset["height"])
            previous = next((entry for entry in entries if entry["asset"] == filename), None)
            if previous:
                if (not path.is_file()
                        or hashlib.sha256(path.read_bytes()).hexdigest() != previous["sha256"]
                        or previous["prompt"] != prompt or previous["parameters"] != body):
                    raise ValueError(f"Recorded artwork or generation recipe changed: {filename}")
                print(f"Keeping verified {filename}", flush=True)
                continue
            if path.exists():
                raise ValueError(f"Unrecorded artwork exists: {filename}; refusing to overwrite")
            print(f"Generating final {filename} with {config['deployment']}...", flush=True)
            result = client.post(route, body)
            raw = image_bytes(result)
            image = Image.open(io.BytesIO(raw))
            image.load()
            if image.format != "PNG" or image.size != (asset["width"], asset["height"]):
                raise ValueError(f"Unexpected image format/dimensions for {filename}; no file saved")
            path.write_bytes(raw)
            entry = {
                "asset": filename, "deployment": config["deployment"],
                "modelVersion": config["version"], "direction": config["direction"],
                "createdAt": datetime.now(timezone.utc).isoformat(), "prompt": prompt,
                "parameters": body, "sha256": hashlib.sha256(raw).hexdigest(),
                "dimensions": list(image.size), "postProcessing": "Original PNG retained unchanged",
            }
            record(provenance, entry)
            entries.append(entry)
            print(f"Saved {filename}: {image.width}x{image.height}", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    configuration = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    generate(Foundry(os.environ.get("AZBRIDGE_BRAND_ENDPOINT", "")), configuration, args.output)
