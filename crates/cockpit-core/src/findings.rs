//! Advisory reviewer-agent findings: the JSON contract parser.
//!
//! The read-only pre-pass reviewer ([`crate::model::AgentMode::Review`]) is
//! instructed to emit a JSON array of findings (see
//! [`crate::prompt::assemble_review_prompt`] and the `REVIEW_INSTRUCTION`
//! contract). This module turns that agent output into typed
//! [`ReviewFinding`]s.
//!
//! Parsing is deliberately **tolerant**: agents routinely wrap their output in
//! prose or Markdown code fences, and individual entries can be sloppy. The
//! parser therefore extracts the first bracket region that parses as a JSON
//! array, skips entries it cannot make sense of, and fills defaults for the
//! forgiving fields rather than failing the whole batch. The one hard failure
//! is "no array at all", so a caller can log that the reviewer produced nothing
//! usable.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::model::{DiffSide, FindingSeverity, ReviewFinding};

/// Upper bound on the number of findings returned from a single parse.
///
/// A misbehaving agent could emit thousands of entries; capping keeps memory
/// and UI bounded. Valid entries beyond this count are truncated (dropped).
const MAX_FINDINGS: usize = 200;

/// Errors from parsing reviewer findings output.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The reviewer output contained no bracket region that parsed as a JSON
    /// array. Distinct from a found-but-empty array, which is `Ok(vec![])`.
    #[error("no JSON array found in reviewer findings output")]
    NoArrayFound,
}

/// Parse an agent's reviewer output into a list of [`ReviewFinding`]s.
///
/// The input may contain arbitrary prose or code fences around the array; the
/// first bracket region that parses as a JSON array is used (see
/// [`extract_findings_array`]). Malformed entries are skipped rather than
/// failing the parse:
///
/// - an entry that is not a JSON object, or that lacks a non-empty `path` or
///   `title`, is dropped;
/// - an unknown or missing `severity` becomes [`FindingSeverity::Warning`];
/// - a missing or unknown `side` becomes [`DiffSide::New`];
/// - a range whose `line_end` precedes `line_start` is swapped;
/// - a missing line number defaults to `0`.
///
/// Each returned finding gets a stable id of the form `f{index}-{hash8}`, where
/// `index` is its position in the returned list and `hash8` is the first eight
/// hex digits of `sha256("{path}|{start}-{end}|{title}")`. The same input parses
/// to the same ids every time.
///
/// Returns [`Error::NoArrayFound`] only when no JSON array can be located; a
/// located-but-empty array yields `Ok(vec![])`. At most [`MAX_FINDINGS`]
/// findings are returned.
pub fn parse_findings(json: &str) -> Result<Vec<ReviewFinding>, Error> {
    let array = extract_findings_array(json).ok_or(Error::NoArrayFound)?;

    let mut findings = Vec::new();
    for value in &array {
        if findings.len() >= MAX_FINDINGS {
            break;
        }
        // The id index is the finding's final position, so re-parsing the same
        // input reproduces the same ids.
        if let Some(finding) = value_to_finding(value, findings.len()) {
            findings.push(finding);
        }
    }
    Ok(findings)
}

/// Extract the first bracket region that parses as a JSON array.
///
/// Scans for each `[` and, using a string-aware balanced-bracket walk
/// ([`scan_balanced`]), finds its matching `]`. The enclosed slice is then
/// tried as a JSON array; the first slice that parses wins. This tolerates
/// leading/trailing prose and Markdown code fences without a fragile regex.
///
/// Byte scanning is safe for UTF-8: the only bytes acted on (`[`, `]`, `"`,
/// `\`) are ASCII, and multi-byte code points never contain those byte values,
/// so slice boundaries always fall on char boundaries.
///
/// Heuristic note: the first *parseable* JSON array is assumed to be the
/// findings array. A stray JSON array embedded in prose ahead of the real
/// findings would be picked instead; in practice the reviewer emits only the
/// findings array (optionally fenced), so this holds.
fn extract_findings_array(text: &str) -> Option<Vec<Value>> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(end) = scan_balanced(bytes, i) {
                // `i` and `end` index ASCII `[`/`]`, so this range is valid.
                if let Ok(array) = serde_json::from_str::<Vec<Value>>(&text[i..=end]) {
                    return Some(array);
                }
            }
        }
        i += 1;
    }
    None
}

