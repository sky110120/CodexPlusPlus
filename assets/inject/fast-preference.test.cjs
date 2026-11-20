const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const { it } = require("node:test");

const shardPath = require.resolve("./renderer-inject/92-fast-preference.js");

function fastPreferenceApi(globals = {}) {
  const source = fs.readFileSync(shardPath, "utf8");
  const context = vm.createContext(globals);
  vm.runInContext(`${source}\nglobalThis.__fastPreferenceTestApi = {
    read: codexPlusReadFastPreference,
    write: codexPlusWriteFastPreference,
    bridgeClient: codexPlusFastPreferenceClient,
  };`, context);
  return context.__fastPreferenceTestApi;
}

function plain(value) {
  return JSON.parse(JSON.stringify(value));
}

async function rejectsWithSafeMessage(promise, { match, hidden = [] } = {}) {
  let message = "";
  await assert.rejects(promise, (error) => {
    assert.equal(typeof error?.fastPreferenceMessage, "string");
    assert.equal(error.message, error.fastPreferenceMessage);
    message = error.fastPreferenceMessage;
    if (match) assert.match(message, match);
    for (const value of hidden) assert.equal(message.includes(value), false, `safe error leaked ${value}`);
    return true;
  });
  return message;
}

function readResult({ effective, layers }) {
  return { config: effective, layers };
}

function userLayer({ file = "/home/user/.codex/config.toml", version = "user-v1", config = {}, disabledReason = null, profile } = {}) {
  return {
    name: { type: "user", file, ...(profile === undefined ? {} : { profile }) },
    version,
    config,
    disabledReason,
  };
}

function readClient(result) {
  const calls = [];
  return {
    calls,
    client: {
      async sendRequest(method, params) {
        calls.push({ method, params: plain(params) });
        if (method !== "config/read") throw new Error(`unexpected RPC: ${method}`);
        return result;
      },
    },
  };
}

function writeClient({ writeResult = { status: "ok" }, readback, writeError, readError } = {}) {
  const calls = [];
  const writes = [];
  return {
    calls,
    writes,
    client: {
      async sendRequest(method, params) {
        calls.push({ method, params: plain(params) });
        if (method === "config/batchWrite") {
          writes.push(plain(params));
          if (writeError) throw writeError;
          return writeResult;
        }
        if (method === "config/read") {
          if (readError) throw readError;
          return readback;
        }
        throw new Error(`unexpected RPC: ${method}`);
      },
    },
  };
}

function bridgeFixture() {
  const listeners = new Map();
  const outgoing = [];
  const timers = new Map();
  const clearedTimers = [];
  let nextTimer = 1;
  let nextUuid = 1;
  const schedule = (callback, delay) => {
    const id = nextTimer++;
    timers.set(id, { callback, delay });
    return id;
  };
  const clear = (id) => {
    clearedTimers.push(id);
    timers.delete(id);
  };
  const window = {
    location: { origin: "app://-" },
    crypto: { randomUUID: () => `fast-preference-test-${nextUuid++}` },
    electronBridge: {
      sendMessageFromView(message) {
        outgoing.push(plain(message));
      },
    },
    setTimeout: schedule,
    clearTimeout: clear,
    addEventListener(type, listener) {
      if (!listeners.has(type)) listeners.set(type, new Set());
      listeners.get(type).add(listener);
    },
    removeEventListener(type, listener) {
      listeners.get(type)?.delete(listener);
    },
  };
  return {
    globals: { window, setTimeout: schedule, clearTimeout: clear },
    window,
    outgoing,
    timers,
    clearedTimers,
    listenerCount(type) { return listeners.get(type)?.size || 0; },
    emit(data, { source = window, origin = window.location.origin } = {}) {
      for (const listener of [...(listeners.get("message") || [])]) listener({ data, source, origin });
    },
    fireTimer(id) {
      const timer = timers.get(id);
      if (!timer) return false;
      timers.delete(id);
      timer.callback();
      return true;
    },
  };
}

const originalSnapshot = {
  tier: "fast",
  stored: "fast",
  file: "/home/user/.codex/config.toml",
  version: "user-v7",
};

