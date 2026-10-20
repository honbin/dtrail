//! Anthropic message-content helpers, shared by `super::claude` and `super::cursor`.

use serde_json::Value;

use super::model::{Block, ToolResultContent};

/// First non-empty `text` block in a `content` array.
pub(crate) fn first_block_text(content: &Value) -> Option<String> {
    content
        .as_array()?
        .iter()
        .filter_map(|b| {
            (b.get("type").and_then(Value::as_str) == Some("text"))
                .then(|| b.get("text").and_then(Value::as_str))
                .flatten()
        })
        .map(str::trim)
        .find(|t| !t.is_empty())
        .map(str::to_string)
}

pub(crate) fn parse_content(content: &Value) -> Vec<Block> {
    match content {
        Value::String(s) => vec![Block::Text(s.clone())],
        Value::Array(items) => items.iter().filter_map(parse_block).collect(),
        _ => Vec::new(),
    }
}

fn parse_block(block: &Value) -> Option<Block> {
    let block_type = block.get("type").and_then(Value::as_str)?;
    let b = match block_type {
        "text" => Block::Text(
            block
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        "thinking" => Block::Thinking(
            block
                .get("thinking")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        "tool_use" => Block::ToolUse {
            id: block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input: block.get("input").cloned().unwrap_or(Value::Null),
        },
        "tool_result" => Block::ToolResult {
            tool_use_id: block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            content: parse_tool_result_content(block.get("content")),
            is_error: block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        other => Block::Unknown {
            block_type: other.to_string(),
        },
    };
    Some(b)
}

/// `tool_result.content`: a string, or an array of `text`/`image`/`tool_reference`.
/// Non-text elements become placeholders; no binary is retained.
fn parse_tool_result_content(content: Option<&Value>) -> ToolResultContent {
    match content {
        Some(Value::String(s)) => ToolResultContent::Text(s.clone()),
        Some(Value::Array(items)) => {
            let parts = items
                .iter()
                .map(|item| {
                    let t = item
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    match t {
                        "text" => item
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        "image" => "[image]".to_string(),
                        other => format!("[{other}]"),
                    }
                })
                .collect();
            ToolResultContent::Blocks(parts)
        }
        _ => ToolResultContent::Text(String::new()),
    }
}
