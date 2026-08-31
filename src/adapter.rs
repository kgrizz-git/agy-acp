use fs2::FileExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use uuid::Uuid;

use crate::db::read_replay_updates_from_db;
use crate::permission::{PermissionBridge, SOCKET_ENV};
use crate::proc::LiveChildren;
use crate::streaming::StreamProcessor;
use crate::types::*;

/// Reads a single newline-terminated frame from `reader` into `buf`.
///
/// Returns `Ok(true)` when a frame (including its trailing `\n`) was read,
/// `Ok(false)` at EOF with nothing more to read, or `Err` on an underlying I/O
/// error. `buf` is cleared first so each call yields exactly one frame. Split
/// out from the stdout drain task so the byte-oriented loop can be unit-tested
/// without spawning a real `agy` process.
pub(crate) async fn read_until_newline<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> std::io::Result<bool> {
    buf.clear();
    let n = reader.read_until(b'\n', buf).await?;
    Ok(n != 0)
}

/// Processes one stream-json frame and publishes its `session/update`
/// notifications on `notify_tx`.
///
/// Split out of the stdout drain task so it can be unit-tested without spawning
/// a real `agy` process. `processor` is the per-turn `StreamProcessor` and is
/// mutated in place, since stream-json is a sequence of events that carry state
/// (conversation binding, step indices, de-duplicated text).
///
/// Only ever sends `Some(..)`. `None` is the main loop's "pending prompt
/// finished" sentinel, so sending it here would corrupt `pending_prompts` and
/// make the process exit early.
pub(crate) fn publish_stream_notifications(
    processor: &mut StreamProcessor,
    notify_tx: &tokio::sync::mpsc::UnboundedSender<Option<String>>,
    line: &str,
    session_id: &str,
) {
    for notification in processor.process_line(line, session_id) {
        let _ = notify_tx.send(Some(notification));
    }
}

pub struct Adapter {
    pub sessions: HashMap<String, Session>,
    pub working_dir: String,
    /// Only the session/load replay path reads this. Live streaming comes from
    /// agy's stream-json output; agy still writes these SQLite conversation DBs,
    /// and they remain the only place a past turn's history exists.
    pub conversations_dir: PathBuf,
    pub state_file: PathBuf,
    pub available_models: Vec<AgyModel>,
    pub skip_naration: bool,
    /// Present only when `--permission-prompts` is on. Its presence is what makes
    /// the adapter hand agy's tool gating over to the ACP client.
    pub permission_bridge: Option<PermissionBridge>,
    /// Private workspace root supplying the `PreToolUse` hook to agy.
    pub hook_root_dir: Option<PathBuf>,
    /// Monotonic recency counter handed out by `touch_session`. A counter rather
    /// than wall-clock time so eviction order survives a clock that jumps.
    pub session_tick: u64,
    /// The agy children running right now, so that shutdown can kill the same
    /// trees a cancel would.
    pub live_children: LiveChildren,
}

/// Print-mode timeout used when permission prompts are on. Must outlast the
/// bridge's own wait so an unanswered prompt ends as a deny, not a failed turn.
const PERMISSION_PRINT_TIMEOUT: &str = "60m";
/// Hard cap on persisted sessions in sessions.json. The file is fully rewritten
/// on every turn, so keeping it bounded is what stops it growing without limit.
const MAX_PERSISTED_SESSIONS: usize = 256;

impl Adapter {
    pub const MODEL_CONFIG_ID: &'static str = "model";

    /// Routes agy's tool permission checks through the ACP client.
    pub fn enable_permission_bridge(&mut self, bridge: &PermissionBridge, hook_root: &Path) {
        self.permission_bridge = Some(bridge.clone());
        self.hook_root_dir = Some(hook_root.to_path_buf());
    }

    pub fn new() -> Self {
        Self::new_with_skip_naration(false)
    }

    /// A handle on the live agy children, for whoever owns shutdown.
    pub fn live_children(&self) -> LiveChildren {
        self.live_children.clone()
    }