it("reads effective and stored service tiers without writing, and normalizes the fast alias", async () => {
  const api = fastPreferenceApi();
  const rpc = readClient(readResult({
    effective: { model: "gpt-5.5", service_tier: "fast", approval_policy: "on-request" },
    layers: [
      { name: { type: "project", dotCodexFolder: "/repo/.codex" }, version: "project-v2", config: { service_tier: "flex" }, disabledReason: null },
      userLayer({
        file: originalSnapshot.file,
        version: originalSnapshot.version,
        config: { model_provider: "openai", service_tier: "fast", model_reasoning_effort: "high" },
      }),
      userLayer({ file: "/home/user/.codex/profile.toml", version: "profile-v1", profile: "work", config: { service_tier: "default" } }),
      userLayer({ file: "/home/user/.codex/disabled.toml", version: "disabled-v1", disabledReason: "disabled by policy", config: { service_tier: "flex" } }),
    ],
  }));

  const result = plain(await api.read(rpc.client));
  assert.deepEqual(result, {
    tier: "priority",
    stored: "priority",
    file: originalSnapshot.file,
    version: originalSnapshot.version,
  });
  assert.deepEqual(rpc.calls.map((call) => call.method), ["config/read"]);
  assert.equal(rpc.calls[0].params.includeLayers, true);
  assert.equal(rpc.calls[0].params.cwd, null);
});
it("keeps non-Fast tiers distinct and maps missing tiers to inheritance", async () => {
  const api = fastPreferenceApi();
  const flex = readClient(readResult({
    effective: { service_tier: "flex" },
    layers: [userLayer({ config: { service_tier: "flex" } })],
  }));
  assert.deepEqual(plain(await api.read(flex.client)), {
    tier: "flex", stored: "flex", file: "/home/user/.codex/config.toml", version: "user-v1",
  });

  const inherited = readClient(readResult({
    effective: {},
    layers: [userLayer({ config: {} })],
  }));
  assert.deepEqual(plain(await api.read(inherited.client)), {
    tier: null, stored: null, file: "/home/user/.codex/config.toml", version: "user-v1",
  });
});

it("ignores disabled and profile layers and rejects a missing or ambiguous user base layer", async () => {
  const api = fastPreferenceApi();
  const candidates = [
    userLayer({ profile: "work", config: { service_tier: "priority" } }),
    userLayer({ disabledReason: "disabled by policy", config: { service_tier: "priority" } }),
    { name: { type: "project", dotCodexFolder: "/repo/.codex" }, version: "project-v1", config: { service_tier: "flex" }, disabledReason: null },
    userLayer({ file: "/home/user/.codex/base.toml", version: "base-v3", config: { service_tier: "default" } }),
  ];
  const selected = readClient(readResult({ effective: { service_tier: "flex" }, layers: candidates }));
  assert.deepEqual(plain(await api.read(selected.client)), {
    tier: "flex", stored: "default", file: "/home/user/.codex/base.toml", version: "base-v3",
  });

  const missing = readClient(readResult({ effective: {}, layers: candidates.slice(0, 3) }));
  await rejectsWithSafeMessage(api.read(missing.client));

  const duplicate = readClient(readResult({ effective: {}, layers: [
    candidates[3], userLayer({ file: "/home/user/.codex/other.toml", version: "base-v4" }),
  ] }));
  await rejectsWithSafeMessage(api.read(duplicate.client));
});

it("writes only service_tier to the selected file with the snapshot version, then reads back", async () => {
  const api = fastPreferenceApi();
  const rpc = writeClient({
    writeResult: { status: "ok" },
    readback: readResult({
      effective: { model: "gpt-5.5", service_tier: "priority", model_provider: "openai" },
      layers: [userLayer({
        file: originalSnapshot.file,
        version: "user-v8",
        config: { service_tier: "priority", model_provider: "openai" },
      })],
    }),
  });

  const result = plain(await api.write(rpc.client, originalSnapshot, "priority"));
  assert.deepEqual(rpc.calls.map((call) => call.method), ["config/batchWrite", "config/read"]);
  assert.deepEqual(rpc.writes[0], {
    edits: [{ keyPath: "service_tier", value: "priority", mergeStrategy: "replace" }],
    filePath: originalSnapshot.file,
    expectedVersion: originalSnapshot.version,
    reloadUserConfig: true,
  });
  assert.deepEqual(result, {
    tier: "priority", stored: "priority", file: originalSnapshot.file, version: "user-v8", overridden: false,
  });
});

