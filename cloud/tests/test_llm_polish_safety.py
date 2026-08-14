from __future__ import annotations

import importlib.util
import sys
import types
import unittest
from pathlib import Path


def load_llm_module():
    root = Path(__file__).resolve().parents[1]
    package = types.ModuleType("app")
    package.__path__ = [str(root / "app")]
    settings_mod = types.ModuleType("app.settings")
    settings_mod.settings = types.SimpleNamespace(
        hermes_api_key="test",
        hermes_base_url="http://127.0.0.1:8080",
        hermes_model="qwen2.5:3b",
    )
    sys.modules["app"] = package
    sys.modules["app.settings"] = settings_mod

    spec = importlib.util.spec_from_file_location("app.llm", root / "app" / "llm.py")
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["app.llm"] = module
    spec.loader.exec_module(module)
    return module


llm = load_llm_module()


class PolishSafetyTests(unittest.TestCase):
    def test_accepts_punctuation_and_case_cleanup(self):
        source = "tomorrow check flow api latency and qwen response time"
        output = "Tomorrow, check Flow API latency and Qwen response time."
        self.assertTrue(llm._is_safe_polish(source, output, ["Flow", "Qwen"]))

    def test_rejects_summary_or_note_rewrite(self):
        source = "tomorrow check flow api latency and qwen response time"
        output = "Review the production system performance tomorrow."
        self.assertFalse(llm._is_safe_polish(source, output, ["Flow", "Qwen"]))

    def test_rejects_dropped_negation(self):
        source = "do not rotate the flow api key today"
        output = "Rotate the Flow API key today."
        self.assertFalse(llm._is_safe_polish(source, output, ["Flow"]))

    def test_allows_dictionary_homophone_replacement(self):
        source = "check floor api logs"
        output = "Check Flow API logs."
        self.assertTrue(llm._is_safe_polish(source, output, ["flow"]))


if __name__ == "__main__":
    unittest.main()
