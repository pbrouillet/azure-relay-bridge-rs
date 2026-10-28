"""Generate review concepts through existing Foundry deployments, using Entra auth."""

from __future__ import annotations

import argparse
import base64
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import re
import sys
import time
from urllib.parse import urlsplit

from azure.identity import AzureCliCredential
from PIL import Image
import requests
import yaml


CONFIG = Path(__file__).with_name("prompts.yaml")
SCOPE = "https://cognitiveservices.azure.com/.default"


def endpoint(value: str) -> str:
    parsed = urlsplit(value)
    if (
        parsed.scheme != "https"
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.port not in (None, 443)
        or parsed.path not in ("", "/")
        or parsed.query
        or parsed.fragment
        or not parsed.hostname.endswith(".services.ai.azure.com")
    ):
        raise ValueError("AZBRIDGE_BRAND_ENDPOINT must be an HTTPS Foundry services resource origin")
    return value.rstrip("/")


def validate_concepts(value: dict) -> dict:
    directions = value.get("directions", [])
    if len(directions) != 3 or len(value.get("taglines", [])) != 10:
        raise ValueError("The concept brief must contain three directions and ten taglines")
    ids = set()
    for direction in directions:
        identifier = direction.get("id", "")
        if not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", identifier) or identifier in ids:
            raise ValueError("Direction IDs must be distinct safe kebab-case names")
        ids.add(identifier)
        for key in ("name", "rationale", "motif", "logo_prompt", "hero_prompt"):
            if not isinstance(direction.get(key), str) or not direction[key].strip():
                raise ValueError(f"Direction is missing {key}")
        colors = direction.get("palette", [])
        if len(colors) != 4 or any(
            not isinstance(color.get("name"), str)
            or not re.fullmatch(r"#[0-9A-Fa-f]{6}", color.get("hex", ""))
            for color in colors
        ):
            raise ValueError("Each direction needs four named hex palette colors")
    for tagline in value["taglines"]:
        if not all(isinstance(tagline.get(key), str) and tagline[key].strip()
                   for key in ("text", "rationale")):
            raise ValueError("Taglines need text and rationale")
    if (not isinstance(value.get("voice"), list) or len(value["voice"]) != 3
            or any(not isinstance(line, str) or not line.strip() for line in value["voice"])):
        raise ValueError("The brief needs three voice principles")
    return value


def image_request(model: dict, prompt: str, kind: str) -> tuple[str, dict]:
    width, height = (1024, 1024) if kind == "logo" else (1536, 1024)
    body = {"model": model["deployment"], "prompt": prompt}
    if model["provider"] == "openai":
        return "/openai/v1/images/generations?api-version=preview", {
            **body, "size": f"{width}x{height}", "n": 1,
            "quality": "medium", "output_format": "png",
        }
    if model["provider"] == "mai":
        return "/mai/v1/images/generations", {**body, "width": width, "height": height}
    if model["provider"] == "flux":
        return "/providers/blackforestlabs/v1/flux-2-pro?api-version=preview", {
            **body, "width": width, "height": height,
            "output_format": "png", "num_images": 1,
        }
    raise ValueError(f"Unsupported image provider: {model['provider']}")


def image_bytes(result: dict) -> bytes:
    data = result.get("data")
    if isinstance(data, list) and len(data) == 1 and isinstance(data[0], dict):
        encoded = data[0].get("b64_json")
        if isinstance(encoded, str):
            return base64.b64decode(encoded, validate=True)
    # The BFL provider's synchronous API uses a base64 image field.
    encoded = result.get("image")
    if isinstance(encoded, str) and not encoded.startswith(("https:", "http:")):
        return base64.b64decode(encoded, validate=True)
    raise ValueError(f"No supported base64 image in provider response; fields: {sorted(result)}")


def write_json(path: Path, value: object) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    temporary.replace(path)


def record(path: Path, entry: dict) -> None:
    entries = json.loads(path.read_text(encoding="utf-8")) if path.exists() else []
    if any(item["asset"] == entry["asset"] for item in entries):
        raise ValueError(f"Provenance already contains {entry['asset']}; refusing to overwrite")
    write_json(path, [*entries, entry])


class Foundry:
    def __init__(self, origin: str):
        self.origin = endpoint(origin)
        self.credential = AzureCliCredential(process_timeout=60)
        self.session = requests.Session()

    def post(self, path: str, body: dict) -> dict:
        for attempt in range(5):
            token = self.credential.get_token(SCOPE).token
            try:
                response = self.session.post(
                    self.origin + path,
                    headers={"Authorization": f"Bearer {token}"},
                    json=body,
                    timeout=(30, 600),
                    allow_redirects=False,
                )
            except requests.RequestException:
                raise RuntimeError("Foundry request failed or timed out; not retried to avoid duplicate charges") from None
            if response.status_code == 429 and attempt < 4:
                delay = response.headers.get("retry-after", "")
                seconds = min(float(delay), 300) if delay.isdigit() else min(20 * 2**attempt, 300)
                print(f"Rate limited; waiting {seconds:g}s before retry {attempt + 1}/4", flush=True)
                time.sleep(seconds)
                continue
            if response.status_code != 200:
                try:
                    error = response.json().get("error", {})
                    code = error.get("code", "unknown") if isinstance(error, dict) else "unknown"
                except ValueError:
                    code = "non-JSON error"
                raise RuntimeError(f"Foundry HTTP {response.status_code}, code={code}; request not retried")
            return response.json()
        raise RuntimeError("Foundry rate-limit retry budget exhausted")