it("writes default literally and writes null to inherit", async () => {
  const api = fastPreferenceApi();
  const defaultRpc = writeClient({
    writeResult: { status: "ok" },
    readback: readResult({
      effective: { service_tier: "default" },
      layers: [userLayer({ file: originalSnapshot.file, version: "user-v8", config: { service_tier: "default" } })],
    }),
  });
  assert.equal(plain(await api.write(defaultRpc.client, originalSnapshot, "default")).stored, "default");
  assert.equal(defaultRpc.writes[0].edits[0].value, "default");

  const inheritRpc = writeClient({
    writeResult: { status: "ok" },
    readback: readResult({
      effective: { service_tier: "flex" },
      layers: [userLayer({ file: originalSnapshot.file, version: "user-v8", config: {} })],
    }),
  });
  const inherited = plain(await api.write(inheritRpc.client, originalSnapshot, null));
  assert.equal(inheritRpc.writes[0].edits[0].value, null);
  assert.equal(inheritRpc.writes[0].edits[0].mergeStrategy, "replace");
  assert.deepEqual(inherited, {
    tier: "flex", stored: null, file: originalSnapshot.file, version: "user-v8", overridden: false,
  });
});

it("accepts an explicit higher-layer override only when the user-layer value was saved", async () => {
  const api = fastPreferenceApi();
  const rpc = writeClient({
    writeResult: { status: "okOverridden" },
    readback: readResult({
      effective: { service_tier: "flex" },
      layers: [
        { name: { type: "project", dotCodexFolder: "/repo/.codex" }, version: "project-v2", config: { service_tier: "flex" }, disabledReason: null },
        userLayer({ file: originalSnapshot.file, version: "user-v8", config: { service_tier: "fast" } }),
      ],
    }),
  });

  const result = plain(await api.write(rpc.client, originalSnapshot, "priority"));
  assert.equal(result.tier, "flex");
  assert.equal(result.stored, "priority");
  assert.equal(result.overridden, true);
});

it("rejects effective or stored readback mismatches for status ok", async () => {
  const api = fastPreferenceApi();
  const effectiveMismatch = writeClient({
    writeResult: { status: "ok" },
    readback: readResult({
      effective: { service_tier: "flex" },
      layers: [userLayer({ file: originalSnapshot.file, version: "user-v8", config: { service_tier: "priority" } })],
    }),
  });
  await rejectsWithSafeMessage(api.write(effectiveMismatch.client, originalSnapshot, "priority"));

  const storedMismatch = writeClient({
    writeResult: { status: "okOverridden" },
    readback: readResult({
      effective: { service_tier: "priority" },
      layers: [userLayer({ file: originalSnapshot.file, version: "user-v8", config: { service_tier: "default" } })],
    }),
  });
  await rejectsWithSafeMessage(api.write(storedMismatch.client, originalSnapshot, "priority"));
});

it("rejects invalid desired tiers before writing and rejects unknown write statuses", async () => {
  const api = fastPreferenceApi();
  const invalid = writeClient();
  await rejectsWithSafeMessage(api.write(invalid.client, originalSnapshot, "fast"));
  assert.equal(invalid.calls.length, 0);

  const unknown = writeClient({ writeResult: { status: "unsupported" } });
  await rejectsWithSafeMessage(api.write(unknown.client, originalSnapshot, "priority"));
  assert.deepEqual(unknown.calls.map((call) => call.method), ["config/batchWrite"]);
});

it("propagates version conflicts and read/write RPC failures without retrying stale snapshots", async () => {
  const api = fastPreferenceApi();
  const conflictDetail = "/private/config.toml api_key=secret-version-conflict";
  const conflict = writeClient({ writeError: new Error(`config version conflict ${conflictDetail}`) });
  await rejectsWithSafeMessage(api.write(conflict.client, originalSnapshot, "priority"), {
    match: /其他窗口|版本/u,
    hidden: [conflictDetail, "secret-version-conflict"],
  });
  assert.equal(conflict.writes.length, 1);
  assert.deepEqual(conflict.calls.map((call) => call.method), ["config/batchWrite"]);

  const writeDetail = "/private/config.toml api_key=secret-write-detail";
  const writeFailure = writeClient({ writeError: new Error(`app-server unavailable ${writeDetail}`) });
  await rejectsWithSafeMessage(api.write(writeFailure.client, originalSnapshot, "default"), {
    hidden: [writeDetail, "secret-write-detail"],
  });

  const readDetail = "/private/config.toml api_key=secret-read-detail";
  const readFailure = { async sendRequest() { throw new Error(`config read unavailable ${readDetail}`); } };
  await rejectsWithSafeMessage(api.read(readFailure), {
    match: /无法读取本机速度设置/u,
    hidden: [readDetail, "secret-read-detail"],
  });

  const readbackDetail = "/private/config.toml api_key=secret-readback-detail";
  const readbackFailure = writeClient({
    writeResult: { status: "ok" },
    readError: new Error(`readback unavailable ${readbackDetail}`),
  });
  await rejectsWithSafeMessage(api.write(readbackFailure.client, originalSnapshot, null), {
    match: /未能回读核实/u,
    hidden: [readbackDetail, "secret-readback-detail"],
  });
});

