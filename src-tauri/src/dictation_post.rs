//! Post-STT helpers for hold-fn: snippets, edit-command detection, cleanup safety,
//! and dictionary terms learned from user corrections.

use crate::store::Snippet;

const EDIT_MARKERS: &[&str] = &[
    "make this",
    "make it",
    "make that",
    "rewrite",
    "rephrase",
    "shorten",
    "expand this",
    "turn this",
    "convert this",
    "change this",
    "fix this",
    "clean this",
    "bullet",
    "more concise",
    "more formal",
    "more casual",
    "warmer",
    "shorter",
    "longer",
    "summarize",
];

const LEARN_STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "to", "of", "in", "on", "for", "is", "it", "this", "that",
    "with", "was", "were", "be", "as", "at", "by", "from", "not", "but", "if", "then", "so",
    "just", "can", "will", "you", "we", "i", "my", "your", "our", "they", "them", "their",
];

pub fn expand_snippets(text: &str, snippets: &[Snippet]) -> String {
    if text.trim().is_empty() || snippets.is_empty() {
        return text.to_string();
    }

    let mut ordered: Vec<(&Snippet, Vec<char>)> = snippets
        .iter()
        .filter(|s| !s.trigger.trim().is_empty())
        .map(|s| (s, s.trigger.to_lowercase().chars().collect::<Vec<_>>()))
        .collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    let lower = text.to_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let chars: Vec<char> = text.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();

    while i < chars.len() {
        let mut matched = None;
        for (snippet, trigger) in &ordered {
            if trigger.is_empty() || i + trigger.len() > lower_chars.len() {
                continue;
            }
            if lower_chars[i..i + trigger.len()] != trigger[..] {
                continue;
            }
            if !is_phrase_boundary(&lower_chars, i, trigger.len()) {
                continue;
            }
            matched = Some(*snippet);
            break;
        }
        if let Some(snippet) = matched {
            out.push_str(&snippet.expansion);
            i += snippet.trigger.chars().count();
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn is_phrase_boundary(chars: &[char], start: usize, len: usize) -> bool {
    let before_ok = start == 0 || !chars[start - 1].is_alphanumeric();
    let end = start + len;
    let after_ok = end == chars.len() || !chars[end].is_alphanumeric();
    before_ok && after_ok
}

pub fn looks_like_edit_command(utterance: &str, selection: &str) -> bool {
    let selection = selection.trim();
    let utterance = utterance.trim();
    if selection.is_empty() || utterance.is_empty() {
        return false;
    }

    let words = utterance.split_whitespace().count();
    if words > 24 {
        return false;
    }

    let lower = utterance.to_lowercase();
    if EDIT_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return true;
    }

    words <= 12
        && (lower.contains(" this")
            || lower.starts_with("this ")
            || lower.starts_with("make ")
            || lower.starts_with("change ")
            || lower.starts_with("turn "))
}

pub fn is_safe_light_cleanup(source: &str, output: &str) -> bool {
    let source = source.trim();
    let output = output.trim();
    if source.is_empty() {
        return output.is_empty();
    }
    if output.is_empty() {
        return false;
    }

    let ratio = output.len() as f32 / source.len().max(1) as f32;
    if ratio < 0.35 || ratio > 1.9 {
        return false;
    }

    let source_tokens = content_tokens(source);
    let output_tokens = content_tokens(output);
    if source_tokens.is_empty() {
        return true;
    }

    for token in ["not", "no", "never", "dont", "don't", "wont", "won't", "without"] {
        if source_tokens.iter().any(|t| t == token)
            && !output_tokens.iter().any(|t| t == token)
        {
            return false;
        }
    }

    let missing = source_tokens
        .iter()
        .filter(|token| !output_tokens.iter().any(|out| out == *token))
        .count();
    let max_missing = (source_tokens.len() as f32 * 0.35).ceil() as usize;
    missing <= max_missing.max(2)
}

pub fn learnable_dictionary_terms(model_output: &str, gold: &str) -> Vec<String> {
    let output_tokens = content_tokens(model_output);
    let mut learned = Vec::new();
    for raw in gold.split(|c: char| !c.is_alphanumeric() && c != '+' && c != '-') {
        let trimmed = raw.trim();
        if !looks_like_dictionary_term(trimmed) {
            continue;
        }
        if output_tokens.contains(&trimmed.to_lowercase()) {
            continue;
        }
        if learned
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(trimmed))
        {
            continue;
        }
        learned.push(trimmed.to_string());
        if learned.len() >= 5 {
            break;
        }
    }
    learned
}

fn looks_like_dictionary_term(token: &str) -> bool {
    if token.len() < 3 {
        return false;
    }
    if LEARN_STOPWORDS
        .iter()
        .any(|stop| token.eq_ignore_ascii_case(stop))
    {
        return false;
    }
    let letters: String = token.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    if letters.is_empty() {
        return false;
    }
    token.chars().any(|c| c.is_ascii_uppercase())
        || token.contains('+')
        || (token.len() >= 4 && letters.chars().all(|c| c.is_ascii_uppercase()))
}

const PROMPT_LEAK_PREFIXES: &[&str] = &[
    "accurate dictation transcript",
    "preserve product names, hotkeys, and technical terms exactly",
    "spell these product names and terms exactly",
];

const ARTIFACT_LABELS: &[&str] = &[
    "blank audio",
    "silence",
    "music",
    "inaudible",
    "noise",
    "applause",
    "laughter",
    "cough",
    "unknown",
];

/// Drop Whisper prompt leaks and `[BLANK_AUDIO]`-style tags so they never get pasted.
pub fn sanitize_stt_transcript(text: &str) -> String {
    let stripped = strip_prompt_leaks(&strip_bracket_artifacts(text));
    collapse_space(&stripped)
}

pub fn is_unusable_stt(text: &str) -> bool {
    sanitize_stt_transcript(text).is_empty()
}

fn strip_bracket_artifacts(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' || chars[i] == '(' {
            let close = if chars[i] == '[' { ']' } else { ')' };
            if let Some(rel_end) = chars[i + 1..].iter().position(|&c| c == close) {
                let inner: String = chars[i + 1..i + 1 + rel_end].iter().collect();
                let label = inner
                    .replace('_', " ")
                    .replace('-', " ")
                    .to_lowercase()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if ARTIFACT_LABELS.contains(&label.as_str()) {
                    i += rel_end + 2;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn strip_prompt_leaks(text: &str) -> String {
    let mut t = text.trim().to_string();
    loop {
        let lower = t.to_lowercase();
        let Some(prefix) = PROMPT_LEAK_PREFIXES
            .iter()
            .copied()
            .find(|prefix| lower.starts_with(prefix))
        else {
            break;
        };
        let skip = prefix.chars().count();
        t = t
            .chars()
            .skip(skip)
            .collect::<String>()
            .trim_start_matches(|c: char| c == '.' || c == ':' || c == ',' || c.is_whitespace())
            .to_string();
    }
    let lower = t.to_lowercase();
    if lower.starts_with("vocabulary:") {
        return String::new();
    }
    t
}

fn collapse_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn content_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|raw| raw.trim().to_lowercase())
        .filter(|token| token.len() >= 2)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(trigger: &str, expansion: &str) -> Snippet {
        Snippet {
            id: trigger.to_string(),
            trigger: trigger.to_string(),
            expansion: expansion.to_string(),
        }
    }

    #[test]
    fn expands_longest_trigger_first() {
        let snippets = vec![
            snippet("calendar", "WRONG"),
            snippet("my calendar link", "https://cal.example/efi"),
        ];
        assert_eq!(
            expand_snippets("send my calendar link please", &snippets),
            "send https://cal.example/efi please"
        );
    }

    #[test]
    fn does_not_expand_inside_words() {
        let snippets = vec![snippet("cal", "CALENDAR")];
        assert_eq!(expand_snippets("call me later", &snippets), "call me later");
    }

    #[test]
    fn detects_spoken_edit_commands() {
        assert!(looks_like_edit_command(
            "make this more concise",
            "A long selected paragraph about launch timing."
        ));
        assert!(!looks_like_edit_command(
            "tomorrow we should ship the light cleanup and then test snippets in slack",
            "A long selected paragraph about launch timing."
        ));
        assert!(!looks_like_edit_command("make this shorter", ""));
    }

    #[test]
    fn light_cleanup_allows_takebacks_but_keeps_negation() {
        assert!(is_safe_light_cleanup(
            "lets meet at 5 actually 6 tomorrow",
            "Let's meet at 6 tomorrow."
        ));
        assert!(!is_safe_light_cleanup(
            "do not rotate the api key",
            "Rotate the API key."
        ));
    }

    #[test]
    fn learns_proper_nouns_from_user_edits() {
        let learned = learnable_dictionary_terms(
            "tomorrow check floor api latency",
            "Tomorrow check Flow API latency.",
        );
        assert!(learned.iter().any(|term| term == "Flow"));
        assert!(!learned.iter().any(|term| term.eq_ignore_ascii_case("tomorrow")));
        assert!(!learned.iter().any(|term| term.eq_ignore_ascii_case("api")));
    }

    #[test]
    fn drops_whisper_prompt_leak_and_blank_audio() {
        assert_eq!(
            sanitize_stt_transcript("Accurate dictation transcript.[BLANK_AUDIO]"),
            ""
        );
        assert_eq!(
            sanitize_stt_transcript(
                "Accurate dictation transcript. hold fn for raw paste [BLANK_AUDIO]"
            ),
            "hold fn for raw paste"
        );
        assert!(is_unusable_stt("[BLANK_AUDIO]"));
        assert!(!is_unusable_stt("tomorrow check Flow API latency"));
    }
}
