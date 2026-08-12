# Skill: grammar-correct

Automatically corrects grammar of transcribed text before prompt generation.

## Trigger

Runs after speech-to-text on **Control+1**.

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

Clean written English preserving proper nouns and meaning, ready to feed into `vibe-prompt`.
