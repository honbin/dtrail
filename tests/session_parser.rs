//! Parser tests: message extraction, tool_use/tool_result linking, unknown
//! events, and tolerance of incomplete records.

use ccscope::session::claude;
use ccscope::session::model::{Block, RecordKind, ToolResultContent};

fn fixture() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/minimal-session.jsonl"
    ))
    .expect("fixture readable")
}

#[test]
fn skips_broken_line_but_keeps_rest() {
    let (sess, stats) = claude::parse(&fixture());
    // The broken last line is skipped; the rest still parse.
    assert_eq!(stats.skipped_lines, 1);
    assert!(stats.total_lines >= 10);
    assert_eq!(sess.session_id.as_deref(), Some("sess-abc"));
}

#[test]
fn extracts_user_string_and_assistant_blocks() {
    let (sess, _) = claude::parse(&fixture());

    // The first user message has string content.
    let first = &sess.records[0];
    match &first.kind {
        RecordKind::User(blocks) => match &blocks[0] {
            Block::Text(t) => assert_eq!(t, "hello"),
            other => panic!("expected text, got {other:?}"),
        },
        other => panic!("expected user, got {other:?}"),
    }

    // thinking and tool_use are split across separate records.
    let thinking = sess.records.iter().find_map(|r| match &r.kind {
        RecordKind::Assistant(b) => b.iter().find_map(|x| match x {
            Block::Thinking(t) => Some(t.clone()),
            _ => None,
        }),
        _ => None,
    });
    assert_eq!(thinking.as_deref(), Some("let me think"));
}

#[test]
fn links_tool_use_and_result_by_id() {
    let (sess, _) = claude::parse(&fixture());
    let index = sess.tool_use_index();

    // toolu_1's tool_use is indexed and its name is recoverable.
    let (ri, bi) = index.get("toolu_1").copied().expect("toolu_1 indexed");
    match &sess.records[ri].kind {
        RecordKind::Assistant(blocks) => match &blocks[bi] {
            Block::ToolUse { name, .. } => assert_eq!(name, "Bash"),
            other => panic!("expected tool_use, got {other:?}"),
        },
        other => panic!("expected assistant, got {other:?}"),
    }

    // The matching tool_result is reachable by id.
    let has_matching_result = sess.records.iter().any(|r| match &r.kind {
        RecordKind::User(b) => b.iter().any(|x| {
            matches!(
                x,
                Block::ToolResult { tool_use_id, .. } if index.contains_key(tool_use_id)
            )
        }),
        _ => false,
    });
    assert!(has_matching_result);
}

#[test]
fn orphan_tool_result_is_not_force_linked() {
    let (sess, _) = claude::parse(&fixture());
    let index = sess.tool_use_index();
    // toolu_err has no matching tool_use (never linked by guesswork).
    assert!(!index.contains_key("toolu_err"));
}

#[test]
fn tool_result_array_content_flattens_with_placeholders() {
    let (sess, _) = claude::parse(&fixture());
    let content = sess.records.iter().find_map(|r| match &r.kind {
        RecordKind::User(b) => b.iter().find_map(|x| match x {
            Block::ToolResult {
                is_error: true,
                content,
                ..
            } => Some(content.clone()),
            _ => None,
        }),
        _ => None,
    });
    match content.expect("error tool_result present") {
        ToolResultContent::Blocks(parts) => {
            assert_eq!(parts, vec!["boom".to_string(), "[image]".to_string()]);
        }
        other => panic!("expected flattened blocks, got {other:?}"),
    }
}

#[test]
fn unknown_block_type_is_preserved_not_dropped() {
    let (sess, _) = claude::parse(&fixture());
    let unknown = sess.records.iter().find_map(|r| match &r.kind {
        RecordKind::Assistant(b) => b.iter().find_map(|x| match x {
            Block::Unknown { block_type } => Some(block_type.clone()),
            _ => None,
        }),
        _ => None,
    });
    assert_eq!(unknown.as_deref(), Some("future_block"));
}

#[test]
fn unknown_record_type_becomes_meta() {
    let (sess, _) = claude::parse(&fixture());
    let has_meta = sess.records.iter().any(|r| {
        matches!(
            &r.kind,
            RecordKind::Meta { record_type } if record_type == "file-history-snapshot"
        )
    });
    assert!(has_meta);
}
