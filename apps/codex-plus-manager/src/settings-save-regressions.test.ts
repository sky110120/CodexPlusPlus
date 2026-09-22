import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import {
  mergeGrokProviderSnapshot,
  mergeSettingsDraft,
  runSingleFlight,
} from "./settings-draft.ts";

const app = fs.readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const commands = fs.readFileSync(new URL("../src-tauri/src/commands.rs", import.meta.url), "utf8");

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
