//! JSONL stream parser for Claude Code's `stream-json` output format.
//!
//! When Claude Code is invoked with `--output-format stream-json --verbose`,
//! it emits one JSON object per line on stdout. This module parses those
//! lines into [`Event`] values that the UI can render as a live activity
//! timeline.
//!
//! Subagents appear as regular `tool_use` blocks (name `"Agent"` or
//! `"Skill"`) — there are no dedicated subagent events. We detect them
//! by tool name and promote them to [`Event::SubagentSpawn`] /
//! [`Event::SubagentResult`].

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from stream-line parsing.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The line was not valid JSON.
    #[error("invalid JSON in stream line: {0}")]
    InvalidJson(String),
}

// ---------------------------------------------------------------------------
// Event
// ---------------------------------------------------------------------------

/// A parsed event from Claude Code's JSONL stream.
///
/// Each variant carries only the data the UI needs. Large payloads (full
/// file contents in tool inputs) are summarised to keep IPC lightweight.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum Event {
    /// Session initialisation: model, session id, available tools.
    Init {
        /// Model name (e.g. `"claude-opus-4-6"`).
        model: String,
        /// Unique session identifier.
        session_id: String,
        /// Names of tools available to the agent.
        tools: Vec<String>,
    },

    /// Cumulative thinking-token progress.
    Thinking {
        /// Total estimated thinking tokens so far.
        #[ts(type = "number")]
        estimated_tokens: u64,
        /// Tokens added since the last thinking event.
        #[ts(type = "number")]
        delta: u64,
    },

    /// The agent invoked a tool.
    ToolUse {
        /// Tool-use block id (matches a later [`Event::ToolResult`]).
        id: String,
        /// Tool name (e.g. `"Read"`, `"Edit"`, `"Bash"`).
        name: String,
        /// One-line summary of the tool input.
        input_summary: String,
    },

    /// Result of a previously invoked tool.
    ToolResult {
        /// The `tool_use_id` that this result corresponds to.
        tool_use_id: String,
        /// Whether the tool succeeded.
        success: bool,
        /// Short summary of the output.
        summary: String,
    },

    /// A text block from the assistant turn.
    Text {
        /// The text content.
        content: String,
    },

    /// A subagent (Agent/Skill tool) was spawned.
    SubagentSpawn {
        /// Tool-use block id.
        id: String,
        /// The prompt or task description sent to the subagent.
        prompt: String,
    },

    /// A subagent completed.
    SubagentResult {
        /// The `tool_use_id` of the spawn that produced this result.
        tool_use_id: String,
        /// The subagent's output.
        result: String,
    },

    /// Rate-limit status update.
    RateLimit {
        /// Human-readable status string.
        status: String,
    },

    /// The agent run completed (final event).
    Complete {
        /// Wall-clock duration in milliseconds.
        #[ts(type = "number")]
        duration_ms: u64,
        /// Total cost in USD.
        cost_usd: f64,
        /// Number of conversation turns.
        #[ts(type = "number")]
        num_turns: u64,
        /// Total output tokens generated.
        #[ts(type = "number")]
        output_tokens: u64,
        /// The final result text.
        result_text: String,
    },

    /// An error occurred during parsing or in the agent.
    Error {
        /// Human-readable error message.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Raw JSON helpers (internal)
// ---------------------------------------------------------------------------

/// Parse a single JSONL line into an [`Event`].
///
/// Returns `None` for lines that are empty, whitespace-only, or represent
/// event types we intentionally skip (e.g. `system::hook_started`).
pub fn parse_stream_line(line: &str) -> Option<Event> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let raw: serde_json::Value = serde_json::from_str(trimmed).ok()?;

    let event_type = raw.get("type")?.as_str()?;

    match event_type {
        "system" => parse_system_event(&raw),
        "assistant" => parse_assistant_event(&raw),
        "user" => parse_user_event(&raw),
        "result" => parse_result_event(&raw),
        _ => None,
    }
}

