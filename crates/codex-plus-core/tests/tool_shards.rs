//! 工具分区（`settings.json` 的 `tools` 字段）的兼容性契约。
//!
//! 背景：管理器要按工具（Codex / Grok / 后续）区分供应商配置。为了不破坏
//! 老版本读取，Codex 仍以扁平字段为唯一事实来源，`tools.codex` 只是镜像。
//! 这里钉住四个方向：
//!   1. 老文件（无 `tools`）→ 新版本能读，且分片被补出来
//!   2. 新文件（有 `tools`）→ 扁平字段照旧可读，不因分片存在而错乱
//!   3. 改名 / 删除分片不会影响扁平字段
//!   4. 未知工具（老版本读新版本写的 `tools.claude`）不解析失败、不丢失

use codex_plus_core::settings::{BackendSettings, RelayMode, RelayProfile, SettingsStore};
use codex_plus_core::tools::{ToolConfig, ToolId};

fn profile(id: &str, name: &str) -> RelayProfile {
    RelayProfile {
        id: id.into(),
        name: name.into(),
        relay_mode: RelayMode::PureApi,
        ..RelayProfile::default()
    }
}

/// 老版本写的 settings.json：没有 `tools`，只有扁平字段。
#[test]
fn legacy_file_without_tools_shard_still_loads_and_gets_a_shard() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{
          "relayProfilesEnabled": true,
          "relayProfiles": [
            { "id": "legacy", "name": "老供应商", "relayMode": "pureApi" }
          ],
          "activeRelayId": "legacy",
          "relayTestModel": "legacy-model"
        }"#,
    )
    .unwrap();

    let store = SettingsStore::new(path.clone());
    let settings = store.load().unwrap();

    // 扁平字段原样读出来 —— 所有既有代码路径不受影响。
    assert_eq!(settings.active_relay_id, "legacy");
    assert_eq!(settings.relay_profiles[0].name, "老供应商");
    assert_eq!(settings.relay_test_model, "legacy-model");

    // 分片被补出来，内容与扁平字段一致。
    let shard = settings.tool_config(&ToolId::Codex);
    assert_eq!(shard.active_relay_id, "legacy");
    assert_eq!(shard.relay_profiles[0].name, "老供应商");
}

/// 保存之后，扁平字段和分片必须同时落在文件里，老版本才读得回来。
#[test]
fn save_writes_both_flat_fields_and_codex_shard() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let store = SettingsStore::new(path.clone());

    let settings = BackendSettings {
        relay_profiles: vec![profile("a", "供应商 A")],
        active_relay_id: "a".into(),
        ..BackendSettings::default()
    };
    store.save(&settings).unwrap();

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

    // 老版本读的路径：扁平字段还在。
    assert_eq!(raw["activeRelayId"], "a");
    assert_eq!(raw["relayProfiles"][0]["name"], "供应商 A");
    // 新版本读的路径：分片也在，且值一致。
    assert_eq!(raw["tools"]["codex"]["activeRelayId"], "a");
    assert_eq!(
        raw["tools"]["codex"]["relayProfiles"][0]["name"],
        "供应商 A"
    );
}

/// 分片里的 Codex 内容以扁平字段为准，不允许两边打架。
#[test]
fn stale_codex_shard_is_overwritten_by_flat_fields_on_load() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{
          "relayProfiles": [ { "id": "real", "name": "真实值", "relayMode": "pureApi" } ],
          "activeRelayId": "real",
          "tools": {
            "codex": {
              "relayProfiles": [ { "id": "stale", "name": "过期值", "relayMode": "pureApi" } ],
              "activeRelayId": "stale"
            }
          }
        }"#,
    )
    .unwrap();

    let settings = SettingsStore::new(path).load().unwrap();
    let shard = settings.tool_config(&ToolId::Codex);
    assert_eq!(shard.active_relay_id, "real");
    assert_eq!(shard.relay_profiles[0].name, "真实值");
}

/// 改 Codex 配置不能碰其它工具的分片。
#[test]
fn codex_changes_do_not_touch_other_tool_shards() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let store = SettingsStore::new(path.clone());

    let settings = BackendSettings {
        relay_profiles: vec![profile("codex-a", "Codex 供应商")],
        active_relay_id: "codex-a".into(),
        ..BackendSettings::default()
    };
    store.save(&settings).unwrap();

    // 模拟另一个工具已经写入过自己的分片。
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    raw["tools"]["grok"] = serde_json::to_value(ToolConfig {
        relay_profiles: vec![profile("grok-a", "Grok 供应商")],
        active_relay_id: "grok-a".into(),
        ..ToolConfig::default()
    })
    .unwrap();
    std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

    // 再走一次正常的 Codex 保存流程。
    let mut settings = store.load().unwrap();
    settings.active_relay_id = "codex-a".into();
    store.save(&settings).unwrap();

    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after["tools"]["grok"]["activeRelayId"], "grok-a");
    assert_eq!(
        after["tools"]["grok"]["relayProfiles"][0]["name"],
        "Grok 供应商"
    );
    assert_eq!(after["tools"]["codex"]["activeRelayId"], "codex-a");
}

