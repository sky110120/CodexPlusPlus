const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

const extract = (file, name) => {
  const source = fs.readFileSync(require.resolve(`./renderer-inject/${file}`), "utf8");
  const body = source.match(new RegExp(`^  (?:async )?function ${name}\\([^]*?^  \\}`, "m"))?.[0];
  assert.ok(body, name);
  return body;
};

async function runNativeFastSettingsTests() {
  let response = {
    enhancementsEnabled: true,
    codexAppNativeFastUnlock: true,
    nativeFastLocalApiConfigured: true,
    relayProfiles: [{ configContents: "private backend fixture" }],
  };
  const context = vm.createContext({
    window: { __CODEX_PLUS_TEST_SERVICE_TIER__: true }, module: { exports: {} },
    codexPlusBackendSettings: {}, codexPlusBackendSettingsSeq: 0,
    codexPlusBackendSettingsLoaded: false,
    codexPlusFastPreferenceWrite: { pending: false, revision: 0 },
    postJson: async (_route, changes) => { response = { ...response, ...changes }; return response; },
    refreshCodexPlusBackendToggles() {}, runScanStep() {}, installCodexPlusDictation() {},
  });
  vm.runInContext(fs.readFileSync(require.resolve("./native-fast-unlock.js"), "utf8"), context);
  vm.runInContext(fs.readFileSync(require.resolve("./native-predictions.js"), "utf8"), context);
  const runtime = context.window.__codexPlusNativeFastUnlock.runtime;
  const predictions = context.window.__codexPlusNativePredictions.runtime;
  vm.runInContext([
    extract("30-service-tier.js", "loadBackendSettingsState"),
    extract("40-backend-settings.js", "setBackendSetting"),
  ].join("\n"), context);

  assert.equal(await context.loadBackendSettingsState(), true);
  assert.equal(runtime.allow("local", null), false, "首次加载仍关闭旧 Fast 补丁");
  assert.equal(context.codexPlusBackendSettings.codexAppNativeFastUnlock, true, "入口使用真实后端设置");
  assert.equal(context.window.__codexPlusSettings.codexAppNativeFastUnlock, false);
  assert.equal(context.window.__codexPlusSettings.nativeFastLocalApiConfigured, true);
  assert.deepEqual(Object.keys(context.window.__codexPlusSettings).sort(), [
    "codexAppNativeFastUnlock", "enhancementsEnabled", "nativeFastLocalApiConfigured",
  ]);
  assert.equal(JSON.stringify(context.window.__codexPlusSettings).includes("private backend fixture"), false);

  // 原生输入预测：只接收自己的开关与本机 API 布尔投影，同样随心跳与保存刷新。
  assert.equal(predictions.relax("local"), false, "预测开关默认关闭");
  response.codexAppNativePredictions = true;
  response.nativePredictionsLocalApiConfigured = true;
  assert.equal(await context.loadBackendSettingsState(), true);
  assert.equal(predictions.relax("local"), true);
  assert.deepEqual(Object.keys(context.window.__codexPlusNativePredictionsState.settings).sort(), [
    "codexAppNativePredictions", "enhancementsEnabled", "nativePredictionsLocalApiConfigured",
  ]);
  response.nativePredictionsLocalApiConfigured = false;
  assert.equal(await context.loadBackendSettingsState(), true);
  assert.equal(predictions.relax("local"), false, "心跳撤销预测的本机 API 投影");
  response.nativePredictionsLocalApiConfigured = true;
  await context.setBackendSetting("codexAppNativePredictions", false);
  assert.equal(predictions.relax("local"), false, "保存关闭后立即停止放宽");
  await context.setBackendSetting("codexAppNativePredictions", true);
  assert.equal(predictions.relax("local"), true);

  response.nativeFastLocalApiConfigured = false;
  assert.equal(await context.loadBackendSettingsState(), true);
  assert.equal(runtime.allow("local", null), false, "心跳也保持旧补丁关闭");
  assert.equal(context.window.__codexPlusSettings.nativeFastLocalApiConfigured, false);

  response.nativeFastLocalApiConfigured = true;
  await context.setBackendSetting("codexAppNativeFastUnlock", true);
  assert.equal(context.codexPlusBackendSettings.codexAppNativeFastUnlock, true);
  assert.equal(runtime.allow("local", null), false, "保存入口开关不重新开启旧补丁");
  await context.setBackendSetting("codexAppNativeFastUnlock", false);
  assert.equal(context.codexPlusBackendSettings.codexAppNativeFastUnlock, false);
  assert.equal(runtime.allow("local", null), false);
  await context.setBackendSetting("codexAppNativeFastUnlock", true);
  await context.setBackendSetting("enhancementsEnabled", false);
  assert.equal(runtime.allow("local", null), false, "增强总开关关闭后立即停止放行");
  runFastPreferenceControlTests();
  await runFastPreferenceWriteTests();
}

