use rusqlite::Connection;
use serde_json::Value;
use std::path::Path;

use crate::adapter::filter_narration;
use crate::protobuf::{
    extract_text_from_step_payload, extract_tool_update_from_step_payload,
    extract_user_text_from_step_payload, is_tool_step_type, message_chunk_update,
};

/// `after_step_idx` is an exclusive cursor over every `steps.idx` row; an
/// incremental caller must advance it to the largest returned row index.
pub fn read_rows_from_db(
    conversations_dir: &Path,
    conversation_id: &str,
    after_step_idx: i64,
) -> Option<Vec<(i64, i64, Vec<u8>)>> {
    let db_path = conversations_dir.join(format!("{}.db", conversation_id));
    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;

    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='steps'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !table_exists {
        eprintln!(
            "[agy-acp] WARN: steps table not found in {}.db — schema changed?",
            conversation_id
        );
        return None;
    }

    let mut stmt = conn
        .prepare("SELECT idx, step_type, step_payload FROM steps WHERE idx > ?1 ORDER BY idx")
        .ok()?;
    let rows: Vec<(i64, i64, Vec<u8>)> = stmt
        .query_map([after_step_idx], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .ok()?
        .filter_map(|r| r.ok())
        .collect();
    Some(rows)
}

pub fn read_replay_updates_from_db(
    conversations_dir: &Path,
    conversation_id: &str,
    skip_naration: bool,
) -> Option<(Vec<Value>, i64)> {
    let rows = read_rows_from_db(conversations_dir, conversation_id, -1)?;
    let mut max_idx = -1;
    let mut updates = Vec::new();
    let mut pending_agent_parts = Vec::new();

    for (idx, step_type, payload) in &rows {
        max_idx = max_idx.max(*idx);
        if *step_type == 14 {
            flush_agent_message(&mut pending_agent_parts, &mut updates, skip_naration);
            if let Some(text) = extract_user_text_from_step_payload(payload) {
                updates.push(message_chunk_update("user_message_chunk", text));
            }
        } else if *step_type == 15 {
            if let Some(text) = extract_text_from_step_payload(payload) {
                if !text.is_empty() {
                    pending_agent_parts.push(text);
                }
            }
        } else if is_tool_step_type(*step_type) {
            flush_agent_message(&mut pending_agent_parts, &mut updates, skip_naration);
            if let Some(update) = extract_tool_update_from_step_payload(*idx, *step_type, payload) {
                updates.push(update);
            }
        }
    }
    flush_agent_message(&mut pending_agent_parts, &mut updates, skip_naration);

    if updates.is_empty() {
        return None;
    }
    Some((updates, max_idx))
}

fn flush_agent_message(parts: &mut Vec<String>, updates: &mut Vec<Value>, skip_naration: bool) {
    if parts.is_empty() {
        return;
    }
    let text = if skip_naration {
        filter_narration(parts)
    } else {
        Some(parts.join("\n"))
    };
    parts.clear();
    if let Some(text) = text {
        if !text.is_empty() {
            updates.push(message_chunk_update("agent_message_chunk", text));
        }
    }
}
