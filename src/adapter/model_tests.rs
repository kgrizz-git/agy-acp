//! Tests for model discovery, selection and config options.

use crate::test_support::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use uuid::Uuid;

use crate::adapter::Adapter;
use crate::types::AgyModel;

#[test]
fn test_adapter_uses_a_scratch_home_without_model_discovery() {
    let adapter = test_adapter();

    assert!(adapter.state_file.starts_with(std::env::temp_dir()));
    assert!(adapter.conversations_dir.starts_with(std::env::temp_dir()));
    assert!(adapter.available_models.is_empty());
}

#[test]
fn test_session_new_returns_models() {
    let mut adapter = test_adapter();
    let response = adapter.handle_session_new(json!(1));
    let result = response.result.as_ref().unwrap();
    assert!(result.get("sessionId").is_some());
    let models = result.get("models").unwrap();
    assert!(models.get("currentModelId").is_some());
    assert!(models.get("availableModels").is_some());
    let config_options = result.get("configOptions").unwrap().as_array().unwrap();
    assert_eq!(config_options.len(), 1);
    assert_eq!(config_options[0]["id"].as_str(), Some("model"));
    assert_eq!(config_options[0]["category"].as_str(), Some("model"));
    assert_eq!(config_options[0]["type"].as_str(), Some("select"));
    assert!(config_options[0].get("currentValue").is_some());
    assert!(config_options[0].get("options").is_some());
}

#[test]
fn test_session_set_model() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let set_resp = adapter.handle_session_set_model(
        json!(2),
        &json!({"sessionId": session_id, "modelId": "model-b"}),
    );
    assert!(set_resp.error.is_none());
    assert_eq!(
        adapter
            .sessions
            .get(&session_id)
            .unwrap()
            .model_id
            .as_deref(),
        Some("model-b")
    );
}

#[test]
fn test_session_set_model_missing_params() {
    let mut adapter = test_adapter();
    let resp = adapter.handle_session_set_model(json!(1), &json!({}));
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap()["code"].as_i64(), Some(-32602));
}

#[test]
fn test_session_set_model_unknown_session() {
    let mut adapter = test_adapter();
    let resp = adapter.handle_session_set_model(
        json!(1),
        &json!({"sessionId": "nonexistent", "modelId": "some-model"}),
    );
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap()["code"].as_i64(), Some(-32000));
}

#[test]
fn test_session_set_config_option_sets_model() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let set_resp = adapter.handle_session_set_config_option(
        json!(2),
        &json!({"sessionId": session_id, "configId": "model", "value": "model-b"}),
    );

    assert!(set_resp.error.is_none(), "error: {:?}", set_resp.error);
    assert_eq!(
        adapter
            .sessions
            .get(&session_id)
            .unwrap()
            .model_id
            .as_deref(),
        Some("model-b")
    );
    let config_options = set_resp.result.as_ref().unwrap()["configOptions"]
        .as_array()
        .unwrap();
    assert_eq!(config_options[0]["currentValue"].as_str(), Some("model-b"));
}

#[test]
fn test_session_set_config_option_rejects_unknown_config() {
    let mut adapter = test_adapter();
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = adapter.handle_session_set_config_option(
        json!(2),
        &json!({"sessionId": session_id, "configId": "not-model", "value": "Model B"}),
    );

    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap()["code"].as_i64(), Some(-32602));
}