it("creates a synchronous local app-server client and accepts only its matching bridge response", async () => {
  const bridge = bridgeFixture();
  const api = fastPreferenceApi(bridge.globals);
  const client = api.bridgeClient();
  assert.equal(client && typeof client.then, "undefined", "client discovery is synchronous");
  assert.equal(typeof client.sendRequest, "function");

  let settled = false;
  const request = client.sendRequest("config/read", { includeLayers: true, cwd: null });
  request.finally(() => { settled = true; }).catch(() => {});
  assert.equal(bridge.outgoing.length, 1);
  const sent = bridge.outgoing[0];
  assert.equal(sent.type, "mcp-request");
  assert.equal(sent.hostId, "local");
  assert.equal(sent.request.method, "config/read");
  assert.ok(sent.request.id != null);
  assert.deepEqual(sent.request.params, { includeLayers: true, cwd: null });
  assert.equal(sent.coalesce, false);
  const timer = [...bridge.timers.entries()][0];
  assert.ok(timer);
  assert.equal(timer[1].delay, 10_000);

  const message = (hostId, id, result = { config: { service_tier: "priority" } }) => ({
    type: "mcp-response", hostId, message: { id, result },
  });
  const wrongId = `${sent.request.id}-wrong`;
  bridge.emit(message("other-host", sent.request.id));
  bridge.emit(message("local", wrongId));
  bridge.emit(message("local", sent.request.id), { source: {}, origin: bridge.window.location.origin });
  bridge.emit(message("local", sent.request.id), { source: bridge.window, origin: "https://wrong.invalid" });
  await Promise.resolve();
  assert.equal(settled, false, "host, ID, source and origin mismatches leave the request pending");

  const expected = { config: { service_tier: "priority" } };
  bridge.emit(message("local", sent.request.id, expected));
  assert.deepEqual(plain(await request), expected);
  assert.equal(bridge.timers.has(timer[0]), false, "matching response clears the 10-second timeout");
  assert.ok(bridge.clearedTimers.includes(timer[0]));
  assert.equal(bridge.listenerCount("message"), 0, "matching response removes its listener");
  assert.equal(bridge.outgoing.length, 1, "a read request is sent only once");
});

it("cleans up a timed-out write and never resends it, even if a late response arrives", async () => {
  const bridge = bridgeFixture();
  const api = fastPreferenceApi(bridge.globals);
  const client = api.bridgeClient();
  const write = api.write(client, originalSnapshot, "priority");
  const rejection = rejectsWithSafeMessage(write, { match: /保存结果/u });
  assert.equal(bridge.outgoing.length, 1);
  const sent = bridge.outgoing[0];
  assert.equal(sent.type, "mcp-request");
  assert.equal(sent.hostId, "local");
  assert.equal(sent.request.method, "config/batchWrite");
  const timer = [...bridge.timers.entries()][0];
  assert.ok(timer);
  assert.equal(timer[1].delay, 10_000);

  assert.equal(bridge.fireTimer(timer[0]), true);
  await rejection;
  assert.equal(bridge.timers.size, 0, "timeout removes the pending timer");
  assert.ok(bridge.clearedTimers.includes(timer[0]));
  assert.equal(bridge.listenerCount("message"), 0, "timeout removes its listener");
  bridge.emit({
    type: "mcp-response", hostId: "local",
    message: { id: sent.request.id, result: { status: "ok" } },
  });
  await Promise.resolve();
  assert.equal(bridge.outgoing.length, 1, "a timeout never repeats config/batchWrite");
  assert.deepEqual(bridge.outgoing.map((message) => message.request.method), ["config/batchWrite"]);
});

it("propagates one bridge RPC failure without leaking it or repeating the write", async () => {
  const bridge = bridgeFixture();
  const api = fastPreferenceApi(bridge.globals);
  const client = api.bridgeClient();
  const write = api.write(client, originalSnapshot, "default");
  const rejection = rejectsWithSafeMessage(write, { hidden: ["secret-bridge-payload"] });
  const sent = bridge.outgoing[0];
  bridge.emit({
    type: "mcp-response", hostId: "local",
    message: { id: sent.request.id, error: { code: -32000, message: "secret-bridge-payload" } },
  });
  await rejection;
  assert.equal(bridge.outgoing.length, 1);
  assert.deepEqual(bridge.outgoing.map((message) => message.request.method), ["config/batchWrite"]);
  assert.equal(bridge.timers.size, 0, "RPC errors clean up the pending timer");
  assert.equal(bridge.listenerCount("message"), 0, "RPC errors remove the response listener");
});
