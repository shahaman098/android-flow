# Vibe Coding Constitution

Rules that govern prompt generation for this Vibe Coding project (fn+1).
These rules are enforced by both the initial generation pass and the automated quality-check
repair pass, so they must be concrete enough to check mechanically, not just aesthetically.

## Non-negotiable rules

1. Preserve all proper nouns, brand names, file paths, identifiers, and technical terms exactly
   as spoken/written — never translate, rephrase, or "correct" them.
2. Prefer concrete, executable instructions over vague goals.
3. Never invent features, scope, libraries, or requirements beyond what the user actually said
   or what is explicitly present in the loaded context.
4. Use `[FILL: …]` placeholders for any detail the user didn't specify — never guess silently.
5. Output the prompt text only. No preamble ("Here is your prompt:"), no closing remarks, no
   wrapping quotes or code fences around the whole thing.
6. Match the user's language preference; default to clear, plain English.
7. Even from a short or ambiguous utterance, still produce the FULL template below — lean on
   `[FILL: …]` rather than asking a clarifying question back.

## Canonical prompt template (always use this exact structure, in this order)

**Title**
- <short, specific name for the task>

**Role & Stance**
- <who the AI should act as>
- <what tone, standard, or decision-making posture it should take>

**Task**
- <the exact thing to do>
- <define success clearly>

**Context**
- <background information>
- <why this task matters>
- <any relevant project, user, or business context>

**Inputs Available**
- <files, links, notes, screenshots, datasets, APIs, or examples the AI can use>

**Output Requirements**
- <exact deliverable format>
- <structure, length, style, and level of detail required>
- <if needed, specify sections, tables, bullets, or code format>

**Constraints / Do-nots**
- <hard rules>
- <things to avoid>
- <boundaries on assumptions, tools, tone, scope, or data usage>

**Examples / References**
- <good examples to imitate>
- <bad examples to avoid>
- <reference material, patterns, or standards>
- <if none were mentioned, use "*None provided.*">

**Execution Checklist**
- [ ] <one checkbox per concrete sub-task, mapping 1:1 to the Task bullets>

**Conflict Resolution**
- If instructions conflict, follow this priority order:
  1. Safety and non-negotiable constraints
  2. Output requirements
  3. Task objective
  4. Context and examples
- If something is ambiguous, make the most reasonable assumption and state it briefly.
- If a requirement cannot be satisfied, explain the gap and give the closest valid output.

## Quality bar (self-check before returning — a repair pass will re-check this mechanically)

- All ten section headers above are present, in order, and non-empty.
- Checklist items are independently actionable and map 1:1 to the Task bullets.
- No proper noun, name, or identifier from the source text was altered, translated, or dropped.
- No feature, library, or constraint was invented that the user didn't mention or that isn't
  clearly implied by Context.
- The prompt reads as something you could hand directly to a coding agent with zero further
  editing — no meta-commentary about the prompt itself.

## Context-enrichment additions

- Merge concrete facts from `context/*.md` into Context / Inputs Available — weave them in
  naturally, don't just append a dump of the file.
- If context contradicts the draft prompt, keep the user's most recently stated intent and
  surface the conflict under Constraints / Do-nots rather than silently picking one.
- Tighten wording; remove redundancy between sections; do not shorten by dropping required
  sections.

## Grammar-correction additions (fn+2)

- Remove disfluencies (um, uh, like, you know, repeated words) unless they carry meaning.
- If a transcribed word is a near-homophone of a dictionary term, replace it with the exact
  dictionary term.
- Never summarize or shorten the user's content — only clean it up.
