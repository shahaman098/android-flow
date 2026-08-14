from __future__ import annotations

import base64
import importlib.util
import sys
import types
import unittest
from pathlib import Path
from unittest.mock import AsyncMock, patch

from fastapi.testclient import TestClient


def load_main_module():
    root = Path(__file__).resolve().parents[1]
    package = types.ModuleType("app")
    package.__path__ = [str(root / "app")]
    sys.modules["app"] = package
    sys.modules.pop("app.settings", None)
    sys.modules.pop("app.main", None)
    sys.modules.pop("app.llm", None)
    sys.modules.pop("app.stt", None)
    sys.modules.pop("app.auth", None)

    settings_mod = types.ModuleType("app.settings")
    settings_mod.settings = types.SimpleNamespace(
        default_language="en-GB",
        flow_api_key="test-key",
        gcp_project_id="test-project",
        gcp_location="europe-west2",
        groq_api_key="test-groq",
        groq_stt_model="whisper-large-v3-turbo",
        hermes_api_key="test-llm",
        hermes_base_url="https://api.deepseek.com",
        hermes_model="deepseek-v4-flash",
        llm_provider="deepseek",
        stt_model="latest_long",
        stt_provider="groq_whisper",
    )
    sys.modules["app.settings"] = settings_mod

    spec = importlib.util.spec_from_file_location("app.main", root / "app" / "main.py")
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["app.main"] = module
    spec.loader.exec_module(module)
    return module


main = load_main_module()


class CloudModeTests(unittest.TestCase):
    def setUp(self):
        self.client = TestClient(main.app)
        self.headers = {"Authorization": "Bearer test-key"}
        self.audio = base64.b64encode(b"fake audio").decode()

    def test_dictate_polishes_transcript_and_keeps_raw(self):
        traced = main.llm.GenerationTrace(
            source="raw words um",
            final="Raw words.",
        )
        with (
            patch.object(main.stt, "transcribe", new=AsyncMock(return_value="raw words um")),
            patch.object(main.llm, "polish_traced", new=AsyncMock(return_value=traced)) as polish,
        ):
            response = self.client.post(
                "/v1/process",
                headers=self.headers,
                json={"mode": "dictate", "audio_base64": self.audio},
            )

        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json()["text"], "Raw words.")
        self.assertEqual(response.json()["raw_transcript"], "raw words um")
        polish.assert_awaited_once()

    def test_dictate_selected_text_skips_stt(self):
        traced = main.llm.GenerationTrace(source="raw words", final="Raw words.")
        with (
            patch.object(main.stt, "transcribe", new=AsyncMock()) as transcribe,
            patch.object(main.llm, "polish_traced", new=AsyncMock(return_value=traced)),
        ):
            response = self.client.post(
                "/v1/process",
                headers=self.headers,
                json={"mode": "dictate", "selected_text": "raw words"},
            )

        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json()["raw_transcript"], "raw words")
        transcribe.assert_not_called()

    def test_edit_text_rewrites_selection(self):
        with patch.object(
            main.llm,
            "edit_selected_text",
            new=AsyncMock(return_value="Shorter text."),
        ):
            response = self.client.post(
                "/v1/process",
                headers=self.headers,
                json={
                    "mode": "edit_text",
                    "selected_text": "A much longer selected paragraph.",
                    "project_context": "make this shorter",
                },
            )

        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json()["text"], "Shorter text.")

    def test_correct_text_returns_polished_selected_text(self):
        traced = main.llm.GenerationTrace(source="raw words", final="Corrected words.")
        with patch.object(main.llm, "polish_traced", new=AsyncMock(return_value=traced)):
            response = self.client.post(
                "/v1/process",
                headers=self.headers,
                json={"mode": "correct_text", "selected_text": "raw words"},
            )

        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json()["text"], "Corrected words.")
        self.assertIsNone(response.json()["raw_transcript"])
        self.assertIn("trace", response.json())
        self.assertEqual(response.json()["trace"]["polish_rejected"], False)

    def test_livez_is_public_and_cheap(self):
        response = self.client.get("/livez")
        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json(), {"ok": True})

    def test_health_requires_api_key(self):
        response = self.client.get("/health")
        self.assertEqual(response.status_code, 401)

    def test_health_with_key_checks_backends(self):
        with (
            patch.object(
                main.llm,
                "health",
                new=AsyncMock(return_value={"ok": True, "provider": "deepseek", "model": "x"}),
            ),
            patch.object(
                main.stt,
                "health",
                new=AsyncMock(return_value={"ok": True, "provider": "groq_whisper", "model": "y"}),
            ),
        ):
            response = self.client.get("/health", headers=self.headers)
        self.assertEqual(response.status_code, 200)
        self.assertTrue(response.json()["ok"])


if __name__ == "__main__":
    unittest.main()
