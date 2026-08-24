use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

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

/// Persisted session→conversation mapping stored in ~/.openab/agy-acp/sessions.json
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
    /// Unix seconds when this entry was last written. Entries written before
    /// this field existed default to 0 and are pruned first.
    #[serde(default)]
    pub updated_at: u64,
}

/// Result of a test-only delta read from a conversation DB.
#[cfg(test)]
pub struct ConversationDelta {
    pub text: Option<String>,
    pub max_step_idx: i64,
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
