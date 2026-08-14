# Skill: grammar-correct

Corrects grammar, spelling, punctuation, and capitalization in the current text.

## Trigger

Runs on bubble **Fix grammar** after Sprout reads the focused field. Bubble **Vibe prompt**
still performs its own cleanup before producing a Vibe Coding prompt.

## Implementation

- `cloud/app/llm.py:polish()` via `POST /v1/process` `mode=correct_text`
- Default cloud LLM: DeepSeek `deepseek-v4-flash`
- Client: `android/.../service/BubbleOverlayService.kt` → `FlowApiClient.correctText`
- Insert: `FlowAccessibilityService.insertOrReplaceText`

## Rules

- Fix grammar, spelling, punctuation, capitalization.
- Remove disfluencies (um, uh, like, you know, stutters/repeats) unless meaningful.
- Preserve meaning — never summarize or shorten.
- Preserve dictionary terms exactly; if a word is a near-homophone of a dictionary term
  (a likely STT mishearing), replace it with the exact dictionary term.
- Return only the corrected text — no preamble, quotes, or commentary.

## Output

Clean written English preserving proper nouns and meaning, inserted into the focused field.
