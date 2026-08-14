"""LLM calls via an OpenAI-compatible API. DeepSeek is the cheap cloud default.

Generation strategy: a fast draft pass, followed by a *mechanical* quality check
(required template sections present, in order, and non-empty; proper nouns preserved
with the same named-token heuristic as the Mac path). A repair pass only runs when
the check actually fails, so the common case stays fast while malformed output gets
one automatic corrective pass instead of being pasted as-is.
"""

from __future__ import annotations

import asyncio
import difflib
import logging
import re
from dataclasses import dataclass, field

import httpx

from .settings import settings

# The `_fallback_prompt` paths below return HTTP 200 with a mechanically-built
# template, so a dead or too-slow LLM is indistinguishable from success at the
# client. Log every fallback so it shows up in Cloud Run logs instead.
logger = logging.getLogger(__name__)


@dataclass
class GenerationTrace:
    """Structured record of one LLM pass, for training and mistake review."""

    source: str = ""
    draft: str = ""
    final: str = ""
    repaired: bool = False
    used_fallback: bool = False
    polish_rejected: bool = False
    mistakes: list[dict] = field(default_factory=list)

    def as_api(self) -> dict:
        payload = {
            "repaired": self.repaired,
            "used_fallback": self.used_fallback,
            "polish_rejected": self.polish_rejected,
            "mistakes": self.mistakes,
        }
        if self.draft:
            payload["draft"] = self.draft
        return payload

    def log(self, mode: str) -> None:
        logger.info(
            "training_trace mode=%s repaired=%s fallback=%s polish_rejected=%s mistakes=%s",
            mode,
            self.repaired,
            self.used_fallback,
            self.polish_rejected,
            [item.get("type") for item in self.mistakes],
            extra={
                "mode": mode,
                "source_chars": len(self.source),
                "draft_chars": len(self.draft),
                "final_chars": len(self.final),
                "repaired": self.repaired,
                "used_fallback": self.used_fallback,
                "polish_rejected": self.polish_rejected,
                "mistakes": self.mistakes,
            },
        )

REQUIRED_SECTIONS = [
    "**Title**",
    "**Role & Stance**",
    "**Task**",
    "**Context**",
    "**Inputs Available**",
    "**Output Requirements**",
    "**Constraints / Do-nots**",
    "**Examples / References**",
    "**Execution Checklist**",
    "**Conflict Resolution**",
]

_PREAMBLE_PREFIXES = (
    "here is",
    "here's",
    "here are",
    "sure,",
    "sure!",
    "certainly,",
    "certainly!",
    "below is",
    "of course,",
)

_TRIM_MARKER = "\n\n[Trimmed for latency]"

_CONTENT_STOPWORDS = {
    "a",
    "an",
    "and",
    "are",
    "as",
    "at",
    "be",
    "by",
    "for",
    "from",
    "i",
    "in",
    "is",
    "it",
    "of",
    "on",
    "or",
    "that",
    "the",
    "this",
    "to",
    "was",
    "were",
    "with",
}

_FILLER_TOKENS = {"ah", "eh", "er", "erm", "hmm", "um", "uh", "like", "yeah", "actually", "wait", "sorry"}

# Sentence-start verbs/pronouns that are not product names. Keep in sync with
# `is_likely_named_single_token` in src-tauri/src/dictate.rs.
_NAMED_SINGLE_TOKEN_STOPWORDS = {
    "Add",
    "Build",
    "Can",
    "Change",
    "Check",
    "Clean",
    "Create",
    "Delete",
    "Fix",
    "Generate",
    "Help",
    "I",
    "Make",
    "Move",
    "Remove",
    "Run",
    "Set",
    "Show",
    "Tell",
    "Update",
    "Use",
    "Verify",
    "What",
    "When",
    "Why",
}

_HOTKEYS = ("fn", "fn+1", "fn+2", "fn+3", "Control+1", "Control+2")

