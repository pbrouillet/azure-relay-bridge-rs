import base64
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import generate
from PIL import Image


def brief():
    return {
        "directions": [{
            "id": f"direction-{index}", "name": "Name", "rationale": "Rationale",
            "motif": "Motif", "logo_prompt": "Logo", "hero_prompt": "Hero",
            "palette": [{"name": "Blue", "hex": "#0078D4"} for _ in range(4)],
        } for index in range(3)],
        "taglines": [{"text": "A tagline", "rationale": "Rationale"} for _ in range(10)],
        "voice": ["Precise", "Approachable", "Operational"],
    }


class GenerationTests(unittest.TestCase):
    def test_concept_validation_accepts_complete_brief_and_rejects_invalid_content(self):
        value = brief()
        self.assertEqual(generate.validate_concepts(value), value)
        mutations = [
            lambda item: item.update(directions=[]),
            lambda item: item.update(taglines=[]),
            lambda item: item["directions"][0].update(id="../unsafe"),
            lambda item: item["directions"][0].update(id="direction-1"),
            lambda item: item["directions"][0].update(name=""),
            lambda item: item["directions"][0]["palette"][0].update(hex="red"),
            lambda item: item["taglines"][0].update(text=""),
            lambda item: item.update(voice=[]),
            lambda item: item.update(voice=["one", "two", None]),
        ]
        for change in mutations:
            value = brief()
            change(value)
            with self.subTest(value=value), self.assertRaises(ValueError):
                generate.validate_concepts(value)

    def test_endpoint_rejects_credentials_and_non_resource_origins(self):
        self.assertEqual(generate.endpoint("https://sample.services.ai.azure.com/"),
                         "https://sample.services.ai.azure.com")
        for value in ("", "http://sample.services.ai.azure.com", "https://example.com",
                      "https://user:pass@sample.services.ai.azure.com",
                      "https://sample.services.ai.azure.com/path",
                      "https://sample.services.ai.azure.com?token=secret"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                generate.endpoint(value)

    def test_image_routes_and_dimensions(self):
        for provider in ("openai", "mai", "flux"):
            path, body = generate.image_request({"provider": provider, "deployment": "model"}, "prompt", "hero")
            self.assertTrue(path.startswith("/"))
            self.assertEqual(body["model"], "model")
            self.assertEqual(body.get("size", f"{body.get('width')}x{body.get('height')}"), "1536x1024")
        with self.assertRaises(ValueError):
            generate.image_request({"provider": "unsupported", "deployment": "model"}, "prompt", "logo")

    def test_image_extraction_never_fakes_missing_or_filtered_images(self):
        data = base64.b64encode(b"image").decode()
        self.assertEqual(generate.image_bytes({"data": [{"b64_json": data}]}), b"image")
        self.assertEqual(generate.image_bytes({"image": data}), b"image")
        for result in ({}, {"error": "filtered"}, {"data": []}, {"image": "https://external.invalid/image"}):
            with self.subTest(result=result), self.assertRaises(ValueError):
                generate.image_bytes(result)

    def test_provenance_refuses_duplicate_assets(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "provenance.json"
            generate.record(path, {"asset": "one.png"})
            self.assertEqual(json.loads(path.read_text()), [{"asset": "one.png"}])
            with self.assertRaises(ValueError):
                generate.record(path, {"asset": "one.png"})

    @patch("generate.AzureCliCredential")
    @patch("generate.requests.Session")
    @patch("generate.time.sleep")
    def test_rate_limit_retries_are_bounded_and_use_fresh_tokens(self, sleep, session, credential):
        limited = Mock(status_code=429, headers={"retry-after": "1"}, ok=False)
        success = Mock(status_code=200, ok=True)
        success.json.return_value = {"result": "ok"}
        session.return_value.post.side_effect = [limited, success]
        client = generate.Foundry("https://sample.services.ai.azure.com")
        self.assertEqual(client.post("/test", {}), {"result": "ok"})
        self.assertEqual(credential.return_value.get_token.call_count, 2)
        sleep.assert_called_once_with(1)
        session.return_value.post.side_effect = None
        session.return_value.post.return_value = limited
        limited.json.return_value = {"error": {"code": "RateLimitExceeded"}}
        with self.assertRaisesRegex(RuntimeError, "429"):
            client.post("/test", {})

    @patch("generate.AzureCliCredential")
    @patch("generate.requests.Session")
    def test_auth_and_filter_errors_do_not_retry_or_expose_headers(self, session, credential):
        response = Mock(status_code=403, ok=False)
        response.json.return_value = {"error": {"code": "content_filter"}}
        session.return_value.post.return_value = response
        client = generate.Foundry("https://sample.services.ai.azure.com")
        with self.assertRaisesRegex(RuntimeError, "content_filter"):
            client.post("/test", {})
        session.return_value.post.assert_called_once()

    @patch("generate.AzureCliCredential")
    @patch("generate.requests.Session")
    def test_timeout_does_not_retry_a_potentially_billable_request(self, session, credential):
        session.return_value.post.side_effect = generate.requests.Timeout("sensitive URL")
        with self.assertRaisesRegex(RuntimeError, "not retried") as caught:
            generate.Foundry("https://sample.services.ai.azure.com").post("/test", {})
        self.assertNotIn("sensitive", str(caught.exception))
        session.return_value.post.assert_called_once()

    def test_concepts_are_recorded_and_resumed_without_another_model_call(self):
        with tempfile.TemporaryDirectory() as folder:
            config = {"brief": "Public brief", "text_deployment": "text-model"}
            client = Mock()
            client.post.return_value = {
                "choices": [{"finish_reason": "stop", "message": {"content": json.dumps(brief())}}],
                "model": "model-version", "usage": {},
            }
            output = Path(folder)
            self.assertEqual(generate.concepts(client, config, output), brief())
            self.assertEqual(generate.concepts(client, config, output), brief())
            client.post.assert_called_once()
            record = json.loads((output / "provenance.json").read_text())[0]
            self.assertEqual(record["model"], "model-version")
            self.assertEqual(record["prompt"], "Public brief")

    def test_refused_or_truncated_concepts_leave_no_success_output(self):
        for finish, refusal in (("length", None), ("stop", "refused")):
            with tempfile.TemporaryDirectory() as folder:
                client = Mock()
                client.post.return_value = {
                    "choices": [{"finish_reason": finish, "message": {"refusal": refusal}}],
                }
                with self.assertRaisesRegex(RuntimeError, "refused or incomplete"):
                    generate.concepts(client, {"brief": "brief", "text_deployment": "text-model"}, Path(folder))
                self.assertEqual(list(Path(folder).iterdir()), [])

    def test_images_record_originals_and_resume_without_regenerating(self):
        buffer = io.BytesIO()
        Image.new("RGB", (16, 16), "white").save(buffer, "PNG")
        client = Mock()
        client.post.return_value = {"data": [{"b64_json": base64.b64encode(buffer.getvalue()).decode()}]}
        model = {"provider": "mai", "deployment": "test-image", "version": "test-version"}
        config = {"models": [model], "image_constraints": "No text"}
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder)
            generate.write_json(output / "provenance.json", [])
            generate.generate_images(client, config, brief(), output, set())
            self.assertEqual(client.post.call_count, 6)
            records = json.loads((output / "provenance.json").read_text())
            self.assertEqual(len(records), 6)
            self.assertEqual(records[0]["modelVersion"], "test-version")
            generate.generate_images(client, config, brief(), output, set())
            self.assertEqual(client.post.call_count, 6)
            (output / records[0]["asset"]).write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "missing or changed"):
                generate.generate_images(client, config, brief(), output, set())

    def test_images_reject_unrecorded_collisions_and_missing_output(self):
        model = {"provider": "mai", "deployment": "test-image", "version": "test-version"}
        config = {"models": [model], "image_constraints": "No text"}
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder)
            generate.write_json(output / "provenance.json", [])
            path = output / "direction-0--test-image--logo.png"
            path.write_bytes(b"do not overwrite")
            with self.assertRaisesRegex(ValueError, "Unrecorded output"):
                generate.generate_images(Mock(), config, brief(), output, set())
            path.unlink()
            client = Mock()
            client.post.return_value = {"data": []}
            with self.assertRaisesRegex(ValueError, "No supported base64"):
                generate.generate_images(client, config, brief(), output, set())
            self.assertEqual(json.loads((output / "provenance.json").read_text()), [])


if __name__ == "__main__":
    unittest.main()
