mod adapter;
mod cancel;
mod db;
mod hook_root;
mod permission;
mod protobuf;
mod streaming;
mod tools;
mod types;

#[cfg(test)]
mod tests;

use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use tokio::sync::mpsc;

use adapter::Adapter;
use clap::Parser;
use types::{JsonRpcRequest, JsonRpcResponse};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Skip pure narration messages from agy, such as "I will ...".
    #[arg(long = "skip-naration", default_value_t = false)]
    skip_naration: bool,

    /// Run agy's tool calls past the ACP client for approval instead of letting
    /// agy auto-deny them in headless mode.
    #[arg(long = "permission-prompts", default_value_t = false)]
    permission_prompts: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Internal: the `PreToolUse` hook agy invokes to ask the ACP client for permission.
    PermissionHook,
}

/// Starts the permission bridge and writes the hook agy will call into.
///
/// Both are needed for prompting to work, so a failure in either leaves the
/// adapter running with agy's default (headless) permission behaviour rather than
/// with agy's checks disabled and no bridge to replace them.
async fn start_permission_prompts(
    adapter: &Arc<tokio::sync::Mutex<Adapter>>,
    out_tx: &mpsc::UnboundedSender<Option<String>>,
) -> std::io::Result<(permission::PermissionBridge, hook_root::HookRoot)> {
    let bridge = permission::PermissionBridge::start(out_tx.clone())?;
    let hook_root = hook_root::HookRoot::create()?;
    bridge.set_hook_root(hook_root.path()).await;
    adapter
        .lock()
        .await
        .enable_permission_bridge(&bridge, hook_root.path());
    Ok((bridge, hook_root))
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Some(Command::PermissionHook) = cli.command {
        permission::run_hook();
        return;
    }

    let adapter = if cli.skip_naration {
        Adapter::new_with_skip_naration(true)
    } else {
        Adapter::new()
    };
    let adapter = Arc::new(tokio::sync::Mutex::new(adapter));
    let active_cancellations: cancel::CancelRegistry = cancel::CancelRegistry::default();

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Option<String>>();

    // Permission prompting is opt-in: enabling it disables agy's own tool gating
    // and makes this bridge the only thing standing between the model and the
    // tool, so it must not switch on by accident.
    // `_hook_root` must outlive the session loop: dropping it deletes the hook.
    let (bridge, _hook_root) = if cli.permission_prompts {
        match start_permission_prompts(&adapter, &out_tx).await {
            Ok((bridge, hook_root)) => (Some(bridge), Some(hook_root)),
            Err(e) => {
                eprintln!("agy-acp: could not enable permission prompts: {e}");
                eprintln!("agy-acp: continuing with agy's own permission handling");
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    std::thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) if !l.trim().is_empty() => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
                _ => {}
            }
        }
    });

    let mut stdout = io::stdout();
    let mut stdin_open = true;
    let mut pending_prompts = 0usize;

    loop {
        if !stdin_open && pending_prompts == 0 {
            break;
        }

        let line = if stdin_open {
            tokio::select! {
                output = out_rx.recv() => {
                    match output {
                        Some(Some(line)) => {
                            let _ = writeln!(stdout, "{}", line);
                            let _ = stdout.flush();
                        }
                        Some(None) => pending_prompts = pending_prompts.saturating_sub(1),
                        None => {}
                    }
                    continue;
                }
                input = rx.recv() => {
                    match input {
                        Some(line) => line,
                        None => {
                            stdin_open = false;
                            continue;
                        }
                    }
                }
            }
        } else {
            match out_rx.recv().await {
                Some(Some(line)) => {
                    let _ = writeln!(stdout, "{}", line);
                    let _ = stdout.flush();
                }
                Some(None) => pending_prompts = pending_prompts.saturating_sub(1),
                None => break,
            }
            continue;
        };

        while let Ok(output) = out_rx.try_recv() {
            match output {
                Some(line) => {
                    let _ = writeln!(stdout, "{}", line);
                    let _ = stdout.flush();
                }
                None => pending_prompts = pending_prompts.saturating_sub(1),
            }
        }

        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // A message with an id but no method is a response to something we sent —
        // currently only permission requests.
        if req.method.is_none() {
            if let (Some(bridge), Some(id)) = (bridge.as_ref(), req.id.as_ref()) {
                let result = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|v| v.get("result").cloned());
                if bridge.resolve_response(id, result).await {
                    continue;
                }
            }
        }

        let id = match req.id {
            Some(id) => id,
            None => {
                if req.method.as_deref() == Some("session/cancel") {
                    let params = req.params.unwrap_or(json!({}));
                    if let Some(session_id) = params.get("sessionId").and_then(|v| v.as_str()) {
                        active_cancellations.cancel(session_id);
                    }
                }
                continue;
            }
        };

        let output = match req.method.as_deref() {
            Some("initialize") => {
                let adapter = adapter.lock().await;
                vec![serde_json::to_string(&adapter.handle_initialize(id)).unwrap()]
            }
            Some("session/new") => {
                let mut adapter = adapter.lock().await;
                vec![serde_json::to_string(&adapter.handle_session_new(id)).unwrap()]
            }
            Some("session/load") => {
                let params = req.params.unwrap_or(json!({}));
                let mut adapter = adapter.lock().await;
                adapter.handle_session_load(id, &params)
            }
            Some("session/resume") => {
                let params = req.params.unwrap_or(json!({}));
                let mut adapter = adapter.lock().await;
                vec![serde_json::to_string(&adapter.handle_session_resume(id, &params)).unwrap()]
            }
            Some("session/prompt") => {
                let params = req.params.unwrap_or(json!({}));
                let session_id = params
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                // A prompt with no session id is malformed, but it still spawns a
                // real agy turn, so it is registered under the id it was given --
                // the empty one. A `session/cancel` naming that same id then
                // reaches it, rather than it being the one turn nothing can stop.
                let token = active_cancellations.register(&session_id);
                let adapter = Arc::clone(&adapter);
                let active_cancellations = active_cancellations.clone();
                let out_tx = out_tx.clone();
                let adapter_notify_tx = out_tx.clone();
                pending_prompts += 1;
                tokio::spawn(async move {
                    let output = {
                        let mut adapter = adapter.lock().await;
                        adapter
                            .handle_session_prompt(
                                id,
                                &params,
                                Arc::clone(&token),
                                adapter_notify_tx,
                            )
                            .await
                    };
                    active_cancellations.unregister(&session_id, &token);
                    for line in output {
                        let _ = out_tx.send(Some(line));
                    }
                    let _ = out_tx.send(None);
                });
                Vec::new()
            }
            Some("session/cancel") => {
                let params = req.params.unwrap_or(json!({}));
                if let Some(session_id) = params.get("sessionId").and_then(|v| v.as_str()) {
                    active_cancellations.cancel(session_id);
                }
                let r = JsonRpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: Some(json!({})),
                    error: None,
                };
                vec![serde_json::to_string(&r).unwrap()]
            }
            Some("session/set_model") | Some("session/setModel") => {
                let params = req.params.unwrap_or(json!({}));
                let mut adapter = adapter.lock().await;
                vec![serde_json::to_string(&adapter.handle_session_set_model(id, &params)).unwrap()]
            }
            Some("session/set_config_option") | Some("session/setConfigOption") => {
                let params = req.params.unwrap_or(json!({}));
                let mut adapter = adapter.lock().await;
                vec![
                    serde_json::to_string(&adapter.handle_session_set_config_option(id, &params))
                        .unwrap(),
                ]
            }
            Some(method) => {
                let r = JsonRpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: None,
                    error: Some(
                        json!({"code":-32601,"message":format!("method not found: {method}")}),
                    ),
                };
                vec![serde_json::to_string(&r).unwrap()]
            }
            None => continue,
        };

        for line in output {
            let _ = writeln!(stdout, "{}", line);
        }
        let _ = stdout.flush();
    }
}
