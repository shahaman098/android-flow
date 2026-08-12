"""LLM calls — open-weight Qwen2.5-3B on MyGCP (Ollama). No DeepSeek.

Generation strategy: a fast draft pass, followed by a *mechanical* quality check
(required template sections present, proper nouns preserved). A repair pass only
runs when the check actually fails, so the common case stays fast while malformed
output gets one automatic corrective pass instead of being pasted as-is.
"""

from __future__ import annotations

import asyncio
import difflib
import logging
import re

import httpx

from .settings import settings

# The `_fallback_prompt` paths below return HTTP 200 with a mechanically-built
# template, so a dead or too-slow LLM is indistinguishable from success at the
# client. Log every fallback so it shows up in Cloud Run logs instead.
logger = logging.getLogger(__name__)

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


async def health() -> dict:
    """Verify that Cloud Run can reach and authenticate to the Ollama proxy."""
    key = settings.hermes_api_key.strip()
    if not key:
        raise RuntimeError("LLM API key is not configured (HERMES_API_KEY / llm-api-key).")

    base = settings.hermes_base_url.rstrip("/")
    async with httpx.AsyncClient(timeout=15.0) as client:
        response = await client.get(
            f"{base}/v1/models",
            headers={"Authorization": f"Bearer {key}"},
        )
    if response.status_code >= 400:
        raise RuntimeError(f"LLM health error ({response.status_code}): {response.text}")

    payload = response.json()
    models = [item.get("id") for item in (payload.get("data") or [])]
    model = settings.hermes_model or "qwen2.5:3b"
    if model not in models:
        raise RuntimeError(f"Configured LLM model {model!r} is not installed. Available: {models}")
    return {"ok": True, "model": model}