/// Find the index of the `]` that closes the `[` at `start`.
///
/// Walks bytes tracking string context (honoring `\` escapes) so brackets and
/// quotes inside JSON string values are ignored. Returns `None` if the array is
/// never closed.
fn scan_balanced(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Convert a single JSON value into a [`ReviewFinding`], or `None` if it is too
/// malformed to represent (not an object, or missing a `path`/`title`).
fn value_to_finding(value: &Value, index: usize) -> Option<ReviewFinding> {
    let obj = value.as_object()?;

    let path = obj.get("path").and_then(Value::as_str)?.trim();
    if path.is_empty() {
        return None;
    }
    let title = obj.get("title").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        return None;
    }

    let range = read_range(obj);
    let severity = parse_severity(obj.get("severity").and_then(Value::as_str));
    let side = parse_side(obj.get("side").and_then(Value::as_str));
    let rationale = obj
        .get("rationale")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();

    let id = finding_id(index, path, range, title);

    Some(ReviewFinding {
        id,
        severity,
        path: PathBuf::from(path),
        range,
        side,
        title: title.to_owned(),
        rationale,
    })
}

/// Read the `(line_start, line_end)` range, defaulting missing values to `0`
/// and swapping so `start <= end`.
fn read_range(obj: &Map<String, Value>) -> (u32, u32) {
    let start = read_line(obj, "line_start");
    let end = read_line(obj, "line_end");
    let (start, end) = match (start, end) {
        (Some(s), Some(e)) => (s, e),
        (Some(s), None) => (s, s),
        (None, Some(e)) => (e, e),
        (None, None) => (0, 0),
    };
    if end < start {
        (end, start)
    } else {
        (start, end)
    }
}

/// Read a single line-number field, clamping out-of-`u32`-range values.
fn read_line(obj: &Map<String, Value>, key: &str) -> Option<u32> {
    obj.get(key)
        .and_then(Value::as_u64)
        .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
}

/// Map a raw severity string to a [`FindingSeverity`], case-insensitively.
///
/// Anything unrecognized or absent becomes [`FindingSeverity::Warning`] — the
/// safe middle default for an advisory note.
fn parse_severity(raw: Option<&str>) -> FindingSeverity {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("info") => FindingSeverity::Info,
        Some("critical") => FindingSeverity::Critical,
        _ => FindingSeverity::Warning,
    }
}

/// Map a raw side string to a [`DiffSide`], case-insensitively.
///
/// Anything other than `"old"` (including absent) becomes [`DiffSide::New`],
/// matching the model's own default.
fn parse_side(raw: Option<&str>) -> DiffSide {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("old") => DiffSide::Old,
        _ => DiffSide::New,
    }
}

