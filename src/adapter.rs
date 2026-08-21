use fs2::FileExt;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use uuid::Uuid;

#[cfg(test)]
use crate::db::read_delta_from_db;
use crate::db::read_replay_updates_from_db;
use crate::permission::{PermissionBridge, SOCKET_ENV};
use crate::streaming::poll_streaming_delta;
use crate::types::*;

pub struct Adapter {
    pub sessions: HashMap<String, Session>,
    pub working_dir: String,
    pub conversations_dir: PathBuf,
    pub state_file: PathBuf,
    pub available_models: Vec<AgyModel>,
    pub skip_naration: bool,
    /// Present only when `--permission-prompts` is on. Its presence is what makes
    /// the adapter hand agy's tool gating over to the ACP client.
    pub permission_bridge: Option<PermissionBridge>,
    /// Private workspace root supplying the `PreToolUse` hook to agy.
    pub hook_root_dir: Option<PathBuf>,
}

/// Print-mode timeout used when permission prompts are on. Must outlast the
/// bridge's own wait so an unanswered prompt ends as a deny, not a failed turn.
const PERMISSION_PRINT_TIMEOUT: &str = "60m";

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

    pub fn new_with_skip_naration(skip_naration: bool) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let state_dir = PathBuf::from(&home).join(".openab/agy-acp");
        Self {
            sessions: HashMap::new(),
            working_dir: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/tmp".to_string()),
            conversations_dir: PathBuf::from(&home).join(".gemini/antigravity-cli/conversations"),
            state_file: state_dir.join("sessions.json"),
            available_models: Self::fetch_available_models(),
            skip_naration,
            permission_bridge: None,
            hook_root_dir: None,
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
        let mut store = self.load_store_inner();
        store.sessions.insert(
            session_id.to_string(),
            StoredSession {
                conversation_id: conversation_id.map(String::from),
                last_step_idx,
                model_id: model_id.map(String::from),
            },
        );
        let tmp = self.state_file.with_extension("tmp");
        if let Ok(file) = fs::File::create(&tmp) {
            if serde_json::to_writer_pretty(&file, &store).is_ok() {
                let _ = fs::rename(&tmp, &self.state_file);
            }
        }
    }

    /// Scans the conversations directory for SQLite database files (`*.db`)
    /// and returns their file stems as a set of conversation IDs.
    pub fn conversation_snapshot(&self) -> HashSet<String> {
        let Ok(entries) = fs::read_dir(&self.conversations_dir) else {
            return HashSet::new();
        };
        entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                if path.extension().map(|x| x == "db").unwrap_or(false) {
                    path.file_stem().map(|s| s.to_string_lossy().to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    #[cfg(test)]
    pub fn new_conversation_id(&self, before: &HashSet<String>) -> Option<String> {
        let after = self.conversation_snapshot();
        let mut created: Vec<_> = after.difference(before).collect();
        if created.is_empty() {
            return None;
        }
        if created.len() > 1 {
            eprintln!(
                "[agy-acp] WARN: multiple new agy conversation files appeared; \
                 refusing to bind"
            );
            return None;
        }
        Some(created.remove(0).clone())
    }

    pub fn read_replay_updates_from_db_inner(
        &self,
        conversation_id: &str,
    ) -> Option<(Vec<Value>, i64)> {
        read_replay_updates_from_db(&self.conversations_dir, conversation_id, self.skip_naration)
    }

    #[cfg(test)]
    fn read_delta_from_db_inner(
        &self,
        conversation_id: &str,
        after_step_idx: i64,
    ) -> Option<crate::types::ConversationDelta> {
        read_delta_from_db(&self.conversations_dir, conversation_id, after_step_idx)
    }

    #[cfg(test)]
    pub fn read_response_from_db(
        &self,
        conversation_id: &str,
        after_step_idx: i64,
    ) -> Option<(String, i64)> {
        self.read_delta_from_db_inner(conversation_id, after_step_idx)
            .and_then(|delta| delta.text.map(|text| (text, delta.max_step_idx)))
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

    fn evict_if_needed(&mut self) {
        const MAX_SESSIONS: usize = 64;
        while self.sessions.len() >= MAX_SESSIONS {
            if let Some(key) = self.sessions.keys().next().cloned() {
                self.sessions.remove(&key);
            }
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
        self.sessions.insert(
            session_id.to_string(),
            Session {
                conversation_id: Some(conversation_id),
                last_step_idx,
                // Sessions written before the parser was fixed hold an id with the
                // display label glued on; agy rejects that outright, so repair it
                // here rather than failing every resumed thread.
                model_id: model_id.as_deref().map(Self::sanitize_model_id),
            },
        );
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
        self.sessions.insert(
            session_id.clone(),
            Session {
                conversation_id: None,
                last_step_idx: -1,
                model_id: None,
            },
        );
        let result = self.session_config_result_json(&session_id, None);
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
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

    pub async fn handle_session_prompt(
        &mut self,
        id: Value,
        params: &Value,
        cancelled: Arc<AtomicBool>,
    ) -> Vec<String> {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");

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

        let snapshot = if self
            .sessions
            .get(session_id)
            .map(|s| s.conversation_id.is_none())
            .unwrap_or(false)
        {
            Some(self.conversation_snapshot())
        } else {
            None
        };

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

        let mut command = Command::new("agy");
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

        let mut stdout = child.stdout.take();
        let stdout_reader = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut stdout) = stdout.take() {
                let _ = stdout.read_to_end(&mut buf).await;
            }
            buf
        });

        let mut stderr = child.stderr.take();
        let stderr_reader = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut stderr) = stderr.take() {
                let _ = stderr.read_to_end(&mut buf).await;
            }
            buf
        });

        let initial_conv_id = self
            .sessions
            .get(session_id)
            .and_then(|s| s.conversation_id.clone());
        let initial_step_idx = self
            .sessions
            .get(session_id)
            .map(|s| s.last_step_idx)
            .unwrap_or(-1);
        let streaming_state = Arc::new(Mutex::new(StreamingState {
            conversation_id: initial_conv_id,
            base_step_idx: initial_step_idx,
            last_step_idx: initial_step_idx,
            had_updates: false,
            agent_text_lengths: HashMap::new(),
            emitted_tool_steps: HashSet::new(),
            last_title: None,
            skip_naration: self.skip_naration,
        }));
        let stop_polling = Arc::new(AtomicBool::new(false));
        let poll_conversations_dir = self.conversations_dir.clone();
        let poll_snapshot = snapshot.clone();
        let poll_session_id = session_id.to_string();
        let poll_state = Arc::clone(&streaming_state);
        let poll_stop = Arc::clone(&stop_polling);

        let poller = std::thread::spawn(move || {
            let mut stdout = io::stdout();
            while !poll_stop.load(Ordering::SeqCst) {
                for line in poll_streaming_delta(
                    &poll_conversations_dir,
                    poll_snapshot.as_ref(),
                    &poll_session_id,
                    &poll_state,
                ) {
                    let _ = writeln!(stdout, "{}", line);
                }
                let _ = stdout.flush();
                std::thread::sleep(Duration::from_millis(500));
            }
        });

        let mut was_cancelled = false;
        let result = tokio::select! {
            result = child.wait() => result,
            _ = async {
                while !cancelled.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            } => {
                was_cancelled = true;
                let _ = child.kill().await;
                child.wait().await
            }
        };
        let _ = stdout_reader.await;
        let stderr_bytes = stderr_reader.await.unwrap_or_default();
        stop_polling.store(true, Ordering::SeqCst);
        let _ = poller.join();

        let mut final_lines = Vec::new();
        for attempt in 0..3 {
            let lines = poll_streaming_delta(
                &self.conversations_dir,
                snapshot.as_ref(),
                session_id,
                &streaming_state,
            );
            final_lines.extend(lines);
            if attempt < 2 {
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        {
            let mut stdout = io::stdout();
            for line in &final_lines {
                let _ = writeln!(stdout, "{}", line);
            }
            let _ = stdout.flush();
        }

        // Scoped so the non-Send guard cannot be held across the awaits below.
        let (bound_conv_id, new_step_idx, had_updates) = {
            let state = streaming_state.lock().unwrap();
            (
                state.conversation_id.clone(),
                state.last_step_idx,
                state.had_updates || !final_lines.is_empty(),
            )
        };

        if let Some(session) = self.sessions.get_mut(session_id) {
            if session.conversation_id.is_none() {
                session.conversation_id = bound_conv_id.clone();
            }
            if bound_conv_id.is_some() {
                session.last_step_idx = new_step_idx;
            }
        }

        if let Some(bridge) = self.permission_bridge.clone() {
            if let Some(conv_id) = bound_conv_id.as_deref() {
                bridge.register_conversation(conv_id, session_id).await;
            }
            bridge.set_active_session(None).await;
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

                if !was_cancelled && !status.success() {
                    eprintln!("[agy-acp] WARN: agy exited with status: {}", status);
                    if !had_updates {
                        let msg = if stderr_text.is_empty() {
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
