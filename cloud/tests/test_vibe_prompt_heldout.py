"""Held-out fn+1 checker cases.

These labels are a regression set for the mechanical constitution check.
Do not train DSPy or any other optimizer on this file.
"""

from __future__ import annotations

import importlib.util
import json
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
        hermes_model="deepseek-v4-flash",
        llm_provider="deepseek",
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
FIXTURE = Path(__file__).resolve().parent / "fixtures" / "vibe_prompt_heldout.jsonl"


def load_cases() -> list[dict]:
    cases = []
    for raw in FIXTURE.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        cases.append(json.loads(line))
    return cases


def checker_kinds(source: str, output: str, dictionary: list[str]) -> set[str]:
    kinds = {item["type"] for item in llm._vibe_section_mistakes(output)}
    if llm._dropped_terms(source, output, dictionary):
        kinds.add("dropped_term")
    return kinds


class VibePromptHeldoutTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cases = load_cases()

    def test_fixture_has_thirty_labeled_cases(self):
        self.assertEqual(len(self.cases), 30)
        ids = [case["id"] for case in self.cases]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertTrue(any(case["expect_kinds"] == [] for case in self.cases))
        self.assertTrue(any("empty_section" in case["expect_kinds"] for case in self.cases))
        self.assertTrue(any("out_of_order_section" in case["expect_kinds"] for case in self.cases))

    def test_heldout_cases_match_checker(self):
        failures = []
        for case in self.cases:
            kinds = checker_kinds(case["source"], case["output"], case["dictionary"])
            expected = set(case["expect_kinds"])
            if kinds != expected:
                failures.append(f"{case['id']}: expected {sorted(expected)}, got {sorted(kinds)}")
        self.assertEqual(failures, [])

    def test_does_not_treat_sentence_starters_as_proper_nouns(self):
        terms = llm._proper_nouns("Can you make Flow keep DeepSeek names", ["Flow"])
        self.assertIn("Flow", terms)
        self.assertIn("DeepSeek", terms)
        self.assertNotIn("Can", terms)
        self.assertNotIn("Make", terms)

    def test_empty_and_scrambled_sections_fail_old_substring_check_would_pass(self):
        empty = next(case for case in self.cases if case["id"] == "fail-empty-context")
        scrambled = next(case for case in self.cases if case["id"] == "fail-task-before-title")
        for header in llm.REQUIRED_SECTIONS:
            self.assertIn(header, empty["output"])
            self.assertIn(header, scrambled["output"])
        self.assertIn("empty_section", checker_kinds(empty["source"], empty["output"], empty["dictionary"]))
        self.assertIn(
            "out_of_order_section",
            checker_kinds(scrambled["source"], scrambled["output"], scrambled["dictionary"]),
        )


if __name__ == "__main__":
    unittest.main()