async function runFastPreferenceWriteTests() {
  const requests = [];
  const events = [];
  const notices = [];
  const settings = { enhancementsEnabled: true, codexAppNativeFastUnlock: false, codexAppNativePredictions: false };
  const context = vm.createContext({
    window: { __CODEX_PLUS_TEST_SERVICE_TIER__: true },
    codexPlusBackendSettings: settings,
    codexPlusBackendSettingsSeq: 0,
    codexPlusBackendSettingsLoaded: true,
    codexPlusFastPreferenceWrite: { pending: false, revision: 0 },
    postJson: (route, payload) => new Promise((resolve, reject) => requests.push({ route, payload, resolve, reject })),
    refreshCodexPlusBackendToggles() {},
    emitCodexPlusSettingsChanged: (...event) => events.push(event),
    showToast: (message) => notices.push(message),
  });
  vm.runInContext([
    extract("30-service-tier.js", "loadBackendSettingsState"),
    extract("40-backend-settings.js", "setBackendSetting"),
  ].join("\n"), context);

  const setFast = (value) => context.setBackendSetting("codexAppNativeFastUnlock", value);
  const getFast = () => context.codexPlusBackendSettings.codexAppNativeFastUnlock;
  for (const response of [
    { status: "failed", codexAppNativeFastUnlock: true },
    { status: "ok" },
    { codexAppNativeFastUnlock: "true" },
    { codexAppNativeFastUnlock: false },
    new Error("bridge rejected"),
  ]) {
    const write = setFast(true);
    assert.equal(getFast(), false, "请求期间不能显示未确认入口");
    const request = requests.shift();
    assert.equal(request.route, "/settings/set");
    if (response instanceof Error) request.reject(response);
    else request.resolve(response);
    await write;
    assert.equal(getFast(), false);
  }
  assert.equal(notices.length, 5);
  assert.deepEqual(events, [], "失败不能发布设置已保存");

  const staleRead = context.loadBackendSettingsState();
  const staleRequest = requests.shift();
  const enable = setFast(true);
  await setFast(true);
  assert.equal(requests.length, 1, "同键保存尚未完成时不再发第二个请求");
  requests.shift().resolve({ codexAppNativeFastUnlock: true });
  await enable;
  assert.equal(getFast(), true);
  assert.deepEqual(events, [["codexAppNativeFastUnlock", true, true]]);
  staleRequest.resolve({ enhancementsEnabled: true, codexAppNativeFastUnlock: false });
  await staleRead;
  assert.equal(getFast(), true, "发起于保存之前的读取不能覆盖 Fast 确认");

  const other = context.setBackendSetting("codexAppNativePredictions", true);
  const otherRequest = requests.shift();
  const disable = setFast(false);
  const fastRequest = requests.shift();
  const pendingRead = context.loadBackendSettingsState();
  requests.shift().resolve({ enhancementsEnabled: true, codexAppNativeFastUnlock: false });
  await pendingRead;
  assert.equal(getFast(), true, "保存中读取的值不替代本次确认");
  fastRequest.resolve({ codexAppNativeFastUnlock: false, codexAppNativePredictions: false });
  await disable;
  assert.equal(getFast(), false);
  assert.equal(context.codexPlusBackendSettings.codexAppNativePredictions, true, "Fast 确认只更新 Fast 字段");
  otherRequest.resolve({ ...settings, codexAppNativeFastUnlock: true, codexAppNativePredictions: true });
  await other;
  assert.equal(getFast(), false, "其它保存的陈旧 Fast 字段不能重新开启按钮");
  assert.equal(context.codexPlusBackendSettings.codexAppNativePredictions, true);
}