#[test]
#[ignore]
fn test_session_set_model_persists() {
    let root = std::env::temp_dir().join(format!("agy-acp-model-persist-{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&root);

    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };

    adapter.persist_session("sess-m1", Some("conv-m1"), 0, None);

    adapter.restore_session_state("sess-m1");
    adapter.handle_session_set_model(
        json!(1),
        &json!({"sessionId": "sess-m1", "modelId": "Claude Opus 4.6 (Thinking)"}),
    );

    let adapter2 = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    let restored = adapter2.restore_session("sess-m1");
    assert_eq!(
        restored,
        Some((
            "conv-m1".to_string(),
            0,
            Some("Claude Opus 4.6 (Thinking)".to_string())
        ))
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_session_load_returns_models() {
    let mut adapter = test_adapter();
    adapter.sessions.insert(
        "test-load".to_string(),
        crate::types::Session {
            conversation_id: None,
            last_step_idx: -1,
            model_id: Some("Gemini 3.1 Pro (High)".to_string()),
            last_used: 0,
        },
    );
    adapter.persist_session(
        "test-load",
        Some("conv-load"),
        -1,
        Some("Gemini 3.1 Pro (High)"),
    );
    adapter.sessions.clear();

    let output = adapter.handle_session_load(json!(1), &json!({"sessionId": "test-load"}));
    let response: Value = serde_json::from_str(output.last().unwrap()).unwrap();
    assert!(
        response["error"].is_null(),
        "error: {:?}",
        response["error"]
    );
    let models = response["result"]["models"].as_object().unwrap();
    assert_eq!(
        models["currentModelId"].as_str(),
        Some("Gemini 3.1 Pro (High)")
    );
    assert_eq!(
        response["result"]["configOptions"][0]["currentValue"].as_str(),
        Some("Gemini 3.1 Pro (High)")
    );
}

#[test]
fn test_session_resume_returns_models() {
    let mut adapter = test_adapter();
    adapter.persist_session(
        "test-resume",
        Some("conv-resume"),
        -1,
        Some("GPT-OSS 120B (Medium)"),
    );
    adapter.sessions.clear();

    let response = adapter.handle_session_resume(json!(1), &json!({"sessionId": "test-resume"}));
    assert!(response.error.is_none(), "error: {:?}", response.error);
    let models = response.result.as_ref().unwrap()["models"]
        .as_object()
        .unwrap();
    assert_eq!(
        models["currentModelId"].as_str(),
        Some("GPT-OSS 120B (Medium)")
    );
    assert_eq!(
        response.result.as_ref().unwrap()["configOptions"][0]["currentValue"].as_str(),
        Some("GPT-OSS 120B (Medium)")
    );
}

#[test]
fn test_session_models_json_default() {
    let mut adapter = test_adapter();
    let models = adapter.session_models_json(None);
    let current = models["currentModelId"].as_str().unwrap();
    if adapter.available_models.is_empty() {
        assert_eq!(current, "");
    } else {
        assert_eq!(current, adapter.available_models[0].id);
    }
}

/// What `agy models` actually prints on stdout: `id<TAB>label`, no header —
/// the "Fetching available models..." banner goes to stderr.
const AGY_MODELS_STDOUT: &str = "\
gemini-3.7-flash-high\tGemini 3.7 Flash (High)
gemini-3.7-flash-low\tGemini 3.7 Flash (Low)
gemini-3.1-pro-high\tGemini 3.1 Pro (High)
claude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)
";

fn test_models() -> Vec<AgyModel> {
    vec![
        AgyModel {
            id: "model-a".to_string(),
            label: "Model A".to_string(),
        },
        AgyModel {
            id: "model-b".to_string(),
            label: "Model B".to_string(),
        },
    ]
}

#[test]
fn test_parse_models_output_splits_id_from_label() {
    let models = Adapter::parse_models_output(AGY_MODELS_STDOUT);
    assert_eq!(models.len(), 4);
    assert_eq!(models[0].id, "gemini-3.7-flash-high");
    assert_eq!(models[0].label, "Gemini 3.7 Flash (High)");
    assert_eq!(models[3].id, "claude-sonnet-4-6");
    assert_eq!(models[3].label, "Claude Sonnet 4.6 (Thinking)");
    for model in &models {
        assert!(
            !model.id.contains('\t') && !model.id.contains(' '),
            "id must be the bare model name agy accepts, got {:?}",
            model.id
        );
    }
}

#[test]
fn test_parse_models_output_without_label_column() {
    let models = Adapter::parse_models_output("gemini-3.7-flash-high\n\ngemini-3.1-pro-low\n");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gemini-3.7-flash-high");
    assert_eq!(models[0].label, "gemini-3.7-flash-high");
    assert_eq!(models[1].id, "gemini-3.1-pro-low");
}

#[test]
fn test_session_models_json_never_emits_a_label_as_an_id() {
    let mut adapter = test_adapter();
    adapter.available_models = Adapter::parse_models_output(AGY_MODELS_STDOUT);
    let models = adapter.session_models_json(None);
    let available = models["availableModels"].as_array().unwrap();
    assert_eq!(
        available[0]["modelId"].as_str(),
        Some("gemini-3.7-flash-high")
    );
    assert_eq!(
        available[0]["name"].as_str(),
        Some("Gemini 3.7 Flash (High)")
    );
    for entry in available {
        assert!(!entry["modelId"].as_str().unwrap().contains('\t'));
    }
    assert_eq!(
        models["currentModelId"].as_str(),
        Some("gemini-3.7-flash-high")
    );
}

#[test]
fn test_session_models_json_with_model() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let models = adapter.session_models_json(Some("model-b"));
    assert_eq!(models["currentModelId"].as_str(), Some("model-b"));
    let available = models["availableModels"].as_array().unwrap();
    assert_eq!(available.len(), 2);
    assert_eq!(available[0]["modelId"].as_str(), Some("model-a"));
    assert_eq!(available[0]["name"].as_str(), Some("Model A"));
    assert_eq!(available[1]["modelId"].as_str(), Some("model-b"));
}

#[test]
fn test_session_config_options_json_with_model() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let config_options = adapter.session_config_options_json(Some("model-b"));
    assert_eq!(config_options[0]["id"].as_str(), Some("model"));
    assert_eq!(config_options[0]["category"].as_str(), Some("model"));
    assert_eq!(config_options[0]["type"].as_str(), Some("select"));
    assert_eq!(config_options[0]["currentValue"].as_str(), Some("model-b"));
    let options = config_options[0]["options"].as_array().unwrap();
    assert_eq!(options.len(), 2);
    assert_eq!(options[0]["value"].as_str(), Some("model-a"));
    assert_eq!(options[0]["name"].as_str(), Some("Model A"));
    assert_eq!(options[1]["value"].as_str(), Some("model-b"));
}

