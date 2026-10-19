//! Anthropic message-content helpers, shared by `super::claude` and `super::cursor`.

use serde_json::Value;

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
