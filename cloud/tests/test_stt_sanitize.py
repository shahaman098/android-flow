from __future__ import annotations

import importlib.util
import sys
import types
import unittest
from pathlib import Path


def load_stt_module():
    root = Path(__file__).resolve().parents[1]
    package = types.ModuleType("app")
    package.__path__ = [str(root / "app")]
    settings_mod = types.ModuleType("app.settings")
    settings_mod.settings = types.SimpleNamespace(
        default_language="en-GB",
        groq_api_key="test",
        groq_stt_model="whisper-large-v3-turbo",
        stt_provider="groq_whisper",
        gcp_project_id="test",
        gcp_location="europe-west2",
        stt_model="latest_long",
    )
    sys.modules["app"] = package
    sys.modules["app.settings"] = settings_mod
    spec = importlib.util.spec_from_file_location("app.stt", root / "app" / "stt.py")
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["app.stt"] = module
    spec.loader.exec_module(module)
    return module


stt = load_stt_module()


class SttSanitizeTests(unittest.TestCase):
    def test_drops_prompt_leak_and_blank_audio(self):
        self.assertEqual(
            stt.sanitize_transcript("Accurate dictation transcript.[BLANK_AUDIO]"),
            "",
        )

    def test_keeps_real_words_after_leak(self):
        self.assertEqual(
            stt.sanitize_transcript(
                "Accurate dictation transcript. hold fn for raw paste [BLANK_AUDIO]"
            ),
            "hold fn for raw paste",
        )

    def test_non_english_language_is_not_forced_to_gb(self):
        self.assertEqual(stt.language_bcp47("es"), "es")
        self.assertEqual(stt.language_bcp47("fr"), "fr")
        self.assertEqual(stt.language_bcp47("en"), "en-GB")


if __name__ == "__main__":
    unittest.main()
