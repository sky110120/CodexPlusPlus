use codex_plus_core::user_scripts::{BOOTSTRAP_SCRIPT, UserScriptManager};
use serde_json::json;
use std::{fs, process::Command};

#[test]
fn reload_cleans_resources_and_applies_current_files_and_switches() {
    let temp = tempfile::tempdir().unwrap();
    let user = temp.path().join("user");
    fs::create_dir_all(&user).unwrap();
    let manager = UserScriptManager::new(
        temp.path().join("builtin"),
        &user,
        temp.path().join("config.json"),
    );
    let path = user.join("demo.js");
    fs::write(
        &path,
        r#"
window.active = (window.active || 0) + 1;
window.version = 1;
window.__codexPlusUserScripts.registerCleanup(() => { window.active--; });
"#,
    )
    .unwrap();
    let first = manager.build_reload_bundle().unwrap();
    fs::write(
        &path,
        r#"
window.active = (window.active || 0) + 1;
window.version = 2;
window.__codexPlusUserScripts.registerCleanup(() => { window.active--; });
"#,
    )
    .unwrap();
    let updated = manager.build_reload_bundle().unwrap();
    manager.set_script_enabled("user:demo.js", false).unwrap();
    let disabled = manager.build_reload_bundle().unwrap();
    manager.set_script_enabled("user:demo.js", true).unwrap();
    manager.set_global_enabled(false).unwrap();
    let globally_disabled = manager.build_reload_bundle().unwrap();
    manager.set_global_enabled(true).unwrap();
    fs::write(&path, "window.legacy = (window.legacy || 0) + 1;").unwrap();
    let legacy = manager.build_reload_bundle().unwrap();
    let initial = manager.build_initial_bundle().unwrap();
    manager.delete_user_script("user:demo.js").unwrap();
    let deleted = manager.build_reload_bundle().unwrap();
    let empty_initial = manager.build_initial_bundle().unwrap();
    let input = json!({"first": first, "updated": updated, "disabled": disabled,
        "globallyDisabled": globally_disabled, "legacy": legacy, "deleted": deleted,
        "bootstrap": BOOTSTRAP_SCRIPT, "initial": initial, "emptyInitial": empty_initial});
    let fixture = temp.path().join("bundles.json");
    fs::write(&fixture, input.to_string()).unwrap();
    let output = Command::new("node").arg("-e").arg(r#"
const assert = require('node:assert/strict');
const vm = require('node:vm');
const bundles = require(process.argv[1]);
let reloads = 0;
const timers = [];
const window = { location: { href: 'app://-/index.html', reload() { reloads++; } },
  electronBridge: {}, setTimeout(fn) { timers.push(fn); } };
window.top = window.self = window;
const context = vm.createContext({ window, process, console });
const run = (name) => JSON.parse(vm.runInContext(bundles[name], context));
assert.equal(run('first').mode, 'scripts');
assert.equal(window.active, 1);
for (let i = 0; i < 20; i++) run('updated');
assert.equal(window.active, 1);
assert.equal(window.version, 2);
assert.equal(run('disabled').mode, 'scripts');
assert.equal(window.active, 0);
assert.deepEqual(Object.keys(window.__codexPlusUserScripts.scripts), []);
run('first'); run('globallyDisabled');
assert.equal(window.active, 0);
run('first'); run('deleted');
assert.equal(window.active, 0);
run('legacy');
assert.throws(() => run('first'), /Codex\+\+/);
assert.equal(window.active, 0); // 不能不清理就追加新实例
window.__codexPlusUserScriptsBootstrap = true;
assert.equal(run('first').mode, 'page');
assert.equal(run('first').mode, 'page');
assert.equal(timers.length, 1);
timers.shift()();
assert.equal(reloads, 1);

// 清理抛错时不加载新实例。
window.__codexPlusUserScripts = { scripts: { broken: { cleanups: [() => { throw Error('cleanup'); }] } } };
assert.equal(run('updated').mode, 'page');
assert.equal(window.active, 0);

// 新页面启动只请求一次当前脚本，且不在子框架执行。
let requests = 0;
const fresh = { location: window.location, electronBridge: {},
  __codexSessionDeleteBridge(path) { assert.equal(path, '/user-scripts/load'); requests++; return Promise.resolve({}); } };
fresh.self = fresh.top = fresh;
const startup = vm.createContext({window: fresh, document: {readyState: 'complete'}, console});
vm.runInContext(bundles.bootstrap, startup);
vm.runInContext(bundles.bootstrap, startup);
assert.equal(requests, 1);
fresh.__codexPlusUserScriptsBootstrap = false;
fresh.top = {};
vm.runInContext(bundles.bootstrap, startup);
assert.equal(requests, 1);

// 重复启动请求、桥接恢复及刷新后的新页面均不得再次刷新或叠加旧脚本。
for (let page = 0; page < 3; page++) {
  let pageReloads = 0;
  const scheduled = [];
  const pageWindow = { electronBridge: {}, __codexPlusUserScriptsBootstrap: true,
    location: { href: 'app://-/index.html', reload() { pageReloads++; } },
    setTimeout(fn) { scheduled.push(fn); } };
  pageWindow.top = pageWindow.self = pageWindow;
  const pageContext = vm.createContext({window: pageWindow, console});
  for (let repeat = 0; repeat < 20; repeat++) vm.runInContext(bundles.initial, pageContext);
  assert.equal(pageWindow.legacy, 1);
  assert.equal(scheduled.length, 0);
  assert.equal(JSON.parse(vm.runInContext(bundles.legacy, pageContext)).mode, 'page');
  vm.runInContext(bundles.initial, pageContext); // 刷新在途时仍不追加实例。
  assert.equal(pageWindow.legacy, 1);
  assert.equal(scheduled.length, 1);
  scheduled.shift()();
  assert.equal(pageReloads, 1); // 只有显式重载产生一次刷新。
}

// 即便第一次无脚本，自动初始化也不能随重复请求变成隐式热重载。
const emptyWindow = {};
const emptyContext = vm.createContext({window: emptyWindow, console});
vm.runInContext(bundles.emptyInitial, emptyContext);
vm.runInContext(bundles.initial, emptyContext);
assert.equal(emptyWindow.legacy, undefined);

(async () => {
const settle = async () => { await Promise.resolve(); await Promise.resolve(); };
const drainRetry = async (scheduled) => {
  assert.equal(scheduled.length, 1);
  const {fn, delay} = scheduled.shift();
  assert.ok(delay > 0 && delay <= 250, `unexpected retry delay: ${delay}`);
  fn();
  await settle();
};

// Missing bridge functions are retried, while duplicate bootstrap injection
// must not create a second retry chain.
let recoveredRequests = 0;
const recoveryTimers = [];
const recoveryWindow = {electronBridge: {}, location: {href: 'app://-/index.html'},
  setTimeout(fn, delay) { recoveryTimers.push({fn, delay}); }};
recoveryWindow.top = recoveryWindow.self = recoveryWindow;
const recoveryContext = vm.createContext({window: recoveryWindow, document: {readyState: 'complete'}, console});
vm.runInContext(bundles.bootstrap, recoveryContext);
vm.runInContext(bundles.bootstrap, recoveryContext);
assert.equal(recoveredRequests, 0);
assert.equal(recoveryTimers.length, 1);
recoveryWindow.__codexSessionDeleteBridge = () => { recoveredRequests++; return Promise.resolve({}); };
await drainRetry(recoveryTimers);
assert.equal(recoveredRequests, 1);
assert.equal(recoveryTimers.length, 0);

// Failed status, rejected promises, and synchronous bridge throws all retry;
// loading the same initial bundle on every response still executes it once.
let transientRequests = 0;
let transientReloads = 0;
const transientTimers = [];
const transientWindow = {electronBridge: {}, location: {href: 'app://-/index.html', reload() { transientReloads++; }},
  setTimeout(fn, delay) { transientTimers.push({fn, delay}); }};
transientWindow.top = transientWindow.self = transientWindow;
const transientContext = vm.createContext({window: transientWindow, document: {readyState: 'complete'}, console});
transientWindow.__codexSessionDeleteBridge = () => {
  transientRequests++;
  vm.runInContext(bundles.initial, transientContext);
  if (transientRequests === 1) return Promise.resolve({status: 'failed'});
  if (transientRequests === 2) return Promise.reject(Error('bridge not ready'));
  if (transientRequests === 3) throw Error('temporary bridge error');
  return Promise.resolve({status: 'loaded'});
};
vm.runInContext(bundles.bootstrap, transientContext);
vm.runInContext(bundles.bootstrap, transientContext);
await settle();
assert.equal(transientRequests, 1);
assert.equal(transientTimers.length, 1);
for (let attempt = 0; attempt < 3; attempt++) {
  await drainRetry(transientTimers);
  if (attempt < 2) assert.equal(transientTimers.length, 1);
}
assert.equal(transientRequests, 4);
assert.equal(transientTimers.length, 0);
assert.equal(transientWindow.legacy, 1);
assert.equal(transientReloads, 0);

// Persistent failures stop after the initial request plus three retries.
let failedRequests = 0;
let domReadyHandler;
const failedTimers = [];
const failedWindow = {electronBridge: {}, location: {href: 'app://-/index.html'},
  setTimeout(fn, delay) { failedTimers.push({fn, delay}); },
  __codexSessionDeleteBridge() { failedRequests++; return Promise.resolve({status: 'failed'}); }};
failedWindow.top = failedWindow.self = failedWindow;
const failedDocument = {readyState: 'loading', addEventListener(name, handler, options) {
  assert.equal(name, 'DOMContentLoaded');
  assert.equal(options.once, true);
  domReadyHandler = handler;
}};
const failedContext = vm.createContext({window: failedWindow, document: failedDocument, console});
vm.runInContext(bundles.bootstrap, failedContext);
vm.runInContext(bundles.bootstrap, failedContext);
assert.equal(failedRequests, 0);
assert.equal(failedTimers.length, 0);
domReadyHandler(new Event('DOMContentLoaded'));
await settle();
assert.equal(failedRequests, 1);
assert.equal(failedTimers.length, 1);
for (let retry = 0; retry < 3; retry++) {
  await drainRetry(failedTimers);
  if (retry < 2) assert.equal(failedTimers.length, 1);
}
assert.equal(failedRequests, 4);
assert.equal(failedTimers.length, 0);
vm.runInContext(bundles.bootstrap, failedContext);
assert.equal(failedRequests, 4);
assert.equal(failedTimers.length, 0);
})().catch((error) => { console.error(error); process.exitCode = 1; });
"#).arg(&fixture).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
