# Skill: vibe-prompt

Turns rough text from the active field into a perfect Vibe Coding prompt using project context.

## Trigger

Bubble **Vibe prompt** after reading the focused field text.

## Implementation

- `cloud/app/llm.py:vibe_prompt_from_text()` (draft → automated template/proper-noun check →
  repair pass only if the check fails)
- Client: `android/.../service/BubbleOverlayService.kt` → `FlowApiClient.vibeText`
- Insert: `FlowAccessibilityService.insertOrReplaceText`
- System guidance from `constitutions/vibe-coding.md` (canonical template + quality bar)

There is no local Tauri / `src-tauri` path in this repository.

## Worked example (few-shot anchor)

Rough input text:

> "I want speech to text that light-cleans dictation when I hold fn. When I hold fn+1 it turns
> the current text plus project context into a really good vibe coding prompt. When I press fn+2
> it corrects the grammar and spelling of the current text. When I press fn+3 it answers the latest
> question from the live conversation transcript. Set up folders for context, skills, and
> constitutions."

Expected output (follows the constitution's template exactly):

```
**Title**
Speech-to-Text, Grammar Correction, and Vibe Coding Prompt Generation

**Role & Stance**
You are a software architect tasked with designing and coding the specified features for a
Vibe Coding application.

**Task**
- Create functionality that converts speech to text.
- Light-clean dictation when fn is held.
- When fn+1 is pressed, generate a "perfect" Vibe Coding prompt from the current text and project context.
- When fn+2 is pressed, correct the grammar and spelling of the current text.
- When fn+3 is pressed, answer the latest question detected in the live conversation transcript.
- Integrate this functionality into the project codebase with `context/`, `skills/`, and
  `constitutions/` folders.

**Context**
The project is a Vibe Coding application that requires seamless raw dictation, prompt generation,
current-text correction, and live-conversation question answering tied to specific keyboard shortcuts.

**Inputs Available**
- Current field text from the user.
- Project context files.
- Live conversation transcript when answering questions.
- Keyboard shortcuts: fn, fn+1, fn+2, and fn+3.

**Output Requirements**
- Code implementing raw dictation, current-text prompt generation, current-text correction, and live question answering.
- A folder hierarchy showing where skills, constitutions, and supporting files live.
- Brief comments explaining each component.
- `[FILL: target language/framework if not already fixed by the codebase]`

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
- [ ] `context/`, `skills/`, `constitutions/` folder structure with real content.
- [ ] Inline comments describing each part.

**Conflict Resolution**
If any instruction conflicts, follow this priority order: Safety and non-negotiable constraints,
Output requirements, Task objective, then Context and examples.
```

Notice every section from the constitution's template is present, in order, non-empty, and the
checklist maps 1:1 to the Task bullets. This is the bar every generated prompt must clear.

## Output

A single structured prompt inserted into the focused field.
