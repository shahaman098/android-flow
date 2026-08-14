# Skill: grammar-correct

Corrects grammar, spelling, punctuation, and capitalization in the current text.

## Trigger

Runs on **fn+2** after Flow captures the current field text. Prompt generation on **fn+1**
still performs its own cleanup before producing a Vibe Coding prompt.

## Implementation

- Cloud mode: `cloud/app/llm.py:polish()`. Local mode: `polish_text()` in
  `src-tauri/src/dictate.rs`.
- Model: Qwen2.5-14B via Ollama on MyGCP (`flow-llm` VM), low temperature (deterministic).

## Rules

- Fix grammar, spelling, punctuation, capitalization.
- Remove disfluencies (um, uh, like, you know, stutters/repeats) unless meaningful.
- Preserve meaning — never summarize or shorten.
- Preserve dictionary terms exactly; if a word is a near-homophone of a dictionary term
  (a likely STT mishearing), replace it with the exact dictionary term.
- Return only the corrected text — no preamble, quotes, or commentary.

## Output

Clean written English preserving proper nouns and meaning, pasted back into the active field.