/// Parse a `"type": "system"` event.
fn parse_system_event(raw: &serde_json::Value) -> Option<Event> {
    let subtype = raw.get("subtype")?.as_str()?;

    match subtype {
        "init" => {
            let model = raw
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let session_id = raw
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tools = raw
                .get("tools")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| {
                            // Tools may be strings or objects with a "name" field.
                            t.as_str().map(String::from).or_else(|| {
                                t.get("name").and_then(|n| n.as_str()).map(String::from)
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Event::Init {
                model,
                session_id,
                tools,
            })
        }
        "thinking_tokens" => {
            let estimated_tokens = raw
                .get("cumulative_thinking_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let delta = raw
                .get("thinking_delta")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            Some(Event::Thinking {
                estimated_tokens,
                delta,
            })
        }
        // Hook lifecycle events we skip.
        "hook_started" | "hook_response" => None,
        _ => None,
    }
}

/// Parse a `"type": "assistant"` event, extracting tool_use and text blocks.
fn parse_assistant_event(raw: &serde_json::Value) -> Option<Event> {
    let content = raw.get("content")?.as_array()?;

    // We return the first actionable block. In practice Claude Code sends
    // one meaningful block per line, but we handle arrays gracefully.
    for block in content {
        let block_type = block.get("type").and_then(|v| v.as_str())?;

        match block_type {
            "tool_use" => {
                let id = block.get("id").and_then(|v| v.as_str())?.to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let input = block
                    .get("input")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

                // Detect subagent tools.
                if name == "Agent" || name == "Skill" || name == "Task" {
                    let prompt = extract_subagent_prompt(&input);
                    return Some(Event::SubagentSpawn { id, prompt });
                }

                let input_summary = summarize_tool_input(&name, &input);
                return Some(Event::ToolUse {
                    id,
                    name,
                    input_summary,
                });
            }
            "text" => {
                let text = block
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !text.is_empty() {
                    return Some(Event::Text { content: text });
                }
            }
            "thinking" => {
                // Thinking blocks are handled via system::thinking_tokens.
                continue;
            }
            _ => continue,
        }
    }

    None
}

/// Parse a `"type": "user"` event (tool results).
fn parse_user_event(raw: &serde_json::Value) -> Option<Event> {
    let content = raw.get("content")?.as_array()?;

    for block in content {
        let block_type = block.get("type").and_then(|v| v.as_str())?;

        if block_type == "tool_result" {
            let tool_use_id = block
                .get("tool_use_id")
                .and_then(|v| v.as_str())?
                .to_string();
            let is_error = block
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // Check if this is a subagent result by looking at the content.
            let result_text = extract_tool_result_text(block);
            let summary = truncate_summary(&result_text, 200);

            // We cannot reliably distinguish subagent results from regular
            // tool results here without tracking state. The frontend uses
            // the tool_use_id to match against SubagentSpawn events.
            // For now, emit as ToolResult; the frontend promotes matches.

            return Some(Event::ToolResult {
                tool_use_id,
                success: !is_error,
                summary,
            });
        }
    }

    None
}

/// Parse a `"type": "result"` event (final summary).
fn parse_result_event(raw: &serde_json::Value) -> Option<Event> {
    let duration_ms = raw.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
    let cost_usd = raw
        .get("total_cost_usd")
        .or_else(|| raw.get("total_cost"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let num_turns = raw.get("num_turns").and_then(|v| v.as_u64()).unwrap_or(0);
    let output_tokens = raw
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let result_text = raw
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(Event::Complete {
        duration_ms,
        cost_usd,
        num_turns,
        output_tokens,
        result_text,
    })
}

// ---------------------------------------------------------------------------
// Summarization helpers
// ---------------------------------------------------------------------------

/// Summarize tool input to a short, human-readable string.
///
/// Avoids sending full file contents across IPC; the summary captures the
/// intent (which file, which command) without the payload.
fn summarize_tool_input(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Read" | "Edit" | "Write" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)");
            path.to_string()
        }
        "Bash" => {
            let cmd = input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)");
            truncate_summary(cmd, 120)
        }
        _ => {
            // Generic: show the first string value we find.
            if let Some(obj) = input.as_object() {
                for (_, val) in obj {
                    if let Some(s) = val.as_str() {
                        return truncate_summary(s, 80);
                    }
                }
            }
            String::new()
        }
    }
}

/// Extract the prompt/task from a subagent tool input.
fn extract_subagent_prompt(input: &serde_json::Value) -> String {
    // Subagent tools typically have a "prompt" or "task" or "description" field.
    for key in &["prompt", "task", "description", "query"] {
        if let Some(val) = input.get(*key).and_then(|v| v.as_str()) {
            return truncate_summary(val, 200);
        }
    }
    // Fall back to the first string value.
    if let Some(obj) = input.as_object() {
        for (_, val) in obj {
            if let Some(s) = val.as_str() {
                return truncate_summary(s, 200);
            }
        }
    }
    "(unknown task)".to_string()
}

/// Extract readable text from a tool_result block.
fn extract_tool_result_text(block: &serde_json::Value) -> String {
    // The content may be a string or an array of content blocks.
    if let Some(content) = block.get("content") {
        if let Some(s) = content.as_str() {
            return s.to_string();
        }
        if let Some(arr) = content.as_array() {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                }
            }
            if !parts.is_empty() {
                return parts.join("\n");
            }
        }
    }
    // Fall back to output field.
    if let Some(output) = block.get("output").and_then(|v| v.as_str()) {
        return output.to_string();
    }
    String::new()
}