/// A client that stored the old mangled `id<TAB>label` string and sends it back
/// must not have it passed through to `--model`, which agy would reject.
#[test]
fn test_set_model_strips_a_label_glued_to_the_id() {
    let mut adapter = test_adapter();
    adapter.available_models = Adapter::parse_models_output(AGY_MODELS_STDOUT);
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = adapter.handle_session_set_model(
        json!(2),
        &json!({
            "sessionId": session_id,
            "modelId": "gemini-3.7-flash-high\tGemini 3.7 Flash (High)",
        }),
    );
    assert!(resp.error.is_none(), "error: {:?}", resp.error);
    assert_eq!(
        adapter.sessions[&session_id].model_id.as_deref(),
        Some("gemini-3.7-flash-high")
    );
}

#[test]
fn test_set_model_rejects_a_model_agy_does_not_offer() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = adapter.handle_session_set_model(
        json!(2),
        &json!({"sessionId": session_id, "modelId": "Model B"}),
    );
    assert_eq!(resp.error.as_ref().unwrap()["code"].as_i64(), Some(-32602));
    assert_eq!(adapter.sessions[&session_id].model_id, None);
}

#[test]
fn test_set_config_option_rejects_a_model_agy_does_not_offer() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = adapter.handle_session_set_config_option(
        json!(2),
        &json!({"sessionId": session_id, "configId": "model", "value": "nope"}),
    );
    assert_eq!(resp.error.as_ref().unwrap()["code"].as_i64(), Some(-32602));
}