_NEGATION_TOKENS = {
    "cannot",
    "can't",
    "dont",
    "don't",
    "never",
    "no",
    "not",
    "shouldnt",
    "shouldn't",
    "without",
    "wont",
    "won't",
}

VIBE_PROMPT_MAX_TOKENS = 2200
REFINE_PROMPT_MAX_TOKENS = 2600
REPAIR_PROMPT_MAX_TOKENS = 2600


async def health() -> dict:
    """Verify that Cloud Run can reach and authenticate to the configured LLM API."""
    key = settings.hermes_api_key.strip()
    if not key:
        raise RuntimeError("LLM API key is not configured (LLM_API_KEY / DEEPSEEK_API_KEY / XAI_API_KEY).")

    async with httpx.AsyncClient(timeout=15.0) as client:
        response = await client.get(
            _api_url("models"),
            headers={"Authorization": f"Bearer {key}"},
        )
    if response.status_code >= 400:
        raise RuntimeError(f"LLM health error ({response.status_code}): {response.text}")

    payload = response.json()
    models = [item.get("id") for item in (payload.get("data") or [])]
    model = settings.hermes_model or "deepseek-v4-flash"
    return {
        "ok": True,
        "provider": settings.llm_provider,
        "model": model,
        "available_models": models[:20],
    }


def _api_url(path: str) -> str:
    base = settings.hermes_base_url.rstrip("/")
    return f"{base}/{path.lstrip('/')}"


async def chat(
    system: str,
    user: str,
    temperature: float = 0.2,
    timeout: float = 90.0,
    max_tokens: int = 900,
) -> str:
    key = settings.hermes_api_key.strip()
    if not key:
        raise RuntimeError("LLM API key is not configured (LLM_API_KEY / DEEPSEEK_API_KEY / XAI_API_KEY).")
    model = settings.hermes_model or "deepseek-v4-flash"
    body = {
        "model": model,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    }
    if settings.llm_provider.strip().lower() == "deepseek":
        body["thinking"] = {"type": "disabled"}
    async with httpx.AsyncClient(timeout=timeout) as client:
        response = await client.post(
            _api_url("chat/completions"),
            headers={"Authorization": f"Bearer {key}"},
            json=body,
        )
    if response.status_code >= 400:
        raise RuntimeError(f"LLM error ({response.status_code}): {response.text}")

    data = response.json()
    choices = data.get("choices") or []
    if not choices:
        raise RuntimeError("LLM returned no choices.")
    content = (choices[0].get("message") or {}).get("content") or ""
    content = content.strip()
    if not content:
        raise RuntimeError("LLM returned empty content.")
    return content


def _strip_wrapper(text: str) -> str:
    """Remove common model artifacts: fenced code blocks around the whole answer,
    a leading "Here is..." preamble line, or a single pair of wrapping quotes."""
    t = text.strip()

    if t.startswith("```") and t.endswith("```") and len(t) > 6:
        inner = t[3:-3].strip("\n")
        first_nl = inner.find("\n")
        if first_nl != -1 and len(inner[:first_nl].split()) <= 1:
            inner = inner[first_nl + 1 :]
        t = inner.strip()

    lines = t.split("\n")
    if lines and lines[0].strip().lower().startswith(_PREAMBLE_PREFIXES):
        rest = "\n".join(lines[1:]).lstrip("\n")
        if rest.strip():
            t = rest.strip()

    if len(t) > 1 and t[0] in "\"'" and t[-1] == t[0]:
        t = t[1:-1].strip()

    return t.strip()


def _content_tokens(text: str) -> list[str]:
    """Tokens used to verify that polish did not rewrite the user's note.

    This intentionally ignores small grammar glue words but keeps negations,
    numbers, product names, commands, and longer content words.
    """
    tokens = []
    for token in re.findall(r"[A-Za-z0-9][A-Za-z0-9'._/-]*", text.lower()):
        stripped = token.strip("'._/-")
        if not stripped or stripped in _CONTENT_STOPWORDS or stripped in _FILLER_TOKENS:
            continue
        if len(stripped) < 3 and not stripped.isdigit():
            continue
        tokens.append(stripped)
    return tokens


