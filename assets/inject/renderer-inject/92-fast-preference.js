  // 只经本机 app-server 写默认档位；当前会话、模型支持和账户权限仍由 Codex 决定。
  // 26.1117 的 preload 直接投递本窗口的 mcp-response，不需要从前端分块寻找客户端。
  function codexPlusFastPreferenceClient() {
    const host = window.electronBridge;
    if (typeof host?.sendMessageFromView !== "function") {
      throw codexPlusFastPreferenceError("本机 Codex 配置服务尚未连接，请重新读取。");
    }
    async function sendRequest(method, params) {
      if (method !== "config/read" && method !== "config/batchWrite") throw new Error("Invalid method");
      const nonce = window.crypto.randomUUID?.() ?? window.crypto.getRandomValues(new Uint32Array(4)).join("-");
      const id = `${method}:codex-plus-${nonce}`;
      return new Promise((resolve, reject) => {
        function cleanup() {
          window.clearTimeout(timer);
          window.removeEventListener("message", receive);
        }
        function fail(error) {
          cleanup();
          reject(error);
        }
        function receive(event) {
          const data = event.data;
          if (event.source !== window || event.origin !== window.location.origin
              || data?.type !== "mcp-response" || data.hostId !== "local" || data.message?.id !== id) return;
          cleanup();
          if (data.message.error != null) reject(data.message.error);
          else resolve(data.message.result);
        }
        const timer = window.setTimeout(() => fail(new Error("No config response")), 10_000);
        window.addEventListener("message", receive);
        try {
          Promise.resolve(host.sendMessageFromView({
            type: "mcp-request", hostId: "local", request: { id, method, params }, coalesce: false,
          })).catch(fail);
        } catch (error) {
          fail(error);
        }
      });
    }
    return { sendRequest };
  }

  function codexPlusFastPreferenceError(message) {
    const error = new Error(message);
    error.fastPreferenceMessage = message;
    return error;
  }

  function codexPlusFastPreferenceTier(config) {
    if (!config || typeof config !== "object" || Array.isArray(config)) {
      throw codexPlusFastPreferenceError("Codex 返回的速度配置不完整。");
    }
    const tier = config.service_tier;
    if (tier == null) return null;
    if (typeof tier !== "string") throw codexPlusFastPreferenceError("无法识别当前速度配置。");
    return tier === "fast" ? "priority" : tier;
  }

  async function codexPlusReadFastPreference(client) {
    let response;
    try {
      response = await client.sendRequest("config/read", { includeLayers: true, cwd: null });
    } catch {
      throw codexPlusFastPreferenceError("无法读取本机速度设置，请重新读取。");
    }
    const users = Array.isArray(response?.layers) ? response.layers.filter((layer) =>
      layer?.name?.type === "user" && layer.name.profile == null && layer.disabledReason == null) : [];
    const user = users.length === 1 ? users[0] : null;
    if (typeof user?.name?.file !== "string" || !user.name.file.trim()
        || typeof user.version !== "string" || !user.version.trim()) {
      throw codexPlusFastPreferenceError("Codex 未提供唯一可写的用户默认配置及版本。");
    }
    return {
      tier: codexPlusFastPreferenceTier(response.config),
      stored: codexPlusFastPreferenceTier(user.config),
      file: user.name.file,
      version: user.version,
    };
  }

  async function codexPlusWriteFastPreference(client, snapshot, desired) {
    if (![null, "default", "priority"].includes(desired)
        || typeof snapshot?.file !== "string" || !snapshot.file
        || typeof snapshot.version !== "string" || !snapshot.version) {
      throw codexPlusFastPreferenceError("速度选项或配置版本无效，请重新读取。");
    }
    let ack;
    try {
      ack = await client.sendRequest("config/batchWrite", {
        edits: [{ keyPath: "service_tier", value: desired, mergeStrategy: "replace" }],
        filePath: snapshot.file,
        expectedVersion: snapshot.version,
        reloadUserConfig: true,
      });
    } catch (error) {
      const code = error?.data?.config_write_error_code ?? error?.error?.data?.config_write_error_code;
      if (/config.?version.?conflict/i.test(String(code))
          || /version conflict|modified since last read/i.test(String(error?.message))) {
        throw codexPlusFastPreferenceError("配置版本已变更，请重新读取后再选择速度。");
      }
      throw codexPlusFastPreferenceError("Codex 未确认保存结果，请重新读取当前设置。");
    }
    if (ack?.status !== "ok" && ack?.status !== "okOverridden") {
      throw codexPlusFastPreferenceError("Codex 未确认保存结果，请重新读取当前设置。");
    }
    let actual;
    try {
      actual = await codexPlusReadFastPreference(client);
    } catch {
      throw codexPlusFastPreferenceError("Codex 已接收写入，但未能回读核实，请重新读取。");
    }
    const overridden = ack.status === "okOverridden";
    if (actual.file !== snapshot.file || actual.stored !== desired
        || (!overridden && desired !== null && actual.tier !== desired)) {
      throw codexPlusFastPreferenceError("回读的速度设置与写入不一致，请重新读取。");
    }
    return { ...actual, overridden };
  }
