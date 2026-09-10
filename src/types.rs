use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub id: Option<Value>,
    pub method: Option<String>,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: Value,
}

/// Persisted session→conversation mapping stored in ~/.openab/agy-gated-acp/sessions.json
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionStore {
    pub sessions: HashMap<String, StoredSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub conversation_id: Option<String>,
    /// Last stream-json step index observed for this conversation.
    #[serde(default)]
    pub last_step_idx: i64,
    /// Selected model ID for this session.
    #[serde(default)]
    pub model_id: Option<String>,
    /// Unix milliseconds when this entry was last written. Milliseconds, not
    /// seconds, because this is the sole pruning key: several sessions written
    /// in one second would tie and be evicted in `HashMap` order. Entries
    /// written before this field existed default to 0 and are pruned first.
    #[serde(default)]
    pub updated_at: u64,
}

/// One row of `agy models` output: the id passed to `--model`, and the human
/// label shown beside it. They are not interchangeable — agy accepts only the id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgyModel {
    pub id: String,
    pub label: String,
}

pub struct Session {
    pub conversation_id: Option<String>,
    /// Last stream-json step index observed for this conversation.
    pub last_step_idx: i64,
    /// Selected model ID for this session.
    pub model_id: Option<String>,
    /// Monotonic tick of the most recent handler that touched this session.
    /// Used to evict the least-recently-used session instead of an arbitrary one.
    pub last_used: u64,
}

/// The state directory, moving a pre-rename `sessions.json` across once.
///
/// New-wins, old-moves, neither-creates-nothing. Silent best-effort, like the
/// persistence that reads the result: a failed move leaves the old file and the
/// adapter starts with fresh state.
pub(crate) fn migrated_state_dir(home: &Path) -> std::path::PathBuf {
    let state_dir = home.join(".openab/agy-gated-acp");
    let new_file = state_dir.join("sessions.json");
    if new_file.exists() {
        return state_dir;
    }
    let old_file = home.join(".openab/agy-acp/sessions.json");
    if !old_file.exists() {
        return state_dir;
    }
    if std::fs::create_dir_all(&state_dir).is_err() {
        return state_dir;
    }
    if std::fs::rename(&old_file, &new_file).is_ok() {
        eprintln!("agy-gated-acp: migrated sessions from ~/.openab/agy-acp/sessions.json");
    }
    state_dir
}