    pub fn new_with_skip_naration(skip_naration: bool) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        Self::new_with_home(
            PathBuf::from(home),
            Self::fetch_available_models(),
            skip_naration,
        )
    }

    /// Constructs an adapter for unit tests without consulting HOME or running
    /// `agy models`. Each invocation uses a collision-resistant private scratch
    /// root so a test that persists a session cannot observe another test's
    /// state, including after a prior test process has exited.
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        let home = std::env::temp_dir().join(format!("agy-acp-test-{}", Uuid::new_v4()));
        Self::new_with_home(home, Vec::new(), false)
    }

    fn new_with_home(home: PathBuf, available_models: Vec<AgyModel>, skip_naration: bool) -> Self {
        let state_dir = home.join(".openab/agy-acp");
        Self {
            sessions: HashMap::new(),
            working_dir: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/tmp".to_string()),
            conversations_dir: home.join(".gemini/antigravity-cli/conversations"),
            state_file: state_dir.join("sessions.json"),
            available_models,
            skip_naration,
            permission_bridge: None,
            hook_root_dir: None,
            session_tick: 0,
            live_children: LiveChildren::default(),
        }
    }

    /// Run `agy models` and parse the output into id/label pairs.
    fn fetch_available_models() -> Vec<AgyModel> {
        std::process::Command::new("agy")
            .arg("models")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| Self::parse_models_output(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default()
    }

    /// `agy models` prints `id<TAB>Human Label` per line, and its "Fetching
    /// available models..." banner goes to stderr, so stdout is data only. Taking
    /// the whole line as the id is the bug this splits: agy rejects
    /// `gemini-3.7-flash-high\tGemini 3.7 Flash (High)` as a model name.
    pub(crate) fn parse_models_output(stdout: &str) -> Vec<AgyModel> {
        stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|line| match line.split_once('\t') {
                Some((id, label)) => AgyModel {
                    id: id.trim().to_string(),
                    label: label.trim().to_string(),
                },
                // An agy that stops printing the label column still works; the id
                // is the part we cannot do without.
                None => AgyModel {
                    id: line.to_string(),
                    label: line.to_string(),
                },
            })
            .filter(|m| !m.id.is_empty())
            .collect()
    }

    /// Strips a label that a client echoed back joined to its id. Sessions
    /// persisted before the parser was fixed hold exactly that, so this also
    /// repairs them on restore.
    pub(crate) fn sanitize_model_id(raw: &str) -> String {
        match raw.split_once('\t') {
            Some((id, _label)) => id.trim().to_string(),
            None => raw.trim().to_string(),
        }
    }

    /// Sanitizes an id from a client and checks it against what agy offers.
    /// Returns `None` for an id agy would reject. When the model list is empty —
    /// agy missing, or not yet queried — there is nothing to check against, so
    /// the id is taken at face value rather than refusing every model.
    fn normalize_model_id(available: &[AgyModel], raw: &str) -> Option<String> {
        let id = Self::sanitize_model_id(raw);
        if id.is_empty() {
            return None;
        }
        if available.is_empty() || available.iter().any(|m| m.id == id) {
            Some(id)
        } else {
            None
        }
    }

    /// Build the ACP `models` JSON for a session, given its current model_id.
    pub fn session_models_json(&mut self, model_id: Option<&str>) -> Value {
        if self.available_models.is_empty() {
            self.available_models = Self::fetch_available_models();
        }
        let current = model_id
            .map(Self::sanitize_model_id)
            .or_else(|| self.available_models.first().map(|m| m.id.clone()))
            .unwrap_or_default();
        let available: Vec<Value> = self
            .available_models
            .iter()
            .map(|model| {
                json!({
                    "modelId": model.id,
                    "name": model.label,
                })
            })
            .collect();
        json!({
            "currentModelId": current,
            "availableModels": available,
        })
    }

    /// Build the ACP session config option that Zed uses for its model selector.
    pub fn session_config_options_json(&mut self, model_id: Option<&str>) -> Value {
        if self.available_models.is_empty() {
            self.available_models = Self::fetch_available_models();
        }
        let current = model_id
            .map(Self::sanitize_model_id)
            .or_else(|| self.available_models.first().map(|m| m.id.clone()))
            .unwrap_or_default();
        let options: Vec<Value> = self
            .available_models
            .iter()
            .map(|model| {
                json!({
                    "value": model.id,
                    "name": model.label,
                })
            })
            .collect();
        json!([{
            "id": Self::MODEL_CONFIG_ID,
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": current,
            "options": options,
        }])
    }

    pub fn session_config_result_json(
        &mut self,
        session_id: &str,
        model_id: Option<&str>,
    ) -> Value {
        json!({
            "sessionId": session_id,
            "models": self.session_models_json(model_id),
            "configOptions": self.session_config_options_json(model_id),
        })
    }

    /// Acquire exclusive lock on a dedicated lock file for read-write mutual exclusion.
    fn lock_state_file(&self) -> Option<fs::File> {
        if let Some(parent) = self.state_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let lock_path = self.state_file.with_extension("lock");
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .ok()?;
        lock_file.lock_exclusive().ok()?;
        Some(lock_file)
    }

    /// Load persisted session store (caller must hold lock).
    fn load_store_inner(&self) -> SessionStore {
        let Some(file) = fs::File::open(&self.state_file).ok() else {
            return SessionStore::default();
        };
        serde_json::from_reader(&file).unwrap_or_default()
    }

    /// Load persisted session store with lock.
    pub fn load_store(&self) -> SessionStore {
        let _lock = self.lock_state_file();
        self.load_store_inner()
    }

    /// Try to restore conversation_id, last_step_idx, and model_id from persisted state.
    pub fn restore_session(&self, session_id: &str) -> Option<(String, i64, Option<String>)> {
        let store = self.load_store();
        store.sessions.get(session_id).and_then(|s| {
            s.conversation_id
                .clone()
                .map(|cid| (cid, s.last_step_idx, s.model_id.clone()))
        })
    }

    /// Persist a session binding (read-modify-write under single lock).
    pub fn persist_session(
        &self,
        session_id: &str,
        conversation_id: Option<&str>,
        last_step_idx: i64,
        model_id: Option<&str>,
    ) {
        let Some(_lock) = self.lock_state_file() else {
            return;
        };
        let updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut store = self.load_store_inner();
        store.sessions.insert(
            session_id.to_string(),
            StoredSession {
                conversation_id: conversation_id.map(String::from),
                last_step_idx,
                model_id: model_id.map(String::from),
                updated_at,
            },
        );
        // The just-written entry must survive pruning even if it is old; its
        // caller still needs it on the next turn.
        Self::prune_store(&mut store, session_id);
        let tmp = self.state_file.with_extension("tmp");
        if let Ok(file) = fs::File::create(&tmp) {
            if serde_json::to_writer_pretty(&file, &store).is_ok() {
                let _ = fs::rename(&tmp, &self.state_file);
            }
        }
    }

    /// Drop persisted sessions past the cap. Unbindable entries (no
    /// conversation_id, so they can never be resumed) are removed before bound
    /// ones, and within each group the oldest `updated_at` goes first. The entry
    /// just written is never removed, even if the order would pick it.
    fn prune_store(store: &mut SessionStore, just_written: &str) {
        while store.sessions.len() > MAX_PERSISTED_SESSIONS {
            let victim = store
                .sessions
                .iter()
                .filter(|(id, _s)| *id != just_written)
                .filter(|(_, s)| s.conversation_id.is_none())
                .min_by_key(|(_, s)| s.updated_at)
                .or_else(|| {
                    store
                        .sessions
                        .iter()
                        .filter(|(id, _s)| *id != just_written)
                        .min_by_key(|(_, s)| s.updated_at)
                })
                .map(|(id, _)| id.clone());
            match victim {
                Some(id) => {
                    store.sessions.remove(&id);
                }
                None => break,
            }
        }
    }

    /// Filter out leading narration ("I will ...", "I'll ...") from response parts.
    #[cfg(test)]
    pub fn filter_narration(parts: &[String]) -> Option<String> {
        filter_narration(parts)
    }

    /// A part is considered narration if every non-empty line starts with "I will" or "I'll".
    #[cfg(test)]
    pub fn is_narration(text: &str) -> bool {
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            return false;
        }
        lines.iter().all(|l| {
            let line = l.trim_start();
            line.starts_with("I will") || line.starts_with("I'll") || line.starts_with("I’ll")
        })
    }

    /// Advance the recency clock and stamp the named session, if present, with
    /// the new tick. Callers use this before serving any request so the session
    /// they are about to use is never the one evicted.
    fn touch_session(&mut self, session_id: &str) {
        self.session_tick += 1;
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.last_used = self.session_tick;
        }
    }

    pub(crate) fn evict_if_needed(&mut self) {
        const MAX_SESSIONS: usize = 64;
        while self.sessions.len() >= MAX_SESSIONS {
            // Drop the entry least recently used, not whichever HashMap key the
            // iterator happens to land on first — that could evict a live turn.
            let victim = self
                .sessions
                .iter()
                .min_by_key(|(_, s)| s.last_used)
                .map(|(id, _)| id.clone());
            match victim {
                Some(key) => {
                    self.sessions.remove(&key);
                }
                None => break,
            }
        }
    }

    /// Build a `Session` stamped with the current recency tick. Every insert goes
    /// through here so no session can be created without one.
    fn make_session(
        &mut self,
        conversation_id: Option<String>,
        last_step_idx: i64,
        model_id: Option<String>,
    ) -> Session {
        self.session_tick += 1;
        Session {
            conversation_id,
            last_step_idx,
            model_id,
            last_used: self.session_tick,
        }
    }

    pub fn restore_session_state(&mut self, session_id: &str) -> bool {
        let Some((conversation_id, last_step_idx, model_id)) = self.restore_session(session_id)
        else {
            return false;
        };
        if !self.sessions.contains_key(session_id) {
            self.evict_if_needed();
        }
        // Built before the insert so the recency tick does not alias the map borrow.
        let session = self.make_session(
            Some(conversation_id),
            last_step_idx,
            model_id.as_deref().map(Self::sanitize_model_id),
        );
        self.sessions.insert(session_id.to_string(), session);
        true
    }

    pub fn handle_initialize(&self, id: Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({
                "protocolVersion": 1,
                "agentInfo": { "name": "agy", "version": env!("CARGO_PKG_VERSION") },
                "agentCapabilities": {
                    "loadSession": true,
                    "sessionCapabilities": { "resume": {} },
                },
                "authMethods": [],
            })),
            error: None,
        }
    }

    pub fn handle_session_new(&mut self, id: Value) -> JsonRpcResponse {
        let session_id = Uuid::new_v4().to_string();
        self.evict_if_needed();
        let session = self.make_session(None, -1, None);
        self.sessions.insert(session_id.clone(), session);
        let result = self.session_config_result_json(&session_id, None);
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn read_replay_updates_from_db_inner(
        &self,
        conversation_id: &str,
    ) -> Option<(Vec<Value>, i64)> {
        read_replay_updates_from_db(&self.conversations_dir, conversation_id, self.skip_naration)
    }

    pub fn handle_session_load(&mut self, id: Value, params: &Value) -> Vec<String> {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if session_id.is_empty() {
            return vec![serde_json::to_string(&JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({"code":-32602,"message":"missing sessionId"})),
            })
            .unwrap()];
        }

        self.touch_session(session_id);
        if !self.sessions.contains_key(session_id) && !self.restore_session_state(session_id) {
            return vec![serde_json::to_string(&JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({
                    "code": -32000,
                    "message": format!("unknown sessionId: {session_id}"),
                })),
            })
            .unwrap()];
        }

        let mut output_lines: Vec<String> = Vec::new();

        // Upstream dropped this when it stopped reading SQLite for streaming. The
        // history still only exists in agy's conversation DB, so loading a thread
        // without it hands the client an empty transcript.
        let replay_conv_id = self
            .sessions
            .get(session_id)
            .and_then(|session| session.conversation_id.clone());
        if let Some(conv_id) = replay_conv_id {
            if let Some((updates, max_step_idx)) = self.read_replay_updates_from_db_inner(&conv_id)
            {
                for update in updates {
                    let notification = serde_json::to_string(&JsonRpcNotification {
                        jsonrpc: "2.0",
                        method: "session/update".to_string(),
                        params: json!({
                            "sessionId": session_id,
                            "update": update,
                        }),
                    })
                    .unwrap();
                    output_lines.push(notification);
                }
                if let Some(session) = self.sessions.get_mut(session_id) {
                    session.last_step_idx = max_step_idx;
                }
                let model_id = self
                    .sessions
                    .get(session_id)
                    .and_then(|s| s.model_id.clone());
                self.persist_session(
                    session_id,
                    Some(conv_id.as_str()),
                    max_step_idx,
                    model_id.as_deref(),
                );
            }
        }

        output_lines.push({
            let model_id = self
                .sessions
                .get(session_id)
                .and_then(|s| s.model_id.clone());
            let result = self.session_config_result_json(session_id, model_id.as_deref());
            serde_json::to_string(&JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(result),
                error: None,
            })
            .unwrap()
        });

        output_lines
    }

    pub fn handle_session_resume(&mut self, id: Value, params: &Value) -> JsonRpcResponse {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if session_id.is_empty() {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({"code":-32602,"message":"missing sessionId"})),
            };
        }

        self.touch_session(session_id);
        if self.sessions.contains_key(session_id) || self.restore_session_state(session_id) {
            let model_id = self
                .sessions
                .get(session_id)
                .and_then(|s| s.model_id.clone());
            let result = self.session_config_result_json(session_id, model_id.as_deref());
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(result),
                error: None,
            };
        }

        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(json!({
                "code": -32000,
                "message": format!("unknown sessionId: {session_id}"),
            })),
        }
    }

    pub fn handle_session_set_model(&mut self, id: Value, params: &Value) -> JsonRpcResponse {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let model_id = params.get("modelId").and_then(|v| v.as_str()).unwrap_or("");

        if session_id.is_empty() || model_id.is_empty() {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({"code":-32602,"message":"missing sessionId or modelId"})),
            };
        }

        self.touch_session(session_id);
        if !self.sessions.contains_key(session_id) {
            let _ = self.restore_session_state(session_id);
        }

        let Some(session) = self.sessions.get_mut(session_id) else {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({
                    "code": -32000,
                    "message": format!("unknown sessionId: {session_id}"),
                })),
            };
        };

        // Checked after the session lookup so an unknown session is reported as
        // such rather than blamed on the model.
        let Some(model_id) = Self::normalize_model_id(&self.available_models, model_id) else {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({
                    "code": -32602,
                    "message": format!("unknown modelId: {model_id}"),
                })),
            };
        };

        session.model_id = Some(model_id.clone());
        let model_id_str = session.model_id.clone();
        let last_step_idx = session.last_step_idx;
        let conv_id = session.conversation_id.clone();

        self.persist_session(
            session_id,
            conv_id.as_deref(),
            last_step_idx,
            model_id_str.as_deref(),
        );

        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({})),
            error: None,
        }
    }

    pub fn handle_session_set_config_option(
        &mut self,
        id: Value,
        params: &Value,
    ) -> JsonRpcResponse {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let config_id = params
            .get("configId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let model_id = params.get("value").and_then(|v| v.as_str()).unwrap_or("");

        if session_id.is_empty() || config_id.is_empty() || model_id.is_empty() {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(
                    json!({"code":-32602,"message":"missing sessionId, configId, or value"}),
                ),
            };
        }

        if config_id != Self::MODEL_CONFIG_ID {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({
                    "code": -32602,
                    "message": format!("unknown configId: {config_id}"),
                })),
            };
        }

        self.touch_session(session_id);
        if !self.sessions.contains_key(session_id) {
            let _ = self.restore_session_state(session_id);
        }

        let Some(session) = self.sessions.get_mut(session_id) else {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({
                    "code": -32000,
                    "message": format!("unknown sessionId: {session_id}"),
                })),
            };
        };

        // Checked after the session lookup so an unknown session is reported as
        // such rather than blamed on the model.
        let Some(model_id) = Self::normalize_model_id(&self.available_models, model_id) else {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({
                    "code": -32602,
                    "message": format!("unknown modelId: {model_id}"),
                })),
            };
        };

        session.model_id = Some(model_id.clone());
        let model_id_str = session.model_id.clone();
        let last_step_idx = session.last_step_idx;
        let conv_id = session.conversation_id.clone();

        self.persist_session(
            session_id,
            conv_id.as_deref(),
            last_step_idx,
            model_id_str.as_deref(),
        );

        let config_options = self.session_config_options_json(model_id_str.as_deref());
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({ "configOptions": config_options })),
            error: None,
        }
    }

    /// `notify_tx` carries `session/update` notifications to the main loop, which
    /// is the only writer of stdout. A second writer would interleave mid-line and
    /// corrupt the JSON-RPC stream, so the stream reader never touches the fd.
    pub async fn handle_session_prompt(
        &mut self,
        id: Value,
        params: &Value,
        cancelled: Arc<AtomicBool>,
        notify_tx: tokio::sync::mpsc::UnboundedSender<Option<String>>,
    ) -> Vec<String> {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        self.touch_session(session_id);
        if !session_id.is_empty() && !self.sessions.contains_key(session_id) {
            let _ = self.restore_session_state(session_id);
        }

        let prompt_text = params
            .get("prompt")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let clean_prompt = prompt_text.trim();

        if let Some(bridge) = self.permission_bridge.clone() {
            bridge.set_workspace_root(&self.working_dir).await;
            bridge.set_active_session(Some(session_id)).await;
        }

        let mut args: Vec<String> = Vec::new();
        args.push("--add-dir".to_string());
        args.push(self.working_dir.clone());
        // agy picks up `.agents/hooks.json` from every workspace root, so adding the
        // adapter's private root is what installs the permission hook for this run
        // and nothing else.
        if let Some(hook_root) = &self.hook_root_dir {
            args.push("--add-dir".to_string());
            args.push(hook_root.display().to_string());
        }
        if let Ok(extra) = std::env::var("AGY_EXTRA_ARGS") {
            args.extend(extra.split_whitespace().map(String::from));
        }
        args.push("--output-format".to_string());
        args.push("stream-json".to_string());
        if let Some(session) = self.sessions.get(session_id) {
            if let Some(conv_id) = &session.conversation_id {
                args.push("--conversation".to_string());
                args.push(conv_id.clone());
            }
            if let Some(model_id) = &session.model_id {
                args.push("--model".to_string());
                args.push(model_id.clone());
            }
        }
        // With the bridge on, agy's own permission checks would auto-deny before the
        // PreToolUse hook's decision could take effect, so they are turned off and
        // the hook becomes the sole gate.
        if self.permission_bridge.is_some() {
            args.push("--dangerously-skip-permissions".to_string());

            // Waiting on a human easily outlasts agy's 5 minute print-mode default,
            // and when that fires agy aborts the whole turn instead of letting the
            // bridge deny cleanly. Give it room, unless the user set their own.
            if !args.iter().any(|arg| arg == "--print-timeout") {
                args.push("--print-timeout".to_string());
                args.push(PERMISSION_PRINT_TIMEOUT.to_string());
            }
        }
        args.push("-p".to_string());
        args.push(clean_prompt.to_string());

        // In its own process group, so that a signal aimed at the adapter's group
        // cannot kill agy before the tree under it can be walked.
        let mut command = crate::proc::command_in_own_group("agy");
        command
            .args(&args)
            .current_dir(&self.working_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(bridge) = &self.permission_bridge {
            command.env(SOCKET_ENV, bridge.socket_path());
        }
        let spawn_result = command.spawn();

        let mut child = match spawn_result {
            Ok(child) => child,
            Err(e) => {
                return vec![serde_json::to_string(&JsonRpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: None,
                    error: Some(json!({"code":-32000,"message":format!("failed to run agy: {e}")})),
                })
                .unwrap()];
            }
        };

        // Registered before anything can fail: an early return from here on must
        // not leave a child nobody knows about. Unregistered below, as soon as
        // the child is reaped.
        let child_guard = self.live_children.register(child.id());

        let stdout = child.stdout.take();
        let skip_naration = self.skip_naration;
        let poll_session_id = session_id.to_string();
        // Set when nothing can read agy's stdout any more. The child would then
        // block on a full pipe and `child.wait()` would never return, so this
        // ends the turn the same way a cancel does -- by killing agy and
        // everything it started.
        let undrainable = Arc::new(AtomicBool::new(false));
        let drain_failed = Arc::clone(&undrainable);
        let stdout_reader = tokio::spawn(async move {
            let mut processor = StreamProcessor::new(skip_naration);
            if let Some(stdout) = stdout {
                let mut reader = BufReader::new(stdout);
                let mut read_error: Option<String> = None;
                let mut buf = Vec::new();
                loop {
                    match read_until_newline(&mut reader, &mut buf).await {
                        Ok(true) => {
                            // from_utf8_lossy, not from_utf8: a malformed byte must
                            // not be able to end this drain, or the pipe stops being
                            // read, fills, and wedges child.wait() forever. At worst
                            // one event fails to parse and is skipped below.
                            let line = String::from_utf8_lossy(&buf)
                                .trim_end_matches(['\n', '\r'])
                                .to_string();
                            publish_stream_notifications(
                                &mut processor,
                                &notify_tx,
                                &line,
                                &poll_session_id,
                            );
                        }
                        Ok(false) => break, // EOF
                        Err(e) => {
                            read_error = Some(e.to_string());
                            // The child may still have bytes queued; drain them so its
                            // stdout pipe never blocks and child.wait() can return.
                            // If even that fails the pipe is unreadable, and waiting
                            // on a child that cannot write is the hang this whole
                            // loop exists to avoid.
                            if tokio::io::copy(&mut reader, &mut tokio::io::sink())
                                .await
                                .is_err()
                            {
                                drain_failed.store(true, Ordering::SeqCst);
                            }
                            break;
                        }
                    }
                }
                if let Some(e) = read_error {
                    eprintln!("agy-acp: error reading agy stdout: {e}");
                }
            }
            processor
        });

        let mut stderr = child.stderr.take();
        let stderr_reader = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut stderr) = stderr.take() {
                let _ = stderr.read_to_end(&mut buf).await;
            }
            buf
        });

        let mut was_cancelled = false;
        let result = tokio::select! {
            result = child.wait() => result,
            _ = async {
                while !cancelled.load(Ordering::SeqCst) && !undrainable.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            } => {
                // An undrainable pipe kills the child too, but it is not a cancel:
                // the turn failed, and reporting it as cancelled would hide that.
                was_cancelled = cancelled.load(Ordering::SeqCst);
                // Before the wait below: the walk this does needs agy alive to
                // find the shell it started, and needs its pid unreaped to name
                // it at all.
                crate::proc::kill_tree(&mut child).await;
                child.wait().await
            }
        };
        // The pid is reaped now, so it may be reused; stop naming it.
        drop(child_guard);
        let processor = stdout_reader
            .await
            .unwrap_or_else(|_| StreamProcessor::new(skip_naration));
        let stderr_bytes = stderr_reader.await.unwrap_or_default();

        let bound_conv_id = processor.conversation_id.clone();
        let new_step_idx = processor.last_step_idx;
        let result_failed = processor
            .result_status
            .as_deref()
            .is_some_and(|status| status == "ERROR");
        // Each turn ends with exactly one `result` event. Reaching EOF without it
        // means the stream was truncated -- agy died, stdout closed early -- and
        // the exit status alone does not say so.
        let result_missing = !processor.saw_result;
        let result_error = processor.result_error.clone();

        if let Some(session) = self.sessions.get_mut(session_id) {
            if session.conversation_id.is_none() {
                session.conversation_id = bound_conv_id.clone();
            }
            if bound_conv_id.is_some() {
                session.last_step_idx = new_step_idx;
            }
        }

        // Read before the active session is cleared: agy reports a refused tool
        // call as a failed turn, and only the bridge knows the refusal was the
        // user's own answer rather than the provider breaking. The bridge's own
        // fail-closed denials do not count, or one of them could mask a real
        // failure later in the same turn.
        let mut denied_by_user = false;
        if let Some(bridge) = self.permission_bridge.clone() {
            denied_by_user = bridge.refused_during_prompt().await;
            if let Some(conv_id) = bound_conv_id.as_deref() {
                bridge.register_conversation(conv_id, session_id).await;
            }
            bridge.set_active_session(None).await;
            // The turn is over, so nothing can answer a prompt it left open. Left
            // in place it would wait out its timeout and mark a refusal, which
            // would land in a later turn. Cancellation clears these too, but a
            // turn can also end because agy died or its output became unreadable.
            bridge.abandon_pending(session_id).await;
        }
        if bound_conv_id.is_some() {
            let model_id = self
                .sessions
                .get(session_id)
                .and_then(|s| s.model_id.clone());
            self.persist_session(
                session_id,
                bound_conv_id.as_deref(),
                new_step_idx,
                model_id.as_deref(),
            );
        }

        let stop_reason = if was_cancelled {
            "cancelled"
        } else if denied_by_user {
            "refusal"
        } else {
            "end_turn"
        };
        let output_lines = vec![serde_json::to_string(&JsonRpcResponse {
            jsonrpc: "2.0",
            id: id.clone(),
            result: Some(json!({ "stopReason": stop_reason })),
            error: None,
        })
        .unwrap()];

        match result {
            Ok(status) => {
                let stderr_text = String::from_utf8_lossy(&stderr_bytes);
                if !stderr_text.is_empty() {
                    eprintln!("[agy-acp] agy stderr: {}", stderr_text.trim_end());
                }

                // A turn the user refused is an outcome, not a provider failure;
                // the bridge exists to make that a clean stop the client can show.
                if !was_cancelled
                    && !denied_by_user
                    && (!status.success() || result_failed || result_missing)
                {
                    eprintln!("[agy-acp] WARN: agy exited with status: {}", status);
                    // Updates already streamed to the client stay where they are;
                    // what must not happen is a failed turn ending in a success
                    // response, which is indistinguishable from a good one. This
                    // used to be gated on `had_updates`, so any turn that produced
                    // a single chunk before failing reported end_turn.
                    let msg = if let Some(error) = result_error.filter(|s| !s.is_empty()) {
                        format!("agy failed: {}", error.trim_end())
                    } else if result_missing && status.success() {
                        "agy stream ended without a result event".to_string()
                    } else if stderr_text.is_empty() {
                        format!("agy exited with status: {}", status)
                    } else {
                        format!("agy failed: {}", stderr_text.trim_end())
                    };
                    return vec![serde_json::to_string(&JsonRpcResponse {
                        jsonrpc: "2.0",
                        id,
                        result: None,
                        error: Some(json!({"code":-32000,"message":msg})),
                    })
                    .unwrap()];
                }
            }
            Err(e) => {
                return vec![serde_json::to_string(&JsonRpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: None,
                    error: Some(
                        json!({"code":-32000,"message":format!("failed to wait for agy: {e}")}),
                    ),
                })
                .unwrap()];
            }
        }

        output_lines
    }
}

/// Filter out leading narration ("I will ...", "I'll ...") from response parts.
/// The replay path in `db.rs` uses this outside tests.
pub fn filter_narration(parts: &[String]) -> Option<String> {
    let text = parts
        .iter()
        .filter(|part| !is_narration(part))
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

/// A part is considered narration if every non-empty line starts with "I will" or "I'll".
pub fn is_narration(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return false;
    }
    lines.iter().all(|l| {
        let line = l.trim_start();
        line.starts_with("I will") || line.starts_with("I'll") || line.starts_with("I’ll")
    })
}