def _token_preserved(source_token: str, output_tokens: set[str], dictionary: list[str]) -> bool:
    if source_token in output_tokens:
        return True

    # Allow real spelling corrections, but not arbitrary substitutions.
    for out in output_tokens:
        if difflib.SequenceMatcher(None, source_token, out).ratio() >= 0.86:
            return True

    # Dictionary homophone replacement can legitimately replace a misheard token.
    for term in dictionary:
        term_token = term.strip().lower()
        if not term_token or term_token not in output_tokens:
            continue
        if difflib.SequenceMatcher(None, source_token, term_token).ratio() >= 0.6:
            return True

    return False


def _is_safe_polish(source: str, output: str, dictionary: list[str]) -> bool:
    """Reject LLM polish that behaves like a rewrite rather than a correction."""
    source = source.strip()
    output = output.strip()
    if not source:
        return not output
    if not output:
        return False

    ratio = len(output) / max(len(source), 1)
    if ratio < 0.4 or ratio > 1.8:
        return False

    source_tokens = _content_tokens(source)
    output_tokens = set(_content_tokens(output))
    if not source_tokens:
        return True

    if any(token in _NEGATION_TOKENS and token not in output_tokens for token in source_tokens):
        return False

    missing = [
        token
        for token in source_tokens
        if token not in _FILLER_TOKENS
        and not _token_preserved(token, output_tokens, dictionary)
    ]
    max_missing = 0 if len(source_tokens) <= 4 else max(2, int(len(source_tokens) * 0.25))
    if len(missing) > max_missing:
        return False

    added = [
        token for token in output_tokens if not _token_preserved(token, set(source_tokens), dictionary)
    ]
    max_added = max(2, int(len(source_tokens) * 0.25))
    if len(added) > max_added:
        return False

    return True


def _trim_text(text: str, limit: int) -> str:
    text = text.strip()
    if len(text) <= limit:
        return text
    trimmed = text[:limit]
    split_at = trimmed.rfind("\n")
    if split_at >= int(limit * 0.6):
        trimmed = trimmed[:split_at]
    return trimmed.rstrip() + _TRIM_MARKER


def _compact_constitution(text: str) -> str:
    text = (text or "").strip()
    if not text:
        return "[FILL: constitutions/vibe-coding.md]"

    # Keep template + quality bar + context enrichment. Drop only the fn+2 grammar appendix.
    if "## Grammar-correction additions" in text:
        text = text.split("## Grammar-correction additions", 1)[0].strip()

    return _trim_text(text, 3600)


def _compact_skill(text: str) -> str:
    text = (text or "").strip()
    if not text:
        return ""
    # Keep the worked example — stripping it left fn+1 with no shape to imitate.
    return _trim_text(text, 4000)


def _compact_project_context(text: str) -> str:
    text = (text or "").strip()
    if not text:
        return "[FILL: context/]"
    return _trim_text(text, 1800)


