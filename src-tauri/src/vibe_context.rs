//! Loads Vibe Coding `context/`, `skills/`, and `constitutions/` from the project root.
//! Used by Control+2 refine and as system guidance for Control+1 prompt generation.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::config::load_config;

const CONTEXT_CHAR_LIMIT: usize = 1800;
const CONSTITUTION_CHAR_LIMIT: usize = 2200;
const SKILL_CHAR_LIMIT: usize = 1200;
const TRIM_MARKER: &str = "\n\n[Trimmed for latency]";

/// Resolve the Vibe Coding project root (folder that contains `context/`).
pub fn project_root() -> Result<PathBuf, String> {
    let cfg = load_config();
    if !cfg.vibe_project_root.trim().is_empty() {
        let p = PathBuf::from(cfg.vibe_project_root.trim());
        if p.is_dir() {
            return Ok(p);
        }
        return Err(format!(
            "vibe_project_root is set but not a directory: {}",
            p.display()
        ));
    }

    if let Ok(p) = env::var("FLOW_PROJECT_ROOT") {
        let p = PathBuf::from(p.trim());
        if p.is_dir() {
            return Ok(p);
        }
    }

    // Dev/build: src-tauri/../
    let manifest_parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "Could not resolve project root from CARGO_MANIFEST_DIR".to_string())?;
    if manifest_parent.join("context").is_dir() {
        return Ok(manifest_parent);
    }

    Err(
        "Could not find Vibe Coding project root (expected context/). Set vibe_project_root in Settings or FLOW_PROJECT_ROOT."
            .into(),
    )
}

