import base64
import hashlib
import io
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import Mock

from PIL import Image

from generate_final import generate


class FinalArtworkTests(unittest.TestCase):
    def setUp(self):
        self.config = {
            "direction": "Offset Span", "deployment": "MAI-Image-2.6", "provider": "mai",
            "version": "2026-07-31", "shared": "Public product brief",
            "themes": {"light": "Chalk background"},
            "assets": {"hero": {"width": 16, "height": 16, "prompt": "Open passage"}},
        }
        raw = io.BytesIO()
        Image.new("RGB", (16, 16)).save(raw, "PNG")
        self.raw = raw.getvalue()
        self.client = Mock()
        self.client.post.return_value = {
            "data": [{"b64_json": base64.b64encode(self.raw).decode("ascii")}],
        }

    def test_generation_records_recipe_and_verified_resume_makes_no_calls(self):
        with TemporaryDirectory() as folder:
            path = Path(folder)
            generate(self.client, self.config, path)
            entries = json.loads((path / "provenance.json").read_text())
            self.assertEqual(entries[0]["sha256"], hashlib.sha256(self.raw).hexdigest())
            self.assertEqual(entries[0]["modelVersion"], "2026-07-31")
            self.assertEqual(entries[0]["parameters"]["width"], 16)
            generate(self.client, self.config, path)
            self.client.post.assert_called_once()

    def test_changed_recipe_or_file_returns_err(self):
        with TemporaryDirectory() as folder:
            path = Path(folder)
            generate(self.client, self.config, path)
            self.config["shared"] = "Different brief"
            with self.assertRaisesRegex(ValueError, "recipe changed"):
                generate(self.client, self.config, path)
            self.config["shared"] = "Public product brief"
            (path / "hero-light.png").unlink()
            with self.assertRaisesRegex(ValueError, "recipe changed"):
                generate(self.client, self.config, path)

    def test_collision_returns_err(self):
        with TemporaryDirectory() as folder:
            path = Path(folder)
            (path / "hero-light.png").write_bytes(self.raw)
            with self.assertRaisesRegex(ValueError, "Unrecorded"):
                generate(self.client, self.config, path)
            self.client.post.assert_not_called()

    def test_wrong_dimensions_returns_err_without_saving(self):
        self.config["assets"]["hero"]["width"] = 32
        with TemporaryDirectory() as folder:
            path = Path(folder)
            with self.assertRaisesRegex(ValueError, "dimensions"):
                generate(self.client, self.config, path)
            self.assertEqual(list(path.iterdir()), [])

    def test_provider_failure_does_not_create_success_output(self):
        self.client.post.side_effect = RuntimeError("Foundry HTTP 403")
        with TemporaryDirectory() as folder:
            with self.assertRaisesRegex(RuntimeError, "403"):
                generate(self.client, self.config, Path(folder))
            self.assertEqual(list(Path(folder).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
