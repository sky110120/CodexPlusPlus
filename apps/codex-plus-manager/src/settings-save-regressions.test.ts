import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import {
  mergeGrokProviderSnapshot,
  mergeSettingsDraft,
  runSingleFlight,
} from "./settings-draft.ts";
import {
  reconcileRelayModelDraft,
  relayModelFieldsChanged,
} from "./relay-profile-model.ts";

const app = fs.readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const commands = fs.readFileSync(new URL("../src-tauri/src/commands.rs", import.meta.url), "utf8");

function readTestModel(contents: string): string {
  return /^\s*model\s*=\s*"([^"]*)"/m.exec(contents)?.[1] ?? "";
}

function setTestModel(contents: string, model: string): string {
  const lines = contents.split(/\r?\n/);
  const modelIndex = lines.findIndex((line) => /^\s*model\s*=/.test(line));
  if (!model.trim()) {
    if (modelIndex >= 0) lines.splice(modelIndex, 1);
  } else if (modelIndex >= 0) {
    lines[modelIndex] = `model = "${model}"`;
  } else {
    lines.unshift(`model = "${model}"`);
  }
  return lines.join("\n").replace(/\n*$/, "\n");
}

test("tool switching uses a single-flight guard and commits only after save", () => {
  const start = app.indexOf("const switchTool = async");
  const end = app.indexOf("const refreshOverview", start);
  const source = app.slice(start, end);

  assert.match(source, /runSingleFlight\(toolSwitchInFlightRef/);
  assert.match(source, /const previousSettings = settings \? normalizeSettings\(settings\.settings\) : null;/);
  assert.match(source, /const next = \{ \.\.\.loadedSettings, activeTool: toolId \}/);
  assert.match(source, /save_settings", saveSettingsArgs\(next\)/);
  assert.doesNotMatch(source, /saveSettingsArgs\(settingsForm\)/);
  assert.match(source, /if \(!isSuccessStatus\(result\.value\.status\)\)/);
  assert.match(source, /mergeSettingsDraft\(normalizeSettings\(result\.value!\.settings\), previousSettings!, current\)/);
  assert.doesNotMatch(source, /toolSwitchQueueRef|toolSwitchGenerationRef/);
});

test("single-flight rejects a second write, releases after failure, and permits retry", async () => {
  const ref = { current: false };
  let writes = 0;
  let release!: () => void;
  const pending = new Promise<void>((resolve) => {
    release = resolve;
  });

  const first = runSingleFlight(ref, async () => {
    writes += 1;
    await pending;
    return "first";
  });
  const second = await runSingleFlight(ref, async () => {
    writes += 1;
    return "second";
  });

  assert.equal(second.accepted, false);
  assert.equal(writes, 1);
  release();
  assert.deepEqual(await first, { accepted: true, value: "first" });
  assert.equal(ref.current, false);

  await assert.rejects(runSingleFlight(ref, async () => {
    writes += 1;
    throw new Error("save failed");
  }), /save failed/);
  assert.equal(ref.current, false);
  const retry = await runSingleFlight(ref, async () => {
    writes += 1;
    return "retry";
  });
  assert.deepEqual(retry, { accepted: true, value: "retry" });
  assert.equal(writes, 3);
});

test("switching tools preserves dirty drafts but refreshes clean fields from a newer backend snapshot", () => {
  const base = {
    activeTool: "codex",
    relayTestModel: "base-model",
    codexAppThreadIdBadge: false,
    tools: {
      grok: { activeRelayId: "grok-base", relayTestModel: "grok-base" },
    },
  };
  const draft = {
    ...base,
    activeTool: "grok",
    relayTestModel: "local-draft",
    tools: {
      grok: { activeRelayId: "grok-local", relayTestModel: "grok-base" },
    },
  };
  const latest = {
    ...base,
    codexAppThreadIdBadge: true,
    relayTestModel: "remote-update",
    tools: {
      grok: { activeRelayId: "grok-remote", relayTestModel: "grok-remote" },
    },
  };

  assert.deepEqual(mergeSettingsDraft(latest, base, draft), {
    activeTool: "grok",
    relayTestModel: "local-draft",
    codexAppThreadIdBadge: true,
    tools: {
      grok: { activeRelayId: "grok-local", relayTestModel: "grok-remote" },
    },
  });
});

test("Grok application persists only its shard under the settings lock", () => {
  const start = commands.indexOf("pub fn apply_grok_relay_profile");
  const end = commands.indexOf("#[tauri::command]", start + 1);
  const source = commands.slice(start, end);

  assert.match(source, /store\.update_with_effects\(\s*serde_json::json!\(\{\s*"tools": \{ "grok": \{ "activeRelayId": requested_id \} \}/s);
  assert.doesNotMatch(source, /store\.save\(&next\)/);
  assert.doesNotMatch(source, /saved_shard/);
});

test("Grok refresh updates only the Grok shard and preserves other draft fields", () => {
  const settings = {
    activeTool: "codex",
    relayTestModel: "local-draft",
    tools: {
      codex: { activeRelayId: "codex-local" },
      grok: { activeRelayId: "old", relayProfiles: [{ id: "old" }] },
    },
  };
  const profiles = [{ id: "new", apiKey: "sk-new" }];

  assert.deepEqual(mergeGrokProviderSnapshot(settings, profiles, "new"), {
    activeTool: "codex",
    relayTestModel: "local-draft",
    tools: {
      codex: { activeRelayId: "codex-local" },
      grok: { activeRelayId: "new", relayProfiles: profiles },
    },
  });
});

test("relay editor keeps a generated live model-list head implicit until the user edits model fields", () => {
  const detailStart = app.indexOf("function RelayProfileDetail(");
  const editorStart = app.indexOf("function RelayProfileEditor(", detailStart);
  const detail = app.slice(detailStart, editorStart);
  const effectStart = detail.indexOf("useEffect(() => {");
  const effectEnd = detail.indexOf("const validationSettings", effectStart);
  const hydration = detail.slice(effectStart, effectEnd);
  const saveStart = detail.indexOf("const saveDraft = async");
  const saveEnd = detail.indexOf("const switchDraft", saveStart);
  const save = detail.slice(saveStart, saveEnd);

  assert.match(hydration, /modelFieldsTouchedRef\.current/);
  assert.match(hydration, /reconcileRelayModelDraft\(/);
  assert.doesNotMatch(hydration, /if \(modelFieldsTouchedRef\.current\) return;/);
  assert.match(detail, /const updateDraftProfile = \(next: RelayProfile\)/);
  assert.match(detail, /relayModelFieldsChanged\(draftRef\.current,\s*next,\s*codexModelFromConfig\)/);
  assert.match(detail, /onProfileChange=\{updateDraftProfile\}/);
  assert.match(hydration, /modelFieldsTouchedRef\.current = false;\s*\}, \[profile\.id\]\);/);
  assert.match(hydration, /codexModelFromConfig,\s*\(contents, model\) => setRootTomlStringKey\(contents, "model", parseModelSuffix\(model\)\.slug\)/);
  assert.match(save, /deriveRelayProfileFromFiles\(draftWithWindows\)/);
  assert.doesNotMatch(save, /deriveRelayProfileFromFiles\(\{\s*\.\.\.draftWithWindows,\s*configContents:\s*relayFiles/);
});

test("relay profile draft save keeps an implicit live head unset but persists an explicit same-model choice", () => {
  const stored = {
    model: "",
    configContents: "",
    modelList: "model-a\nmodel-b",
    name: "Stored relay",
  };
  const liveDraft = {
    ...stored,
    model: "model-a",
    configContents: 'model = "model-a"\nmodel_provider = "custom"\n',
    baseUrl: "https://live.example/v1",
  };
  const currentStoredDraft = { ...stored, baseUrl: "" };
  const firstHydration = reconcileRelayModelDraft(
    stored,
    currentStoredDraft,
    liveDraft,
    { liveModel: readTestModel(liveDraft.configContents), modelListHead: "model-a" },
    false,
    readTestModel,
    setTestModel,
  );
  assert.equal(firstHydration.model, "");
  assert.equal(readTestModel(firstHydration.configContents), "");

  const nameOnlyEdit = { ...firstHydration, name: "Renamed relay" };
  assert.equal(relayModelFieldsChanged(firstHydration, nameOnlyEdit, readTestModel), false);
  const savedAfterNameEdit = {
    ...nameOnlyEdit,
    model: readTestModel(nameOnlyEdit.configContents),
  };
  assert.equal(savedAfterNameEdit.name, "Renamed relay");
  assert.equal(savedAfterNameEdit.model, "");
  assert.equal(readTestModel(savedAfterNameEdit.configContents), "");

  const sameHeadExplicitEdit = {
    ...firstHydration,
    model: "model-a",
    configContents: setTestModel(firstHydration.configContents, "model-a"),
  };
  assert.equal(relayModelFieldsChanged(firstHydration, sameHeadExplicitEdit, readTestModel), true);
  const refreshedLiveDraft = {
    ...liveDraft,
    configContents: setTestModel(liveDraft.configContents, "model-b"),
    model: "model-b",
    baseUrl: "https://refreshed.example/v1",
  };
  const refreshedDraft = reconcileRelayModelDraft(
    stored,
    sameHeadExplicitEdit,
    refreshedLiveDraft,
    { liveModel: "model-b", modelListHead: "model-a" },
    true,
    readTestModel,
    setTestModel,
  );
  assert.equal(refreshedDraft.baseUrl, "https://refreshed.example/v1");
  assert.equal(readTestModel(refreshedDraft.configContents), "model-a");
  const savedAfterExplicitEdit = {
    ...refreshedDraft,
    model: readTestModel(refreshedDraft.configContents),
  };
  assert.equal(savedAfterExplicitEdit.model, "model-a");
});

test("explicit stored models and non-head live values are not classified as generated heads", () => {
  const storedExplicitConfig = {
    model: "",
    configContents: 'model = "model-a"\nmodel_provider = "custom"\n',
    modelList: "model-a\nmodel-b",
  };
  const liveNonHead = {
    ...storedExplicitConfig,
    model: "model-b",
    configContents: 'model = "model-b"\nmodel_provider = "custom"\n',
    name: "Explicit config relay",
    baseUrl: "https://live.example/v1",
  };
  const kept = reconcileRelayModelDraft(
    storedExplicitConfig,
    storedExplicitConfig,
    liveNonHead,
    { liveModel: "model-b", modelListHead: "model-a" },
    false,
    readTestModel,
    setTestModel,
  );
  assert.equal(kept.model, "model-b");
  assert.equal(readTestModel(kept.configContents), "model-b");

  const storedExplicitField = {
    model: "model-a",
    configContents: "",
    modelList: "model-a\nmodel-b",
  };
  const currentExplicitField = {
    ...storedExplicitField,
    name: liveNonHead.name,
    baseUrl: liveNonHead.baseUrl,
  };
  const keptWithExplicitField = reconcileRelayModelDraft(
    storedExplicitField,
    currentExplicitField,
    { ...currentExplicitField, ...liveNonHead },
    { liveModel: "model-b", modelListHead: "model-a" },
    false,
    readTestModel,
    setTestModel,
  );
  assert.equal(keptWithExplicitField.model, "model-b");
  assert.equal(readTestModel(keptWithExplicitField.configContents), "model-b");
});

test("user-script reload handler only updates inventory and preserves settings/form state", async () => {
  const start = app.indexOf("const reloadUserScripts = async");
  const end = app.indexOf("const installMarketScript", start);
  const source = app
    .slice(start, end)
    .replace("const reloadUserScripts = async () =>", "async function reloadUserScripts()")
    .replace("call<SettingsResult>", "call")
    .replace(/\n  \};\s*$/, "\n}");

  const cases = [
    {
      status: "failed",
      message: "reload failed",
      scripts: [{ key: "user:failed.js", status: "failed" }],
    },
    {
      status: "failed",
      message: "partial script failure",
      scripts: [{ key: "user:partial.js", status: "failed" }],
    },
  ];

  for (const scenario of cases) {
    const initialSettings = {
      status: "ok",
      message: "loaded",
      settings: { activeTool: "codex", relayApiKey: "keep-me" },
      user_scripts: { scripts: [{ key: "user:old.js" }] },
    };
    const form = { activeTool: "codex", relayApiKey: "draft-key" };
    const payload = {
      status: scenario.status,
      message: scenario.message,
      settings: { activeTool: "codex", relayApiKey: "" },
      user_scripts: { scripts: scenario.scripts },
    };
    let currentSettings: typeof initialSettings | null = initialSettings;
    let marketState: unknown = null;
    let notice: unknown = null;
    const run = async (task: () => Promise<unknown>) => task();
    const call = async (command: string) => {
      assert.equal(command, "reload_user_scripts");
      return payload;
    };
    const setSettings = (update: typeof currentSettings | ((current: typeof currentSettings) => typeof currentSettings)) => {
      currentSettings = typeof update === "function" ? update(currentSettings) : update;
    };
    const setSettingsForm = () => {
      throw new Error("reloadUserScripts must not update settingsForm");
    };
    const setScriptMarket = (update: (current: unknown) => unknown) => {
      marketState = update(marketState);
    };
    const syncMarketInstalledState = (_current: unknown, userScripts: unknown) => userScripts;
    const showResultNotice = (_title: string, result: unknown) => {
      notice = result;
    };
    const t = (value: string) => value;
    const reloadUserScripts = new Function(
      "run",
      "call",
      "setSettings",
      "setSettingsForm",
      "setScriptMarket",
      "syncMarketInstalledState",
      "showResultNotice",
      "t",
      `return (${source});`,
    )(run, call, setSettings, setSettingsForm, setScriptMarket, syncMarketInstalledState, showResultNotice, t) as () => Promise<void>;

    await reloadUserScripts();

    assert.deepEqual(currentSettings?.settings, initialSettings.settings);
    assert.deepEqual(currentSettings?.user_scripts, payload.user_scripts);
    assert.deepEqual(form, { activeTool: "codex", relayApiKey: "draft-key" });
    assert.deepEqual(marketState, payload.user_scripts);
    assert.equal(notice, payload);
  }

  let nullSettings: { user_scripts: unknown } | null = null;
  const nullPayload = {
    status: "failed",
    message: "reload failed",
    settings: { activeTool: "codex", relayApiKey: "" },
    user_scripts: { scripts: [{ key: "user:null.js", status: "failed" }] },
  };
  const run = async (task: () => Promise<unknown>) => task();
  const call = async () => nullPayload;
  const setSettings = (update: typeof nullSettings | ((current: typeof nullSettings) => typeof nullSettings)) => {
    nullSettings = typeof update === "function" ? update(nullSettings) : update;
  };
  const setScriptMarket = () => {};
  const syncMarketInstalledState = (_current: unknown, userScripts: unknown) => userScripts;
  const showResultNotice = () => {};
  const t = (value: string) => value;
  const reloadUserScripts = new Function(
    "run",
    "call",
    "setSettings",
    "setSettingsForm",
    "setScriptMarket",
    "syncMarketInstalledState",
    "showResultNotice",
    "t",
    `return (${source});`,
  )(
    run,
    call,
    setSettings,
    () => {
      throw new Error("reloadUserScripts must not update settingsForm");
    },
    setScriptMarket,
    syncMarketInstalledState,
    showResultNotice,
    t,
  ) as () => Promise<void>;

  await reloadUserScripts();
  assert.equal(nullSettings, null);
});

test("hidden recommendations do not fetch ads during startup", () => {
  const start = app.indexOf("const startup = await run");
  const end = app.indexOf("await refreshTools(true)", start);
  const startup = app.slice(start, end);

  assert.match(startup, /\/\/ await refreshAds\(true\);/);
  assert.equal(
    startup.split("\n").some((line) => line.trim() === "await refreshAds(true);"),
    false,
  );
});