def _clean_line(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip(" -:\n\t")


def _sentence_parts(text: str) -> list[str]:
    parts = [_clean_line(p) for p in re.split(r"[.\n;]+", text) if _clean_line(p)]
    return parts


def _title_from_text(text: str) -> str:
    words = [w for w in re.findall(r"[A-Za-z0-9+/._'-]+", text) if w]
    if not words:
        return "Prompt Request"
    title = " ".join(words[:6]).strip()
    return title[:80]


def _task_bullets(text: str) -> list[str]:
    parts = _sentence_parts(text)
    bullets = parts[:5]
    if not bullets and text.strip():
        bullets = [_clean_line(text)]
    return bullets or ["[FILL: specify the exact work to do]"]


def _context_bullets(project_context: str) -> list[str]:
    lines = []
    for raw in project_context.splitlines():
        raw = raw.strip()
        if raw.startswith("- "):
            cleaned = _clean_line(raw[2:])
            if cleaned:
                lines.append(cleaned)
        elif raw and not raw.startswith("#") and len(lines) < 4:
            cleaned = _clean_line(raw)
            if cleaned:
                lines.append(cleaned)
        if len(lines) >= 4:
            break
    return lines


def _fallback_prompt(source_text: str, project_context: str) -> str:
    tasks = _task_bullets(source_text)
    context_lines = _context_bullets(project_context)
    output_focus = "Return a concise coding prompt ready to hand to an implementation agent."
    checklist = "\n".join(f"- [ ] {item}" for item in tasks[:5])
    context_block = "\n".join(f"- {item}" for item in context_lines[:3]) or "- [FILL: add relevant project context]"
    return f"""**Title**
- {_title_from_text(source_text)}

**Role & Stance**
- Act as a pragmatic software engineer.
- Be concise, concrete, and implementation-focused.

**Task**
{chr(10).join(f"- {item}" for item in tasks)}

**Context**
{context_block}

**Inputs Available**
- Source text: {source_text.strip() or "[FILL: provide source text]"}
- Project context loaded by Flow when available.

**Output Requirements**
- {output_focus}
- Keep all ten required sections in this template.
- Use short bullets and avoid padding.

**Constraints / Do-nots**
- Do not invent requirements beyond the source text and provided context.
- Preserve proper nouns, identifiers, and product names exactly.
- Use [FILL: ...] when something required is unknown.

**Examples / References**
*None provided.*

**Execution Checklist**
{checklist}

**Conflict Resolution**
- If instructions conflict, follow this priority order:
  1. Safety and non-negotiable constraints
  2. Output requirements
  3. Task objective
  4. Context and examples
- If something is ambiguous, make the most reasonable assumption and state it briefly.
- If a requirement cannot be satisfied, explain the gap and give the closest valid output."""


def _fix_dictionary_homophones(text: str, dictionary: list[str]) -> str:
    """STT often mis-hears proper nouns as similar-sounding common words (e.g. "Nimbus" ->
    "numbers"). Fix this mechanically, before any LLM step, so the correct term is present
    from the start instead of relying on the model to guess the phonetic link — and so every
    downstream step (grammar or prompt generation) sees one consistent spelling."""
    single_word_terms = [w.strip() for w in dictionary if w.strip() and " " not in w.strip()]
    if not single_word_terms:
        return text

    tokens = re.findall(r"[A-Za-z']+", text)
    fixed = text
    for term in single_word_terms:
        term_lower = term.lower()
        for tok in {t for t in tokens if t.lower() != term_lower}:
            if tok[0].lower() != term_lower[0]:
                continue
            if abs(len(tok) - len(term)) > 3:
                continue
            ratio = difflib.SequenceMatcher(None, tok.lower(), term_lower).ratio()
            if ratio >= 0.6:
                fixed = re.sub(rf"\b{re.escape(tok)}\b", term, fixed)
    return fixed


def _section_body_is_empty(body: str) -> bool:
    return not any(ch.isalnum() for ch in body)


def _vibe_section_mistakes(text: str) -> list[dict]:
    """Constitution quality bar: all ten headers present, in order, and non-empty."""
    positions = [text.find(section) for section in REQUIRED_SECTIONS]
    present = [(index, pos) for index, pos in enumerate(positions) if pos >= 0]
    present.sort(key=lambda item: item[1])
    next_at = [len(text)] * len(REQUIRED_SECTIONS)
    for idx, (section_index, _pos) in enumerate(present):
        end = present[idx + 1][1] if idx + 1 < len(present) else len(text)
        next_at[section_index] = end

    mistakes: list[dict] = []
    last_pos: int | None = None
    for index, (section, pos) in enumerate(zip(REQUIRED_SECTIONS, positions)):
        if pos < 0:
            mistakes.append({"type": "missing_section", "detail": section})
            continue
        if last_pos is not None and pos <= last_pos:
            mistakes.append({"type": "out_of_order_section", "detail": section})
        else:
            last_pos = pos
        body = text[pos + len(section) : next_at[index]]
        if _section_body_is_empty(body):
            mistakes.append({"type": "empty_section", "detail": section})
    return mistakes


def _missing_sections(text: str) -> list[str]:
    """Section headers that fail the constitution check (missing, empty, or out of order)."""
    details: list[str] = []
    seen: set[str] = set()
    for item in _vibe_section_mistakes(text):
        detail = item["detail"]
        if detail not in seen:
            seen.add(detail)
            details.append(detail)
    return details


def _trim_non_alnum(token: str) -> str:
    start = 0
    end = len(token)
    while start < end and not token[start].isalnum():
        start += 1
    while end > start and not token[end - 1].isalnum():
        end -= 1
    return token[start:end]


def _is_likely_named_single_token(token: str) -> bool:
    if token and all(not ch.islower() for ch in token):
        return True
    return token not in _NAMED_SINGLE_TOKEN_STOPWORDS


def _flush_proper_term(current: list[str], terms: list[str]) -> None:
    while current and not _is_likely_named_single_token(current[0]):
        current.pop(0)
    if not current:
        return
    joined = " ".join(current)
    keep = len(current) > 1 or _is_likely_named_single_token(current[0])
    if keep and len(joined) > 2:
        terms.append(joined)
    current.clear()


def _proper_nouns(text: str, dictionary: list[str]) -> list[str]:
    """Same named-token heuristic as src-tauri/src/dictate.rs `proper_terms`."""
    terms = [word.strip() for word in dictionary if len(word.strip()) > 1]
    for hotkey in _HOTKEYS:
        if hotkey in text:
            terms.append(hotkey)

    current: list[str] = []
    for raw in text.split():
        token = _trim_non_alnum(raw)
        if len(token) < 2:
            _flush_proper_term(current, terms)
            continue
        starts_uppercase = token[0].isascii() and token[0].isupper()
        letters = [ch for ch in token if ch.isascii() and ch.isalpha()]
        all_caps = bool(letters) and all(ch.isupper() for ch in letters)
        if starts_uppercase or all_caps:
            current.append(token)
        else:
            _flush_proper_term(current, terms)
    _flush_proper_term(current, terms)

    deduped: list[str] = []
    seen: set[str] = set()
    for term in sorted(terms, key=str.lower):
        key = term.lower()
        if key in seen:
            continue
        seen.add(key)
        deduped.append(term)
    return deduped


def _dropped_terms(source: str, output: str, dictionary: list[str]) -> list[str]:
    haystack = output.lower()
    dropped = []
    for term in _proper_nouns(source, dictionary):
        if term.lower() not in haystack:
            dropped.append(term)
    return dropped[:12]


def _section_repair_issues(draft: str) -> list[str]:
    mistakes = _vibe_section_mistakes(draft)
    missing = [item["detail"] for item in mistakes if item["type"] == "missing_section"]
    empty = [item["detail"] for item in mistakes if item["type"] == "empty_section"]
    ordered = [item["detail"] for item in mistakes if item["type"] == "out_of_order_section"]
    issues: list[str] = []
    if missing:
        issues.append(
            f"Missing required template sections: {', '.join(missing)}. "
            "Add each missing section using [FILL: ...] when details are unknown."
        )
    if empty:
        issues.append(
            f"Empty template sections: {', '.join(empty)}. "
            "Put real content in each, using [FILL: ...] when details are unknown."
        )
    if ordered:
        issues.append(
            f"Template sections out of order: {', '.join(ordered)}. "
            "Keep the canonical order from the constitution."
        )
    return issues


async def _repair(draft: str, constitution: str, extra_context: str, dropped: list[str], language: str) -> str:
    constitution = _compact_constitution(constitution)
    extra_context = _compact_project_context(extra_context) if extra_context else ""
    issues = _section_repair_issues(draft)
    if dropped:
        issues.append(
            f"These exact terms from the source are missing from the draft, or appear there "
            f"under a different/incorrect name — find every place the draft refers to that "
            f"same thing and replace it with the exact term, consistently everywhere it occurs "
            f"in the document (do not leave two different names for the same entity): "
            f"{', '.join(dropped)}"
        )

    system = f"""You are a strict editor for Vibe Coding prompts, enforcing this constitution:

{constitution}

The DRAFT below has specific, mechanically-detected problems. Fix ONLY those problems and
otherwise preserve the draft's wording and structure exactly. Do not shorten unrelated sections.
When fixing a name/term, apply it consistently across the ENTIRE document — every section.

Detected problems:
- {chr(10).join(issues) if issues else "General quality pass — tighten wording, ensure the canonical template is followed exactly."}

Return ONLY the corrected final prompt text. No preamble, no explanation of what you changed,
no wrapping quotes or code fences.
Language preference code: {language}"""
    user = f"DRAFT:\n\n{draft}\n\n---\nAdditional context to weave in if relevant:\n{extra_context or 'none'}"
    fixed = await chat(system, user, temperature=0.1, timeout=45.0, max_tokens=REPAIR_PROMPT_MAX_TOKENS)
    return _strip_wrapper(fixed)


FEW_SHOT_EXAMPLE = """Example — rough current text in, perfect Vibe Coding prompt out:

INPUT (rough current text):
"I want speech to text that light-cleans dictation when I hold fn. When I hold fn+1 it turns
the current text plus project context into a really good vibe coding prompt. When I press fn+2
it corrects the grammar and spelling of the current text. When I press fn+3 it answers the latest
question from the live conversation transcript. Set up folders for context, skills, and constitutions."

OUTPUT (must match this shape exactly — ten sections, in this order, checklist mapped 1:1 to Task):
**Title**
Speech-to-Text, Grammar Correction, and Vibe Coding Prompt Generation

**Role & Stance**
You are a software architect tasked with designing and coding the specified features for a Vibe Coding application.

**Task**
- Create functionality that converts speech to text.
- Light-clean dictation when fn is held.
- When fn+1 is pressed, generate a "perfect" Vibe Coding prompt from the current text and project context.
- When fn+2 is pressed, correct the grammar and spelling of the current text.
- When fn+3 is pressed, answer the latest question detected in the live conversation transcript.
- Integrate this functionality into the project codebase with context/, skills/, and constitutions/ folders.

**Context**
The project is a Vibe Coding application that requires seamless light-cleaned dictation, prompt generation, current-text correction, and live-conversation question answering tied to specific keyboard shortcuts.

**Inputs Available**
- Current field text from the user.
- Project context files.
- Live conversation transcript when answering questions.
- Keyboard shortcuts: fn, fn+1, fn+2, and fn+3.

**Output Requirements**
- Code implementing raw dictation, current-text prompt generation, current-text correction, and live question answering.
- A folder hierarchy showing where skills, constitutions, and supporting files live.
- Brief comments explaining each component.

**Constraints / Do-nots**
- Do not introduce features beyond those listed.
- Preserve all proper nouns exactly as given.
- Do not alter the intended behavior of fn / fn+1 / fn+2 / fn+3.

**Examples / References**
*None provided.*

**Execution Checklist**
- [ ] Speech-to-text conversion.
- [ ] Light-cleaned dictation on fn.
- [ ] Prompt generation on fn+1.
- [ ] Current-text correction on fn+2.
- [ ] Live conversation question answering on fn+3.
- [ ] context/, skills/, constitutions/ folder structure with real content.
- [ ] Inline comments describing each part.

**Conflict Resolution**
If any instruction conflicts, follow this priority order: Safety and non-negotiable constraints, Output requirements, Task objective, then Context and examples."""


async def polish(text: str, language: str, dictionary: list[str]) -> str:
    return (await polish_traced(text, language, dictionary)).final


async def polish_traced(text: str, language: str, dictionary: list[str]) -> GenerationTrace:
    text = _fix_dictionary_homophones(text, dictionary)
    dict_words = ", ".join(dictionary) if dictionary else "none"
    system = f"""You clean up voice dictation into written text. Be conservative and surgical:
you are correcting transcription mechanics, not rewriting notes.

Rules:
- Preserve the user's words, order, meaning, note style, and level of detail
- Fix only clear transcription, spelling, punctuation, capitalization, and grammatical errors
- Keep fragments and rough notes as fragments; do not convert them into polished prose
- Do not remove words unless they are obvious accidental stutters, filler (um, uh, like, you know), or repeated duplicates
- Honor mid-sentence take-backs: "meet at 5, actually 6" becomes "meet at 6"
- Do not summarize, shorten beyond take-backs, expand, rephrase, professionalize, or change tone
- Preserve negations exactly (not, no, never, don't, won't, without)
- Preserve numbers, dates, commands, file paths, code terms, product names, and proper nouns exactly
- If a word is a near-homophone of a dictionary term (likely an STT mishearing), replace it with
  the exact dictionary term
- Do not add greetings, explanations, or quotes
- If unsure whether a change is safe, leave that part unchanged
- Return only the corrected text
- Language preference code: {language}
- Preserve these dictionary terms exactly when present: {dict_words}"""
    result = await chat(system, text, temperature=0.1, max_tokens=500)
    corrected = _strip_wrapper(result)
    trace = GenerationTrace(source=text, draft=corrected, final=corrected)
    if not _is_safe_polish(text, corrected, dictionary):
        logger.warning(
            "polish rejected unsafe rewrite; returning STT text",
            extra={
                "source_chars": len(text),
                "output_chars": len(corrected),
                "source_tokens": len(_content_tokens(text)),
                "output_tokens": len(_content_tokens(corrected)),
            },
        )
        trace.polish_rejected = True
        trace.final = text.strip()
        trace.mistakes.append(
            {
                "type": "unsafe_polish",
                "detail": "Model rewrote the note instead of correcting it; original text was kept.",
            }
        )
        trace.log("correct_text")
        return trace
    trace.log("correct_text")
    return trace


async def edit_selected_text(
    selected: str,
    instruction: str,
    language: str,
    dictionary: list[str] | None = None,
) -> str:
    dict_words = ", ".join(dictionary or []) if dictionary else "none"
    system = f"""You edit the selected text according to the user's spoken instruction.

Rules:
- Apply only the requested change
- Preserve meaning that the instruction does not ask to change
- Preserve proper nouns, numbers, paths, and dictionary terms
- Return only the edited text
- No preamble, quotes, or explanation
- Language preference code: {language}
- Preserve these dictionary terms exactly when present: {dict_words}"""
    user = (
        f"Selected text:\n{selected}\n\n---\nSpoken instruction:\n{instruction}\n\n"
        "---\nReturn the edited text."
    )
    result = await chat(system, user, temperature=0.15, max_tokens=700)
    edited = _strip_wrapper(result).strip()
    return edited or selected.strip()


async def vibe_prompt_from_text(
    rough_text: str,
    project_context: str,
    constitution: str,
    language: str,
    dictionary: list[str] | None = None,
    skill: str = "",
) -> str:
    return (
        await vibe_prompt_traced(
            rough_text, project_context, constitution, language, dictionary, skill
        )
    ).final


async def vibe_prompt_traced(
    rough_text: str,
    project_context: str,
    constitution: str,
    language: str,
    dictionary: list[str] | None = None,
    skill: str = "",
) -> GenerationTrace:
    dictionary = dictionary or []
    constitution = _compact_constitution(constitution)
    project_context = _compact_project_context(project_context)
    skill = _compact_skill(skill)
    system = f"""You turn the user's existing rough text into a perfect Vibe Coding prompt.

The rough text may be notes, a messy request, or an under-specified prompt. Optimize it into
the final prompt an AI coding agent will receive.

You are NOT implementing the task — you only write the prompt text.

Constitution:
{constitution or "[FILL: constitutions/vibe-coding.md]"}

{skill.strip() if skill.strip() else FEW_SHOT_EXAMPLE}

Rules:
- Preserve the user's core goal and constraints
- Use relevant project context naturally; do not dump raw context
- Follow the canonical template above exactly — all ten sections, in order, none empty
- Preserve all proper nouns exactly
- Use [FILL: …] placeholders where specifics are unknown — never invent them
- Do not invent features beyond the user's rough text and provided project context
- Keep each section concise; do not pad the output
- Return ONLY the prompt — no preamble, no wrapping quotes or code fences
- Language preference code: {language}"""
    user = (
        f"Rough text from the active field:\n\n{rough_text}\n\n---\n"
        f"Project context:\n\n{project_context or '[FILL: context/]'}\n\n---\n"
        "Create the optimized Vibe Coding prompt."
    )
    trace = GenerationTrace(source=rough_text)
    try:
        draft = _strip_wrapper(
            await chat(
                system,
                user,
                temperature=0.1,
                timeout=60.0,
                max_tokens=REFINE_PROMPT_MAX_TOKENS,
            )
        )
    except Exception:
        logger.warning("vibe_text: draft call failed, returning template fallback", exc_info=True)
        trace.used_fallback = True
        trace.final = _fallback_prompt(rough_text, project_context)
        trace.mistakes.append(
            {
                "type": "fallback_template",
                "detail": "Draft LLM call failed; mechanical template was returned.",
            }
        )
        trace.log("vibe_text")
        return trace

    trace.draft = draft
    section_mistakes = _vibe_section_mistakes(draft)
    dropped = _dropped_terms(rough_text, draft, dictionary)
    trace.mistakes.extend(section_mistakes)
    for term in dropped:
        trace.mistakes.append({"type": "dropped_term", "detail": term})
    if section_mistakes or dropped:
        try:
            repaired = await _repair(draft, constitution, project_context, dropped, language)
            trace.repaired = True
            trace.final = repaired
            for item in _vibe_section_mistakes(repaired):
                trace.mistakes.append(
                    {"type": f"remaining_{item['type']}", "detail": item["detail"]}
                )
            for term in _dropped_terms(rough_text, repaired, dictionary):
                trace.mistakes.append({"type": "remaining_dropped_term", "detail": term})
            trace.log("vibe_text")
            return trace
        except Exception:
            structural = [
                item["detail"]
                for item in section_mistakes
                if item["type"] in {"missing_section", "empty_section"}
            ]
            logger.warning("vibe_text: repair call failed (structural=%s)", structural, exc_info=True)
            if structural:
                trace.used_fallback = True
                trace.final = _fallback_prompt(rough_text, project_context)
                trace.mistakes.append(
                    {
                        "type": "fallback_template",
                        "detail": "Repair LLM call failed; mechanical template was returned.",
                    }
                )
            else:
                trace.final = draft
            trace.log("vibe_text")
            return trace
    trace.final = draft
    trace.log("vibe_text")
    return trace


async def answer_meeting_question(question: str, transcript: str, language: str) -> str:
    return (await answer_meeting_question_traced(question, transcript, language)).final


async def answer_meeting_question_traced(question: str, transcript: str, language: str) -> GenerationTrace:
    system = f"""You help the user answer a question from a live conversation.

Use the transcript context to draft a concise answer the user can say or paste.

Rules:
- Answer only the detected question
- Be direct, natural, and useful in a live conversation
- Use facts from the transcript when relevant
- Do not invent private facts or commitments not supported by the conversation
- If the transcript does not contain enough information, give a safe answer that says what is known and what needs checking
- Return only the answer text, with no preamble, quotes, or explanation
- Language preference code: {language}"""
    user = (
        f"Detected question:\n{question}\n\n---\n"
        f"Recent live conversation transcript:\n{transcript}\n\n---\n"
        "Draft the answer."
    )
    answer = _strip_wrapper(
        await chat(system, user, temperature=0.2, timeout=45.0, max_tokens=900)
    )
    trace = GenerationTrace(source=question, final=answer)
    trace.log("meeting_answer")
    return trace
