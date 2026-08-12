# Skill: refine-prompt

Selects the current generated prompt, loads `context/`, and produces a refined Vibe Coding prompt
that still follows the constitution's canonical template.

## Trigger

**Control+2** (no speech required).

## Implementation

1. Select-all + copy in the frontmost app (the Control+1 output currently on screen).
2. Load markdown under `context/` + `constitutions/vibe-coding.md`.
3. Cloud mode: `cloud/app/llm.py:refine_prompt()` (draft refine → automated check → repair pass
   only if needed). Local mode: `refine_vibe_prompt()` in `src-tauri/src/dictate.rs`.
4. Paste the refined prompt back in place.

## What "refine" means here

- Weave concrete facts from `context/*.md` into **Context** and **Inputs Available** — don't
  just append the raw file contents.
- Keep every template section from the selected prompt; refining must never drop a section.
- If `context/` states something that contradicts the selected prompt, keep the user's most
  recent stated intent and note the conflict under **Constraints / Do-nots**.
- Tighten and de-duplicate wording without shortening required sections.

`[FILL: path overrides via FLOW_PROJECT_ROOT / Settings → Vibe project root]`
