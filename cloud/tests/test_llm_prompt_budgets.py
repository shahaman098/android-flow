from __future__ import annotations

import importlib.util
import sys
import types
import unittest
from pathlib import Path
from unittest.mock import AsyncMock, patch


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


VALID_PROMPT = """**Title**
- fix prompting

**Role & Stance**
- Act as a senior engineer.

**Task**
- Fix the prompting feature.

**Context**
- The app generates prompts from dictation.

**Inputs Available**
- Existing app code.

**Output Requirements**
- A working implementation.

**Constraints / Do-nots**
- Do not invent unrelated features.

**Examples / References**
- *None provided.*

**Execution Checklist**
- [ ] Inspect the prompt path.
- [ ] Fix the issue.

**Conflict Resolution**
- Follow the user's latest request first.
"""


class PromptBudgetTests(unittest.IsolatedAsyncioTestCase):
    async def test_vibe_text_prompt_allows_full_template_output(self):
        mocked_chat = AsyncMock(return_value=VALID_PROMPT)
        with patch.object(llm, "chat", new=mocked_chat):
            output = await llm.vibe_prompt_from_text(
                "fix prompting feature",
                "project context",
                "constitution",
                "en-GB",
                [],
                "skill notes",
            )

        self.assertEqual(output, VALID_PROMPT.strip())
        self.assertEqual(mocked_chat.await_args.kwargs["max_tokens"], llm.REFINE_PROMPT_MAX_TOKENS)
        self.assertGreaterEqual(mocked_chat.await_args.kwargs["max_tokens"], 2400)

    async def test_vibe_trace_records_missing_sections_and_repair(self):
        incomplete = "**Title**\n- Fix Flow prompting\n\n**Task**\n- Fix it"
        mocked_chat = AsyncMock(side_effect=[incomplete, VALID_PROMPT])
        with patch.object(llm, "chat", new=mocked_chat):
            traced = await llm.vibe_prompt_traced(
                "Fix Flow prompting",
                "project context",
                "constitution",
                "en-GB",
                ["Flow"],
                "skill notes",
            )

        self.assertTrue(traced.repaired)
        self.assertEqual(traced.final, VALID_PROMPT.strip())
        kinds = [item["type"] for item in traced.mistakes]
        self.assertIn("missing_section", kinds)
        self.assertTrue(any(item["detail"] == "**Role & Stance**" for item in traced.mistakes))

    async def test_polish_trace_flags_unsafe_rewrite(self):
        mocked_chat = AsyncMock(return_value="Review the production system performance tomorrow.")
        with patch.object(llm, "chat", new=mocked_chat):
            traced = await llm.polish_traced(
                "tomorrow check flow api latency and qwen response time",
                "en-GB",
                ["Flow", "Qwen"],
            )

        self.assertTrue(traced.polish_rejected)
        self.assertEqual(
            traced.final,
            "tomorrow check flow api latency and qwen response time",
        )
        self.assertEqual(traced.mistakes[0]["type"], "unsafe_polish")


if __name__ == "__main__":
    unittest.main()