function runFastPreferenceControlTests() {
  const button = {};
  const node = { hidden: true, querySelector: (selector) => selector === "[data-codex-plus-fast-preference-settings]" ? button : null };
  let closes = 0;
  const opened = [];
  const settings = { enhancementsEnabled: true, codexAppNativeFastUnlock: true };
  const context = vm.createContext({
    window: { __codexPlusBackendGeneration: 1 }, codexPlusBackendGeneration: 1,
    codexPlusBackendSettings: settings, codexPlusBackendSettingsLoaded: false,
    openCodexPlusFastPreference: (anchor) => opened.push(anchor),
    closeCodexPlusFastPreference: () => { closes += 1; },
    document: { querySelectorAll: (selector) => selector === "[data-codex-plus-fast-preference-control]" ? [node] : [] },
    escapeHtml: String,
    codexPlusLocalSettingKey: (key) => key === "codexAppNativeFastUnlock" ? "nativeFastUnlock" : "",
    renderCodexPlusSettingsCatalogTitle: (item) => item.title,
    codexPlusSettingsCatalogDescription: (item) => item.description,
  });
  vm.runInContext([
    extract("92-extension-host.js", "codexPlusComposerFastPreferenceEnabled"),
    extract("40-backend-settings.js", "syncCodexPlusFastPreferenceControls"),
    extract("40-backend-settings.js", "renderCodexPlusSettingsCatalogRow"),
  ].join("\n"), context);
  const markup = (key) => context.renderCodexPlusSettingsCatalogRow({
    key, control: "bool", title: "Fixture", description: "Fixture description",
  });
  assert.match(markup("codexAppNativeFastUnlock"), /data-codex-plus-fast-preference-settings aria-haspopup="dialog">设置默认速度/);
  assert.match(markup("codexAppNativeFastUnlock"), /data-codex-plus-setting="nativeFastUnlock" role="switch"/);
  assert.doesNotMatch(markup("codexAppNativePredictions"), /data-codex-plus-fast-preference-settings/);

  context.syncCodexPlusFastPreferenceControls();
  assert.equal(node.hidden, true, "加载完成前不显示按钮");
  context.codexPlusBackendSettingsLoaded = true;
  context.syncCodexPlusFastPreferenceControls();
  assert.equal(node.hidden, false);
  assert.equal(closes, 1, "显示入口时不主动关闭菜单");
  button.onclick({ preventDefault() {} });
  assert.deepEqual(opened, [button], "设置入口使用实际按钮定位菜单");
  context.window.__codexPlusBackendGeneration = 2;
  button.onclick({ preventDefault() {} });
  assert.equal(opened.length, 1, "旧注入的处理器不能打开菜单");
  context.codexPlusBackendGeneration = 2;
  context.syncCodexPlusFastPreferenceControls();
  assert.equal(button.__codexPlusGeneration, 2, "已打开的设置页上的按钮重新绑定");
  settings.codexAppNativeFastUnlock = false;
  context.syncCodexPlusFastPreferenceControls();
  assert.equal(node.hidden, true, "关闭入口设置时隐藏按钮");
  assert.equal(closes, 2, "关闭入口设置时关闭菜单");
  settings.codexAppNativeFastUnlock = true;
  settings.enhancementsEnabled = false;
  context.syncCodexPlusFastPreferenceControls();
  assert.equal(node.hidden, true, "增强总开关关闭时也隐藏按钮");
}

module.exports = runNativeFastSettingsTests;
if (require.main === module) {
  runNativeFastSettingsTests().then(() => console.log("Native Fast settings refresh and safe projection passed")).catch(error => {
    console.error(error);
    process.exitCode = 1;
  });
}