async def chat(
    system: str,
    user: str,
    temperature: float = 0.2,
    timeout: float = 500.0,
    max_tokens: int = 900,
) -> str:
    key = settings.hermes_api_key.strip()
    if not key:
        raise RuntimeError("LLM API key is not configured (HERMES_API_KEY / llm-api-key).")
    base = settings.hermes_base_url.rstrip("/")
    model = settings.hermes_model or "qwen2.5:3b"
    body = {
        "model": model,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    }
    async with httpx.AsyncClient(timeout=timeout) as client:
        response = await client.post(
            f"{base}/v1/chat/completions",
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


def _trim_text(text: str, limit: int) -> str:
    text = text.strip()
    if len(text) <= limit:
        return text
    trimmed = text[:limit]
    split_at = trimmed.rfind("\n")
    if split_at >= int(limit * 0.6):
        trimmed = trimmed[:split_at]
    return trimmed.rstrip() + _TRIM_MARKER


def _compact_constitution(text: str, *, mode: str) -> str:
    text = (text or "").strip()
    if not text:
        return "[FILL: constitutions/vibe-coding.md]"

    head = text
    tail = ""
    if "## Quality bar" in text:
        head, tail = text.split("## Quality bar", 1)
    compact = head.strip()

    if mode == "vibe_refine" and "## Refine step" in tail:
        refine_section = "## Refine step" + tail.split("## Refine step", 1)[1]
        if "## Grammar-correction step" in refine_section:
            refine_section = refine_section.split("## Grammar-correction step", 1)[0]
        compact = f"{compact}\n\n{refine_section.strip()}"

    return _trim_text(compact, 2200)


def _compact_skill(text: str) -> str:
    text = (text or "").strip()
    if not text:
        return ""
    if "## Worked example" in text:
        text = text.split("## Worked example", 1)[0].strip()
    return _trim_text(text, 1200)


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


def _fallback_prompt(source_text: str, project_context: str, mode: str) -> str:
    tasks = _task_bullets(source_text)
    context_lines = _context_bullets(project_context)
    output_focus = (
        "Return a concise refined prompt that preserves the user's intent."
        if mode == "vibe_refine"
        else "Return a concise coding prompt ready to hand to an implementation agent."
    )
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
    downstream step (grammar, prompt, refine) sees one consistent spelling."""
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


def _missing_sections(text: str) -> list[str]:
    return [s for s in REQUIRED_SECTIONS if s not in text]


def _proper_nouns(text: str, dictionary: list[str]) -> list[str]:
    """Best-effort extraction of terms that must survive verbatim: dictionary
    entries plus capitalized multi-word phrases / ALLCAPS tokens from the source."""
    terms = {w.strip() for w in dictionary if w.strip()}
    for m in re.finditer(r"\b([A-Z][a-zA-Z0-9]*(?:\s+[A-Z][a-zA-Z0-9]*){0,2})\b", text):
        candidate = m.group(1).strip()
        if len(candidate) > 2:
            terms.add(candidate)
    for m in re.finditer(r"\b[A-Z]{2,}[A-Z0-9]*\b", text):
        terms.add(m.group(0))
    return sorted(terms)


def _dropped_terms(source: str, output: str, dictionary: list[str]) -> list[str]:
    haystack = output.lower()
    dropped = []
    for term in _proper_nouns(source, dictionary):
        if term.lower() not in haystack:
            dropped.append(term)
    return dropped[:12]


async def _repair(draft: str, constitution: str, extra_context: str, dropped: list[str], language: str) -> str:
    constitution = _compact_constitution(constitution, mode="vibe_refine" if extra_context else "vibe")
    extra_context = _compact_project_context(extra_context) if extra_context else ""
    issues = []
    missing = _missing_sections(draft)
    if missing:
        issues.append(f"Missing or empty template sections (add them, using [FILL: ...] if unknown): {', '.join(missing)}")
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
    fixed = await chat(system, user, temperature=0.1, timeout=45.0, max_tokens=320)
    return _strip_wrapper(fixed)


FEW_SHOT_EXAMPLE = """Example — corrected speech in, perfect Vibe Coding prompt out:

INPUT (grammar-corrected speech):
"I want speech to text that automatically fixes grammar and when I press Control 1 it turns the
corrected text into a really good vibe coding prompt and when I press Control 2 it grabs the
whole prompt plus the project context and refines it. Set up folders for context, skills, and
constitutions."

OUTPUT (must match this shape exactly — ten sections, in this order, checklist mapped 1:1 to Task):
**Title**
Speech-to-Text, Grammar Correction, and Vibe Coding Prompt Generation

**Role & Stance**
You are a software architect tasked with designing and coding the specified features for a Vibe Coding application.

**Task**
- Create functionality that converts speech to text.
- Automatically correct the grammar of the transcribed text.
- When the corrected text is activated with Control+1, generate a "perfect" Vibe Coding prompt.
- When Control+2 is pressed, select the entire generated prompt, extract the project's context, and produce a refined Vibe Coding prompt.
- Integrate this functionality into the project codebase with context/, skills/, and constitutions/ folders.

**Context**
The project is a Vibe Coding application that requires seamless speech input, grammar correction, and prompt generation tied to specific keyboard shortcuts.

**Inputs Available**
- Speech input from the user.
- Keyboard shortcuts: Control+1 and Control+2.

**Output Requirements**
- Code implementing speech-to-text, grammar auto-correction, and prompt generation.
- A folder hierarchy showing where skills, constitutions, and supporting files live.
- Brief comments explaining each component.

**Constraints / Do-nots**
- Do not introduce features beyond those listed.
- Preserve all proper nouns exactly as given.
- Do not alter the intended behavior of Control+1 / Control+2.

**Examples / References**
*None provided.*

**Execution Checklist**
- [ ] Speech-to-text conversion.
- [ ] Grammar auto-correction module.
- [ ] Prompt generation on Control+1.
- [ ] Context extraction and refined prompt generation on Control+2.
- [ ] context/, skills/, constitutions/ folder structure with real content.
- [ ] Inline comments describing each part.

**Conflict Resolution**
If any instruction conflicts, follow this priority order: Safety and non-negotiable constraints, Output requirements, Task objective, then Context and examples."""


async def polish(text: str, language: str, dictionary: list[str]) -> str:
    text = _fix_dictionary_homophones(text, dictionary)
    dict_words = ", ".join(dictionary) if dictionary else "none"
    system = f"""You clean up voice dictation into written text. Be surgical — you are not
rewriting, you are correcting.

Rules:
- Fix grammar, spelling, punctuation, and capitalization
- Remove filler words and disfluencies (um, uh, like, you know, stutters/repeats) unless meaningful
- Keep the speaker's meaning and level of detail — never summarize or shorten
- If a word is a near-homophone of a dictionary term (likely an STT mishearing), replace it with
  the exact dictionary term
- Do not add greetings, explanations, or quotes
- Return only the corrected text
- Language preference code: {language}
- Preserve these dictionary terms exactly when present: {dict_words}"""
    result = await chat(system, text, temperature=0.1, max_tokens=500)
    return _strip_wrapper(result)


async def vibe_prompt(
    corrected: str,
    constitution: str,
    language: str,
    dictionary: list[str] | None = None,
    skill: str = "",
) -> str:
    dictionary = dictionary or []
    constitution = _compact_constitution(constitution, mode="vibe")
    skill = _compact_skill(skill)
    system = f"""You generate a perfect Vibe Coding prompt from the user's grammar-corrected speech.

You are NOT implementing the task — you only write the prompt text an AI coding agent will receive.

Constitution:
{constitution or "[FILL: constitutions/vibe-coding.md]"}

{skill.strip() if skill.strip() else FEW_SHOT_EXAMPLE}

Rules:
- Follow the canonical template above exactly — all ten sections, in order, none empty
- Preserve all proper nouns exactly
- Use [FILL: …] placeholders where specifics are unknown — never invent them
    - Do not invent features beyond the user's request
    - Keep each section concise; do not pad the output
    - Return ONLY the prompt — no preamble, no wrapping quotes or code fences
    - Language preference code: {language}"""
    user = f"Create a perfect Vibe Coding prompt from this corrected speech:\n\n{corrected}"
    try:
        draft = _strip_wrapper(await chat(system, user, temperature=0.15, timeout=60.0, max_tokens=260))
    except Exception:
        logger.warning("vibe: draft call failed, returning template fallback", exc_info=True)
        return _fallback_prompt(corrected, "", "vibe")

    missing = _missing_sections(draft)
    dropped = _dropped_terms(corrected, draft, dictionary)
    if missing or dropped:
        try:
            return await _repair(draft, constitution, "", dropped, language)
        except Exception:
            logger.warning("vibe: repair call failed (missing=%s)", missing, exc_info=True)
            return draft if not missing else _fallback_prompt(corrected, "", "vibe")
    return draft


async def refine_prompt(
    selected_prompt: str,
    project_context: str,
    constitution: str,
    language: str,
    dictionary: list[str] | None = None,
    skill: str = "",
) -> str:
    dictionary = dictionary or []
    constitution = _compact_constitution(constitution, mode="vibe_refine")
    project_context = _compact_project_context(project_context)
    skill = _compact_skill(skill)
    system = f"""You refine an existing Vibe Coding prompt using the project's context files.

You are NOT implementing the task — you only improve the prompt.

Constitution:
{constitution or "[FILL: constitutions/vibe-coding.md]"}

{skill.strip()}

Rules:
- Keep the user's core intent from the selected prompt
- Preserve every one of the ten canonical template sections — refining must never drop a section
- Weave in relevant facts from project context naturally, don't just append the raw file
- Preserve proper nouns exactly
- Prefer [FILL: …] when details are still missing
- Do not introduce features beyond those listed in the selected prompt + context
- Keep each section concise; do not pad the output
- Return ONLY the refined prompt — no preamble, no wrapping quotes or code fences
- Language preference code: {language}"""
    user = (
        f"Selected generated prompt:\n\n{selected_prompt}\n\n---\n"
        f"Project context:\n\n{project_context or '[FILL: context/]'}\n\n---\n"
        "Produce the refined Vibe Coding prompt."
    )
    try:
        draft = _strip_wrapper(await chat(system, user, temperature=0.1, timeout=60.0, max_tokens=260))
    except Exception:
        logger.warning("vibe_refine: draft call failed, returning template fallback", exc_info=True)
        return _fallback_prompt(selected_prompt, project_context, "vibe_refine")

    missing = _missing_sections(draft)
    dropped = _dropped_terms(selected_prompt, draft, dictionary)
    if missing or dropped:
        try:
            return await _repair(draft, constitution, project_context, dropped, language)
        except Exception:
            logger.warning("vibe_refine: repair call failed (missing=%s)", missing, exc_info=True)
            return draft if not missing else _fallback_prompt(selected_prompt, project_context, "vibe_refine")
    return draft