/// Build the stable id `f{index}-{hash8}` for a finding.
///
/// `hash8` is the first eight hex digits (four bytes) of
/// `sha256("{path}|{start}-{end}|{title}")`, so identical content hashes
/// identically across parses.
fn finding_id(index: usize, path: &str, range: (u32, u32), title: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{path}|{}-{}|{title}", range.0, range.1).as_bytes());
    let digest = hasher.finalize();

    // INVARIANT: write! into a String is infallible.
    let mut hex = String::with_capacity(8);
    for byte in digest.iter().take(4) {
        write!(hex, "{byte:02x}").unwrap();
    }
    format!("f{index}-{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_parses_all_fields() {
        let json = r#"[
            {"severity":"Critical","path":"src/main.rs","line_start":10,"line_end":15,"side":"New","title":"Missing error handling","rationale":"The result is ignored."},
            {"severity":"Info","path":"src/lib.rs","line_start":42,"line_end":42,"side":"Old","title":"Nit","rationale":"Prefer as_str."}
        ]"#;

        let findings = parse_findings(json).expect("should parse");
        assert_eq!(findings.len(), 2);

        let first = &findings[0];
        assert_eq!(first.severity, FindingSeverity::Critical);
        assert_eq!(first.path, PathBuf::from("src/main.rs"));
        assert_eq!(first.range, (10, 15));
        assert_eq!(first.side, DiffSide::New);
        assert_eq!(first.title, "Missing error handling");
        assert_eq!(first.rationale, "The result is ignored.");
        assert!(first.id.starts_with("f0-"));

        let second = &findings[1];
        assert_eq!(second.severity, FindingSeverity::Info);
        assert_eq!(second.side, DiffSide::Old);
        assert!(second.id.starts_with("f1-"));
    }

    #[test]
    fn junk_wrapped_array_is_extracted() {
        // Prose plus a fenced code block, exactly the shape agents emit.
        let json = "Here are the findings I spotted [see the diff]:\n\
            ```json\n\
            [{\"severity\":\"Warning\",\"path\":\"a.rs\",\"line_start\":1,\"line_end\":2,\"title\":\"x\",\"rationale\":\"y\"}]\n\
            ```\n\
            Let me know if you want more detail.";

        let findings = parse_findings(json).expect("should parse through the junk");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path, PathBuf::from("a.rs"));
        assert_eq!(findings[0].range, (1, 2));
    }

    #[test]
    fn malformed_entries_are_skipped() {
        // Entry 1 is a bare string, entry 2 lacks a path, entry 3 is valid.
        let json = r#"[
            "not an object",
            {"severity":"Info","title":"no path here","line_start":1,"line_end":1},
            {"severity":"Warning","path":"ok.rs","line_start":3,"line_end":4,"title":"kept","rationale":"r"}
        ]"#;

        let findings = parse_findings(json).expect("should parse");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "kept");
        // The surviving finding is numbered from the output position, not input.
        assert!(findings[0].id.starts_with("f0-"));
    }

    #[test]
    fn unknown_severity_defaults_to_warning() {
        let json = r#"[{"severity":"catastrophic","path":"a.rs","line_start":1,"line_end":1,"title":"t","rationale":"r"}]"#;
        let findings = parse_findings(json).expect("should parse");
        assert_eq!(findings[0].severity, FindingSeverity::Warning);
    }

    #[test]
    fn missing_severity_defaults_to_warning() {
        let json = r#"[{"path":"a.rs","line_start":1,"line_end":1,"title":"t","rationale":"r"}]"#;
        let findings = parse_findings(json).expect("should parse");
        assert_eq!(findings[0].severity, FindingSeverity::Warning);
    }

    #[test]
    fn missing_side_defaults_to_new() {
        let json = r#"[{"severity":"Info","path":"a.rs","line_start":1,"line_end":1,"title":"t","rationale":"r"}]"#;
        let findings = parse_findings(json).expect("should parse");
        assert_eq!(findings[0].side, DiffSide::New);
    }

    #[test]
    fn swapped_range_is_normalized() {
        let json = r#"[{"severity":"Info","path":"a.rs","line_start":20,"line_end":5,"title":"t","rationale":"r"}]"#;
        let findings = parse_findings(json).expect("should parse");
        assert_eq!(findings[0].range, (5, 20));
    }

    #[test]
    fn no_array_is_an_error() {
        let json = "I could not find any issues worth reporting.";
        assert!(matches!(parse_findings(json), Err(Error::NoArrayFound)));
    }

    #[test]
    fn empty_array_is_ok_and_empty() {
        let json = "No problems found: []";
        let findings = parse_findings(json).expect("empty array is not an error");
        assert!(findings.is_empty());
    }

    #[test]
    fn findings_are_truncated_at_the_cap() {
        let mut entries = Vec::new();
        for n in 0..(MAX_FINDINGS + 50) {
            entries.push(format!(
                r#"{{"severity":"Info","path":"f{n}.rs","line_start":1,"line_end":1,"title":"t{n}","rationale":"r"}}"#
            ));
        }
        let json = format!("[{}]", entries.join(","));

        let findings = parse_findings(&json).expect("should parse");
        assert_eq!(findings.len(), MAX_FINDINGS);
    }

    #[test]
    fn ids_are_stable_across_reparses() {
        let json = r#"[
            {"severity":"Critical","path":"src/main.rs","line_start":10,"line_end":15,"title":"a","rationale":"r"},
            {"severity":"Info","path":"src/lib.rs","line_start":1,"line_end":2,"title":"b","rationale":"r"}
        ]"#;

        let first = parse_findings(json).expect("parse 1");
        let second = parse_findings(json).expect("parse 2");

        let ids_first: Vec<&str> = first.iter().map(|f| f.id.as_str()).collect();
        let ids_second: Vec<&str> = second.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids_first, ids_second);

        // Id shape: f{index}-{8 hex}.
        for (i, finding) in first.iter().enumerate() {
            let (prefix, hash) = finding.id.split_once('-').expect("id has a dash");
            assert_eq!(prefix, format!("f{i}"));
            assert_eq!(hash.len(), 8);
            assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn id_hash_tracks_content() {
        // Same content in two entries but different paths must hash differently.
        let json = r#"[
            {"path":"a.rs","line_start":1,"line_end":1,"title":"same","rationale":"r"},
            {"path":"b.rs","line_start":1,"line_end":1,"title":"same","rationale":"r"}
        ]"#;
        let findings = parse_findings(json).expect("should parse");
        let hash_a = findings[0].id.split_once('-').expect("dash").1;
        let hash_b = findings[1].id.split_once('-').expect("dash").1;
        assert_ne!(hash_a, hash_b);
    }
}