/// Concatenate markdown under `context/` for Control+2.
pub fn load_project_context() -> Result<String, String> {
    let root = project_root()?;
    let dir = root.join("context");
    if !dir.is_dir() {
        return Err(format!(
            "Missing context/ folder at {}. [FILL: create context files]",
            dir.display()
        ));
    }

    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(|e| format!("Failed to read context/: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("md"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();

    if files.is_empty() {
        return Ok("[FILL: no context markdown files found]".into());
    }

    let mut out = String::new();
    for path in files {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown.md");
        let body = fs::read_to_string(&path).unwrap_or_else(|_| "[FILL: unreadable file]".into());
        out.push_str(&format!("### {name}\n\n{body}\n\n"));
    }
    Ok(trim_for_latency(&out, CONTEXT_CHAR_LIMIT))
}

/// Load the Vibe Coding constitution text.
pub fn load_constitution() -> String {
    match project_root() {
        Ok(root) => {
            let key = root.display().to_string();
            if let Ok(mut cache) = constitution_cache().lock() {
                if let Some((cached_key, cached_value)) = cache.as_ref() {
                    if cached_key == &key {
                        return cached_value.clone();
                    }
                }
                let path = root.join("constitutions").join("vibe-coding.md");
                let value = compact_constitution(&read_or_placeholder(
                    &path,
                    "[FILL: constitutions/vibe-coding.md missing]",
                ));
                *cache = Some((key, value.clone()));
                return value;
            }
            let path = root.join("constitutions").join("vibe-coding.md");
            compact_constitution(&read_or_placeholder(
                &path,
                "[FILL: constitutions/vibe-coding.md missing]",
            ))
        }
        Err(_) => "[FILL: constitutions/vibe-coding.md unavailable — set project root]".into(),
    }
}

/// Optional skill guidance snippets (kept short for the LLM system prompt).
pub fn load_skill_blurb(skill_id: &str) -> String {
    match project_root() {
        Ok(root) => {
            let key = format!("{}::{skill_id}", root.display());
            if let Ok(mut cache) = skill_cache().lock() {
                if let Some(value) = cache
                    .iter()
                    .find_map(|(cached_key, cached_value)| {
                        (cached_key == &key).then(|| cached_value.clone())
                    })
                {
                    return value;
                }
                let path = root
                    .join("skills")
                    .join(skill_id)
                    .join("SKILL.md");
                let value = compact_skill(&read_or_placeholder(
                    &path,
                    &format!("[FILL: skills/{skill_id}/SKILL.md]"),
                ));
                cache.push((key, value.clone()));
                return value;
            }
            let path = root
                .join("skills")
                .join(skill_id)
                .join("SKILL.md");
            compact_skill(&read_or_placeholder(
                &path,
                &format!("[FILL: skills/{skill_id}/SKILL.md]"),
            ))
        }
        Err(_) => format!("[FILL: skills/{skill_id}/SKILL.md unavailable]"),
    }
}

fn constitution_cache() -> &'static Mutex<Option<(String, String)>> {
    static CACHE: OnceLock<Mutex<Option<(String, String)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn skill_cache() -> &'static Mutex<Vec<(String, String)>> {
    static CACHE: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn read_or_placeholder(path: &Path, placeholder: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|_| placeholder.to_string())
}

fn compact_constitution(text: &str) -> String {
    let source = text.trim();
    let mut compact = source.to_string();
    if let Some((head, tail)) = source.split_once("## Quality bar") {
        compact = head.trim().to_string();
        if let Some((_, refine_tail)) = tail.split_once("## Refine step") {
            let refine = format!("## Refine step{refine_tail}");
            let refine = refine
                .split_once("## Grammar-correction step")
                .map(|(head, _)| head.trim().to_string())
                .unwrap_or_else(|| refine.trim().to_string());
            compact.push_str("\n\n");
            compact.push_str(&refine);
        }
    }
    trim_for_latency(&compact, CONSTITUTION_CHAR_LIMIT)
}

fn compact_skill(text: &str) -> String {
    let compact = text
        .split_once("## Worked example")
        .map(|(head, _)| head.trim().to_string())
        .unwrap_or_else(|| text.trim().to_string());
    trim_for_latency(&compact, SKILL_CHAR_LIMIT)
}

fn trim_for_latency(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.len() <= limit {
        return text.to_string();
    }

    // The limits are byte counts, but the inputs are user-authored markdown. Slicing at
    // `limit` directly panics whenever a multi-byte character straddles that offset — an
    // em dash or curly quote in any context/*.md is enough — and this runs on every
    // request. Walk back to the nearest character boundary before cutting.
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    let mut trimmed = &text[..end];
    // Prefer cutting on a line break, but only when that keeps most of the budget.
    if let Some(split_at) = trimmed.rfind('\n') {
        if split_at >= (limit * 3) / 5 {
            trimmed = &trimmed[..split_at];
        }
    }
    format!("{}{}", trimmed.trim_end(), TRIM_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_returned_untouched() {
        assert_eq!(trim_for_latency("small body", 100), "small body");
        assert!(!trim_for_latency("small body", 100).contains(TRIM_MARKER));
    }

    #[test]
    fn never_splits_a_multibyte_character() {
        // "é" is 2 bytes, so byte 11 falls inside the 6th one. Slicing there directly
        // panicked; the cut must move back to byte 10.
        let text = "é".repeat(20);
        let out = trim_for_latency(&text, 11);
        assert_eq!(out, format!("{}{}", "é".repeat(5), TRIM_MARKER));
    }

    #[test]
    fn trims_markdown_containing_em_dashes_without_panicking() {
        // The realistic case: context/*.md prose full of 3-byte punctuation. Every offset
        // must be survivable, not just the ones that happen to line up.
        let text = "Flow — the Mac helper — pastes prompts. ".repeat(40);
        for limit in 1..=text.len() {
            let out = trim_for_latency(&text, limit);
            // Either it fit (returned as-is, whitespace-trimmed) or it was cut and marked.
            assert!(out.ends_with(TRIM_MARKER) || out == text.trim());
        }
    }

    #[test]
    fn prefers_cutting_on_a_line_break_when_one_is_close_enough() {
        let text = "aaaaaaaaaaaaaaaaaaaa\nbbbbbbbbbb";
        // The newline sits at byte 20, past 3/5 of a 25-byte budget, so it wins.
        assert_eq!(
            trim_for_latency(text, 25),
            format!("aaaaaaaaaaaaaaaaaaaa{TRIM_MARKER}")
        );
    }

    #[test]
    fn ignores_a_line_break_that_would_throw_away_most_of_the_budget() {
        let text = "aa\nbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        // Newline at byte 2 is well under 3/5 of 25, so the hard cut is kept instead.
        assert_eq!(
            trim_for_latency(text, 25),
            format!("aa\nbbbbbbbbbbbbbbbbbbbbbb{TRIM_MARKER}")
        );
    }
}
