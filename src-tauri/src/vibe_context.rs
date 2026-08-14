//! Loads Vibe Coding `context/`, `skills/`, and `constitutions/` from the project root.
//! Used as system guidance for fn+1 prompt generation.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::config::load_config;

// project.md alone is ~3.2k. The old 1800 could not hold it even with nothing else in
// context/, and project.md calls latency "a secondary concern" for fn+1 — so the budget
// is sized to fit the real files whole rather than to shave tokens.
const CONTEXT_CHAR_LIMIT: usize = 6000;
// The compacted constitution (grammar appendix already stripped) is ~3.7k; 3600 cut it.
const CONSTITUTION_CHAR_LIMIT: usize = 4200;
const SKILL_CHAR_LIMIT: usize = 4000;
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

/// Concatenate markdown under `context/` for fn+1 prompt generation.
///
/// Budgeting is per file, not one truncation of the concatenation. Trimming the joined
/// blob spends the whole allowance in directory order, so one long file silently starves
/// every file after it — `cloud-hosting.md` (5.1k of decommissioned-GCP runbook, sorted
/// ahead of `project.md`) left fn+1 with zero project facts and a pile of billing trivia.
/// Each file now gets its own share, and `PRIORITY_FILES` decides who is trimmed last.
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
    files.sort_by_key(|p| priority_rank(p));

    if files.is_empty() {
        return Ok("[FILL: no context markdown files found]".into());
    }

    let entries: Vec<(String, String)> = files
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown.md");
            let body =
                fs::read_to_string(path).unwrap_or_else(|_| "[FILL: unreadable file]".into());
            (format!("### {name}\n\n"), body.trim().to_string())
        })
        .collect();

    // Each file is guaranteed a floor, and whatever the others do not need flows to the one
    // being written now. Reserving the floor for files still to come is what stops a
    // high-priority file from being trimmed while a later short file leaves budget unspent —
    // surplus has to travel in both directions, not just forward.
    let floor = CONTEXT_CHAR_LIMIT / entries.len();
    let mut budget = CONTEXT_CHAR_LIMIT;
    let mut out = String::new();
    for (index, (heading, body)) in entries.iter().enumerate() {
        let reserved: usize = entries[index + 1..]
            .iter()
            .map(|(next_heading, next_body)| (next_heading.len() + next_body.len()).min(floor))
            .sum();
        let room = budget
            .saturating_sub(reserved)
            .saturating_sub(heading.len());
        if room == 0 {
            continue;
        }

        let body = trim_for_latency(body, room);
        budget = budget.saturating_sub(heading.len() + body.len());
        out.push_str(heading);
        out.push_str(&body);
        out.push_str("\n\n");
    }
    Ok(out.trim_end().to_string())
}

/// Files fn+1 cannot do without, most important first. Anything unlisted keeps alphabetical
/// order behind them.
const PRIORITY_FILES: [&str; 2] = ["project.md", "README.md"];

fn priority_rank(path: &Path) -> usize {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    PRIORITY_FILES
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(name))
        .unwrap_or(PRIORITY_FILES.len())
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
                if let Some(value) = cache.iter().find_map(|(cached_key, cached_value)| {
                    (cached_key == &key).then(|| cached_value.clone())
                }) {
                    return value;
                }
                let path = root.join("skills").join(skill_id).join("SKILL.md");
                let value = compact_skill(&read_or_placeholder(
                    &path,
                    &format!("[FILL: skills/{skill_id}/SKILL.md]"),
                ));
                cache.push((key, value.clone()));
                return value;
            }
            let path = root.join("skills").join(skill_id).join("SKILL.md");
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
    // Keep the canonical template + quality bar + context enrichment. Drop only the
    // fn+2 grammar appendix — stripping the quality bar left the model with no
    // mechanical target and hurt fn+1 accuracy.
    let compact = source
        .split_once("## Grammar-correction additions")
        .map(|(head, _)| head.trim().to_string())
        .unwrap_or_else(|| source.to_string());
    trim_for_latency(&compact, CONSTITUTION_CHAR_LIMIT)
}

fn compact_skill(text: &str) -> String {
    // Keep the worked example. Earlier latency trimming cut at "## Worked example",
    // so local fn+1 had skill notes with no shape to imitate.
    trim_for_latency(text.trim(), SKILL_CHAR_LIMIT)
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

    /// The regression that broke fn+1: `project.md` is the file that carries the product
    /// facts, and a long unrelated file sorted ahead of it must never push it out entirely.
    #[test]
    fn the_project_file_always_reaches_the_model() {
        let loaded = load_project_context().expect("context/ should load from the repo");
        assert!(
            loaded.contains("### project.md"),
            "project.md was starved out of the context budget:\n{loaded}"
        );
    }

    /// The real repo must fit whole — if these files start getting cut, the budget is wrong.
    #[test]
    fn the_current_context_files_are_not_trimmed_at_all() {
        let loaded = load_project_context().expect("context/ should load from the repo");
        assert!(
            !loaded.contains(TRIM_MARKER),
            "context/ no longer fits in CONTEXT_CHAR_LIMIT:\n{loaded}"
        );
    }

    /// Priority decides who is trimmed, never who is dropped — every file keeps a share.
    #[test]
    fn a_long_low_priority_file_cannot_starve_the_others() {
        let mut ranked = vec![
            PathBuf::from("context/zzz-runbook.md"),
            PathBuf::from("context/project.md"),
            PathBuf::from("context/README.md"),
        ];
        ranked.sort();
        ranked.sort_by_key(|p| priority_rank(p));
        let order: Vec<&str> = ranked
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(order, ["project.md", "README.md", "zzz-runbook.md"]);
    }

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