/// 老版本读到新版本写的未知工具，必须原样保留而不是丢掉/解析失败。
#[test]
fn unknown_tool_shard_survives_load_and_save() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{
          "activeTool": "claude",
          "tools": {
            "claude": {
              "relayProfiles": [ { "id": "claude-a", "name": "Claude 供应商", "relayMode": "pureApi" } ],
              "activeRelayId": "claude-a"
            }
          }
        }"#,
    )
    .unwrap();

    let store = SettingsStore::new(path.clone());
    let settings = store.load().unwrap();

    assert_eq!(settings.active_tool, ToolId::Unknown("claude".into()));
    let claude = settings.tool_config(&ToolId::Unknown("claude".into()));
    assert_eq!(claude.active_relay_id, "claude-a");
    assert_eq!(claude.relay_profiles[0].name, "Claude 供应商");

    store.save(&settings).unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["tools"]["claude"]["activeRelayId"], "claude-a");
    assert_eq!(raw["activeTool"], "claude");
}

/// 未知工具的分片在 Codex 保存流程里也不能被抹掉。
#[test]
fn unknown_tool_shard_is_preserved_across_codex_save() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{
          "relayProfiles": [ { "id": "codex-a", "name": "Codex 供应商", "relayMode": "pureApi" } ],
          "activeRelayId": "codex-a",
          "tools": {
            "claude": {
              "relayProfiles": [ { "id": "claude-a", "name": "Claude 供应商", "relayMode": "pureApi" } ],
              "activeRelayId": "claude-a"
            }
          }
        }"#,
    )
    .unwrap();

    let store = SettingsStore::new(path.clone());
    let mut settings = store.load().unwrap();
    settings
        .relay_profiles
        .push(profile("codex-b", "Codex 供应商 B"));
    store.save(&settings).unwrap();

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        raw["tools"]["claude"]["relayProfiles"][0]["name"],
        "Claude 供应商"
    );
    assert_eq!(
        raw["tools"]["codex"]["relayProfiles"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn grok_api_key_round_trips_in_the_grok_shard_only() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let mut grok = profile("grok-a", "Grok 供应商");
    grok.api_key = "sk-grok".into();
    let mut settings = BackendSettings::default();
    settings.tools.insert(
        ToolId::Grok,
        ToolConfig {
            relay_profiles: vec![grok],
            active_relay_id: "grok-a".into(),
            ..ToolConfig::default()
        },
    );

    store.save(&settings).unwrap();
    let loaded = store.load().unwrap();

    assert_eq!(loaded.tools[&ToolId::Grok].relay_profiles[0].api_key, "sk-grok");
    assert!(loaded.relay_profiles[0].api_key.is_empty());
}

/// Grok 的新增、编辑、删除都只改自己的分片；并发写入的 Codex 字段和其它工具必须保留。
#[test]
fn grok_shard_crud_save_preserves_flat_codex_and_concurrent_fields() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let store = SettingsStore::new(path);
    let codex_profile = profile("codex-a", "Codex 供应商");
    let grok_profile = profile("grok-a", "Grok 供应商");
    let claude_profile = profile("claude-a", "Claude 供应商");
    let mut initial = BackendSettings {
        relay_profiles: vec![codex_profile.clone()],
        active_relay_id: codex_profile.id.clone(),
        ..BackendSettings::default()
    };
    initial.tools.insert(
        ToolId::Grok,
        ToolConfig {
            relay_profiles: vec![grok_profile.clone()],
            active_relay_id: grok_profile.id.clone(),
            ..ToolConfig::default()
        },
    );
    initial.tools.insert(
        ToolId::Unknown("claude".into()),
        ToolConfig {
            relay_profiles: vec![claude_profile.clone()],
            active_relay_id: claude_profile.id.clone(),
            ..ToolConfig::default()
        },
    );
    store.save(&initial).unwrap();

    let base = store.load().unwrap();
    let mut edited = base.clone();
    edited.relay_test_model = "codex-test-model".into();
    let grok = edited.tools.get_mut(&ToolId::Grok).unwrap();
    grok.relay_profiles[0].name = "Grok 供应商（已编辑）".into();
    grok.relay_profiles.push(profile("grok-b", "Grok 新增"));
    grok.active_relay_id = "grok-b".into();
    store.save_merged(&base, &edited).unwrap();

    let after_edit = store.load().unwrap();
    assert_eq!(after_edit.tools[&ToolId::Grok].relay_profiles.len(), 2);
    assert_eq!(
        after_edit.tools[&ToolId::Grok].relay_profiles[0].name,
        "Grok 供应商（已编辑）"
    );
    assert_eq!(after_edit.tools[&ToolId::Grok].active_relay_id, "grok-b");

    let base = after_edit;
    let mut deleted = base.clone();
    deleted.tools.get_mut(&ToolId::Grok).unwrap().relay_profiles = vec![];
    deleted.tools.get_mut(&ToolId::Grok).unwrap().active_relay_id.clear();
    // 模拟后台在 UI 快照之后更新了与 Grok 无关的 Codex 字段。
    store.update(serde_json::json!({"codexAppThreadIdBadge": true})).unwrap();
    store.save_merged(&base, &deleted).unwrap();

    let saved = store.load().unwrap();
    assert!(saved.tools[&ToolId::Grok].relay_profiles.is_empty());
    assert!(saved.tools[&ToolId::Grok].active_relay_id.is_empty());
    assert_eq!(saved.relay_profiles[0].id, "codex-a");
    assert!(saved.codex_app_thread_id_badge);
    assert_eq!(saved.relay_test_model, "codex-test-model");
    assert_eq!(saved.tools[&ToolId::Codex].relay_test_model, "codex-test-model");
    assert_eq!(saved.tools[&ToolId::Codex].active_relay_id, saved.active_relay_id);
    assert_eq!(saved.tools[&ToolId::Unknown("claude".into())].active_relay_id, "claude-a");
}
