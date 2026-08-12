# Skill: vibe-prompt

Turns grammar-corrected speech into a perfect Vibe Coding prompt.

## Trigger

**Control+1** after STT + grammar correction.

## Implementation

- Cloud mode: `cloud/app/llm.py:vibe_prompt()` (draft → automated template/proper-noun check →
  repair pass only if the check fails)
- Local mode: `generate_vibe_prompt()` in `src-tauri/src/dictate.rs`
- System guidance from `constitutions/vibe-coding.md` (canonical template + quality bar)

## Worked example (few-shot anchor)

Spoken/corrected input:

> "I want speech to text that automatically fixes grammar and when I press Control 1 it turns
> the corrected text into a really good vibe coding prompt and when I press Control 2 it grabs
> the whole prompt plus the project context and refines it. Set up folders for context, skills,
> and constitutions."

Expected output (follows the constitution's template exactly):

```
**Title**
Speech-to-Text, Grammar Correction, and Vibe Coding Prompt Generation

**Role & Stance**
You are a software architect tasked with designing and coding the specified features for a
Vibe Coding application.

**Task**
- Create functionality that converts speech to text.
- Automatically correct the grammar of the transcribed text.
- When the corrected text is activated with Control+1, generate a "perfect" Vibe Coding prompt.
- When Control+2 is pressed, select the entire generated prompt, extract the project's context,
  and produce a refined Vibe Coding prompt.
- Integrate this functionality into the project codebase with `context/`, `skills/`, and
  `constitutions/` folders.

**Context**
The project is a Vibe Coding application that requires seamless speech input, grammar
correction, and prompt generation tied to specific keyboard shortcuts.

**Inputs Available**
- Speech input from the user.
- Keyboard shortcuts: Control+1 and Control+2.

**Output Requirements**
- Code implementing speech-to-text, grammar auto-correction, and prompt generation.
- A folder hierarchy showing where skills, constitutions, and supporting files live.
- Brief comments explaining each component.
- `[FILL: target language/framework if not already fixed by the codebase]`

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
- [ ] `context/`, `skills/`, `constitutions/` folder structure with real content.
- [ ] Inline comments describing each part.

**Conflict Resolution**
If any instruction conflicts, follow this priority order: Safety and non-negotiable constraints,
Output requirements, Task objective, then Context and examples.
```

Notice every section from the constitution's template is present, in order, non-empty, and the
checklist maps 1:1 to the Task bullets. This is the bar every generated prompt must clear.

## Output

A single structured prompt pasted into the frontmost app.