/// Truncate a string to `max_len` characters, appending an ellipsis if cut.
fn truncate_summary(s: &str, max_len: usize) -> String {
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.len() <= max_len {
        first_line.to_string()
    } else {
        let mut truncated = first_line[..max_len].to_string();
        truncated.push_str("...");
        truncated
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_init_event() {
        let line = r#"{"type":"system","subtype":"init","model":"claude-opus-4-6","session_id":"abc-123","tools":["Read","Edit","Bash"]}"#;
        let event = parse_stream_line(line).expect("should parse init");
        match event {
            Event::Init {
                model,
                session_id,
                tools,
            } => {
                assert_eq!(model, "claude-opus-4-6");
                assert_eq!(session_id, "abc-123");
                assert_eq!(tools, vec!["Read", "Edit", "Bash"]);
            }
            other => panic!("expected Init, got {other:?}"),
        }
    }

    #[test]
    fn parse_thinking_event() {
        let line = r#"{"type":"system","subtype":"thinking_tokens","cumulative_thinking_tokens":42,"thinking_delta":10}"#;
        let event = parse_stream_line(line).expect("should parse thinking");
        match event {
            Event::Thinking {
                estimated_tokens,
                delta,
            } => {
                assert_eq!(estimated_tokens, 42);
                assert_eq!(delta, 10);
            }
            other => panic!("expected Thinking, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_use_read() {
        let line = r#"{"type":"assistant","content":[{"type":"tool_use","id":"tu-1","name":"Read","input":{"file_path":"src/main.rs"}}]}"#;
        let event = parse_stream_line(line).expect("should parse tool_use");
        match event {
            Event::ToolUse {
                id,
                name,
                input_summary,
            } => {
                assert_eq!(id, "tu-1");
                assert_eq!(name, "Read");
                assert_eq!(input_summary, "src/main.rs");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_use_bash() {
        let line = r#"{"type":"assistant","content":[{"type":"tool_use","id":"tu-2","name":"Bash","input":{"command":"cargo test"}}]}"#;
        let event = parse_stream_line(line).expect("should parse bash tool_use");
        match event {
            Event::ToolUse {
                id,
                name,
                input_summary,
            } => {
                assert_eq!(id, "tu-2");
                assert_eq!(name, "Bash");
                assert_eq!(input_summary, "cargo test");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_subagent_spawn() {
        let line = r#"{"type":"assistant","content":[{"type":"tool_use","id":"tu-3","name":"Agent","input":{"prompt":"review the changes"}}]}"#;
        let event = parse_stream_line(line).expect("should parse subagent");
        match event {
            Event::SubagentSpawn { id, prompt } => {
                assert_eq!(id, "tu-3");
                assert_eq!(prompt, "review the changes");
            }
            other => panic!("expected SubagentSpawn, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_result_success() {
        let line = r#"{"type":"user","content":[{"type":"tool_result","tool_use_id":"tu-1","content":"file contents here","is_error":false}]}"#;
        let event = parse_stream_line(line).expect("should parse tool_result");
        match event {
            Event::ToolResult {
                tool_use_id,
                success,
                summary,
            } => {
                assert_eq!(tool_use_id, "tu-1");
                assert!(success);
                assert_eq!(summary, "file contents here");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_result_error() {
        let line = r#"{"type":"user","content":[{"type":"tool_result","tool_use_id":"tu-2","content":"command failed","is_error":true}]}"#;
        let event = parse_stream_line(line).expect("should parse error tool_result");
        match event {
            Event::ToolResult {
                tool_use_id,
                success,
                ..
            } => {
                assert_eq!(tool_use_id, "tu-2");
                assert!(!success);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_text_event() {
        let line =
            r#"{"type":"assistant","content":[{"type":"text","text":"I will fix the bug."}]}"#;
        let event = parse_stream_line(line).expect("should parse text");
        match event {
            Event::Text { content } => {
                assert_eq!(content, "I will fix the bug.");
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn parse_result_event() {
        let line = r#"{"type":"result","duration_ms":15800,"total_cost_usd":0.38,"num_turns":5,"usage":{"output_tokens":528},"result":"Done."}"#;
        let event = parse_stream_line(line).expect("should parse result");
        match event {
            Event::Complete {
                duration_ms,
                cost_usd,
                num_turns,
                output_tokens,
                result_text,
            } => {
                assert_eq!(duration_ms, 15800);
                assert!((cost_usd - 0.38).abs() < f64::EPSILON);
                assert_eq!(num_turns, 5);
                assert_eq!(output_tokens, 528);
                assert_eq!(result_text, "Done.");
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_line_returns_none() {
        assert!(parse_stream_line("").is_none());
        assert!(parse_stream_line("   ").is_none());
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        assert!(parse_stream_line("not json").is_none());
    }

    #[test]
    fn parse_hook_events_are_skipped() {
        let line = r#"{"type":"system","subtype":"hook_started"}"#;
        assert!(parse_stream_line(line).is_none());

        let line = r#"{"type":"system","subtype":"hook_response"}"#;
        assert!(parse_stream_line(line).is_none());
    }

    #[test]
    fn truncate_summary_long_input() {
        let long = "a".repeat(300);
        let result = truncate_summary(&long, 100);
        assert_eq!(result.len(), 103); // 100 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_summary_short_input() {
        let short = "hello";
        let result = truncate_summary(short, 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn summarize_edit_tool() {
        let input = serde_json::json!({"file_path": "src/lib.rs", "old_string": "foo", "new_string": "bar"});
        let summary = summarize_tool_input("Edit", &input);
        assert_eq!(summary, "src/lib.rs");
    }

    #[test]
    fn summarize_write_tool() {
        let input = serde_json::json!({"file_path": "new_file.rs", "content": "fn main() {}"});
        let summary = summarize_tool_input("Write", &input);
        assert_eq!(summary, "new_file.rs");
    }

    #[test]
    fn event_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Event>();
    }

    #[test]
    fn event_round_trips_through_serde() {
        let event = Event::ToolUse {
            id: "tu-1".into(),
            name: "Read".into(),
            input_summary: "src/main.rs".into(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: Event = serde_json::from_str(&json).expect("deserialize");
        match back {
            Event::ToolUse { id, name, .. } => {
                assert_eq!(id, "tu-1");
                assert_eq!(name, "Read");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_rate_limit_event() {
        // Rate limit events come as a distinct type.
        let line = r#"{"type":"rate_limit_event","status":"rate limited"}"#;
        // This is not a system/assistant/user/result type, so it returns None.
        // We handle it if Claude Code adds it as a dedicated type.
        assert!(parse_stream_line(line).is_none());
    }
}
