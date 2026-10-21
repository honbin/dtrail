//! Message-content helpers: Anthropic content blocks, shared by `super::claude`
//! and `super::cursor`, and JSON field access shared by every agent.

use serde_json::Value;

use super::model::{Block, ToolResultContent};

/// First non-empty `text` block in a `content` array.
pub(crate) fn first_block_text(content: &Value) -> Option<String> {
    content
        .as_array()?
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .map(str::trim)
        .find(|t| !t.is_empty())
        .map(str::to_string)
}

/// A string field, or empty when it is missing or not a string.
pub(crate) fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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
        "text" => Block::Text(str_field(block, "text")),
        "thinking" => Block::Thinking(str_field(block, "thinking")),
        "tool_use" => Block::ToolUse {
            id: str_field(block, "id"),
            name: str_field(block, "name"),
            input: block.get("input").cloned().unwrap_or(Value::Null),
        },
        "tool_result" => Block::ToolResult {
            tool_use_id: str_field(block, "tool_use_id"),
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
                        "text" => str_field(item, "text"),
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