def concepts(client: Foundry, config: dict, output: Path) -> dict:
    path = output / "concepts.json"
    if path.exists():
        print("Resuming existing concept brief (no text-generation charge).", flush=True)
        return validate_concepts(json.loads(path.read_text(encoding="utf-8")))
    prompt = config["brief"]
    body = {
        "model": config["text_deployment"],
        "messages": [{"role": "user", "content": prompt}],
        "response_format": {"type": "json_object"},
        "max_completion_tokens": 12000,
        "reasoning_effort": "low",
    }
    print(f"Drafting three directions with {config['text_deployment']}...", flush=True)
    result = client.post("/openai/v1/chat/completions", body)
    choice = result["choices"][0]
    if choice.get("finish_reason") != "stop" or choice["message"].get("refusal"):
        raise RuntimeError("Concept generation was refused or incomplete; no substitute brief created")
    value = validate_concepts(json.loads(choice["message"]["content"]))
    write_json(path, value)
    record(output / "provenance.json", {
        "asset": path.name, "deployment": config["text_deployment"],
        "model": result.get("model"), "createdAt": datetime.now(timezone.utc).isoformat(),
        "prompt": prompt, "parameters": {key: val for key, val in body.items() if key != "messages"},
        "usage": result.get("usage"), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "postProcessing": "JSON validation and indentation only",
    })
    return value


def generate_images(client: Foundry, config: dict, brief: dict, output: Path, selected: set[str]) -> None:
    provenance = output / "provenance.json"
    for direction in brief["directions"]:
        for model in config["models"]:
            if selected and model["deployment"] not in selected:
                continue
            for kind in ("logo", "hero"):
                filename = f"{direction['id']}--{model['deployment']}--{kind}.png"
                path = output / filename
                entries = json.loads(provenance.read_text(encoding="utf-8"))
                existing = next((entry for entry in entries if entry["asset"] == filename), None)
                if existing:
                    if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != existing["sha256"]:
                        raise ValueError(f"Recorded output is missing or changed: {filename}")
                    print(f"Keeping verified {filename}", flush=True)
                    continue
                if path.exists():
                    raise ValueError(f"Unrecorded output exists: {filename}; refusing to overwrite")
                prompt = "\n\n".join([
                    direction[f"{kind}_prompt"], f"Shared direction: {direction['motif']}.",
                    "Palette: " + ", ".join(f"{color['name']} {color['hex']}" for color in direction["palette"]),
                    config["image_constraints"],
                ])
                route, body = image_request(model, prompt, kind)
                print(f"Generating {filename}...", flush=True)
                result = client.post(route, body)
                data = image_bytes(result)
                image = Image.open(io.BytesIO(data))
                image.load()
                if image.format != "PNG":
                    raise ValueError(f"Expected PNG, received {image.format}; no output saved")
                path.write_bytes(data)
                preview = output / path.with_suffix(".webp").name
                image.save(preview, "WEBP", quality=88, method=6)
                record(provenance, {
                    "asset": filename, "preview": preview.name, "deployment": model["deployment"],
                    "modelVersion": model["version"], "createdAt": datetime.now(timezone.utc).isoformat(),
                    "prompt": prompt, "parameters": {key: val for key, val in body.items() if key != "prompt"},
                    "sha256": hashlib.sha256(data).hexdigest(), "dimensions": list(image.size),
                    "postProcessing": "Original PNG retained; WebP review copy quality=88 method=6",
                    "revisedPrompt": (result.get("data") or [{}])[0].get("revised_prompt"),
                })
                print(f"Saved {filename}: {image.width}x{image.height}", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="Local review directory (raw artwork stays out of the app)")
    parser.add_argument("--text-only", action="store_true")
    parser.add_argument("--model", action="append", default=[], help="Limit this run to a configured deployment (repeatable)")
    args = parser.parse_args()
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    unknown = set(args.model) - {model["deployment"] for model in config["models"]}
    if unknown:
        raise ValueError(f"Unknown deployments: {sorted(unknown)}")
    args.output.mkdir(parents=True, exist_ok=True)
    origin = os.environ.get("AZBRIDGE_BRAND_ENDPOINT", "")
    client = Foundry(origin)
    brief = concepts(client, config, args.output)
    if not args.text_only:
        generate_images(client, config, brief, args.output, set(args.model))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, RuntimeError, KeyError) as error:
        print(f"Brand generation stopped: {error}", file=sys.stderr)
        sys.exit(1)
