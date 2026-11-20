  /* Fast 偏好弹窗：只通过本机 app-server 的 Fast preference helpers 读写。 */
  const codexPlusFastDialogKey = "__codexPlusFastPreferenceDialog";
  const codexPlusFastDialogStyleId = "codex-plus-fast-preference-dialog-style";

  // 整页重新注入时，先关闭旧作用域持有的弹窗并使其异步回调失效。
  try { if (typeof window[codexPlusFastDialogKey] === "function") window[codexPlusFastDialogKey](false); } catch {}
  let codexPlusFastDialogState = null;

  const codexPlusFastChoices = [
    ["inherit", null, "跟随默认", "保留 Codex 的默认速度选择。"],
    ["fast", "priority", "Fast", "将 Fast 设为本机 Codex 的默认速度。"],
    ["standard", "default", "标准", "将标准速度设为本机 Codex 的默认速度。"],
  ];

  function codexPlusFastDialogEnabled() {
    return typeof codexPlusComposerFastPreferenceEnabled === "function"
      && codexPlusComposerFastPreferenceEnabled() === true;
  }

  function codexPlusFastDialogCurrent(state, operation = null) {
    if (!state || state.closed || codexPlusFastDialogState !== state
        || window[codexPlusFastDialogKey] !== state.close) return false;
    if (state.generation !== window.__codexPlusBackendGeneration || !codexPlusFastDialogEnabled()) {
      state.close();
      return false;
    }
    return operation === null || operation === state.operation;
  }

  function codexPlusFastDialogInstallStyle() {
    if (document.getElementById(codexPlusFastDialogStyleId)) return;
    const style = document.createElement("style");
    style.id = codexPlusFastDialogStyleId;
    if (typeof markCodexPlusExtensionNode === "function") markCodexPlusExtensionNode(style, "builtin-fast-preference");
    else style.setAttribute("data-codex-plus-ext", "builtin-fast-preference");
    style.textContent = `
      dialog.codex-plus-fast-preference { width:min(520px,calc(100vw - 32px)); max-width:calc(100vw - 32px); max-height:calc(100dvh - 32px); padding:0; border:0; overflow:visible; background:transparent; color:inherit; }
      dialog.codex-plus-fast-preference::backdrop { background:var(--modal-backdrop-dim-shadow,rgba(0,0,0,.32)); backdrop-filter:blur(1px); }
      dialog.codex-plus-fast-preference .codex-plus-modal-content { box-sizing:border-box; width:100%; max-width:100%; max-height:calc(100dvh - 32px); }
      dialog.codex-plus-fast-preference .codex-plus-modal-header { padding:16px 20px 10px; }
      dialog.codex-plus-fast-preference .codex-plus-modal-title { font-size:16px; }
      .codex-plus-fast-preference-body { display:grid; gap:12px; padding:8px 20px 18px; overflow:auto; }
      .codex-plus-fast-preference-copy { margin:0; color:var(--codex-plus-text-secondary); line-height:1.5; }
      .codex-plus-fast-preference-status { display:grid; gap:3px; padding:10px 12px; border:1px solid var(--codex-plus-border); border-radius:var(--border-radius-lg,8px); }
      .codex-plus-fast-preference-status p { margin:0; }
      .codex-plus-fast-preference-effective { color:var(--codex-plus-text-secondary); }
      .codex-plus-fast-preference-options { display:grid; gap:7px; min-width:0; margin:0; padding:0; border:0; }
      .codex-plus-fast-preference-options legend { margin-bottom:5px; padding:0; font-weight:600; }
      .codex-plus-fast-preference-option { display:flex; align-items:flex-start; gap:9px; padding:9px 11px; border:1px solid var(--codex-plus-border); border-radius:var(--border-radius-lg,8px); cursor:pointer; }
      .codex-plus-fast-preference-option[data-selected="true"] { border-color:var(--color-token-text-accent,var(--codex-plus-text-accent,#4267e8)); background:var(--codex-plus-bg-selected); }
      .codex-plus-fast-preference-option input { flex:0 0 auto; margin:3px 0 0; accent-color:var(--color-token-text-accent,var(--codex-plus-text-accent,#4267e8)); }
      .codex-plus-fast-preference-option-copy { display:grid; gap:2px; }
      .codex-plus-fast-preference-option-detail { color:var(--codex-plus-text-secondary); }
      .codex-plus-fast-preference-message { min-height:1.4em; margin:0; color:var(--codex-plus-text-secondary); white-space:pre-wrap; }
      .codex-plus-fast-preference-message[data-tone="error"] { color:var(--codex-plus-danger,#b42318); }
      .codex-plus-fast-preference-actions { display:flex; justify-content:flex-end; gap:8px; }
      .codex-plus-fast-preference-actions button { min-height:32px; padding:5px 12px; border:1px solid var(--codex-plus-border); border-radius:var(--border-radius-lg,8px); background:var(--codex-plus-bg-secondary); color:var(--codex-plus-text); font:inherit; cursor:pointer; }
      .codex-plus-fast-preference-actions button:hover:not(:disabled) { background:var(--codex-plus-bg-hover); }
      .codex-plus-fast-preference-actions button[data-primary="true"] { border-color:var(--codex-plus-text-accent,#4267e8); background:var(--codex-plus-text-accent,#4267e8); color:var(--codex-plus-text-on-accent,#fff); }
      .codex-plus-fast-preference-actions button:disabled { cursor:default; opacity:.55; }
      dialog.codex-plus-fast-preference :focus-visible { outline:2px solid var(--color-token-text-accent,var(--codex-plus-text-accent,#4267e8)); outline-offset:2px; }
      @media (max-width:480px) { dialog.codex-plus-fast-preference { width:calc(100vw - 24px); max-width:calc(100vw - 24px); } dialog.codex-plus-fast-preference .codex-plus-modal-header { padding:14px 16px 8px; } .codex-plus-fast-preference-body { padding:8px 16px 16px; } }
    `;
    (document.head || document.documentElement).append(style);
  }

  function codexPlusFastDialogChoice(value) {
    if (value === null) return "inherit";
    if (typeof value !== "string") return "";
    const normalized = value.toLowerCase();
    return normalized === "priority" ? "fast" : normalized === "default" ? "standard" : "";
  }

  function codexPlusFastDialogLabel(value) {
    if (value === null) return "由 Codex / 模型决定";
    if (typeof value !== "string") return "未确认";
    switch (value.toLowerCase()) {
      case "priority":
      case "fast": return "Fast";
      case "default":
      case "standard": return "标准";
      case "ultrafast":
      case "ultra-fast":
      case "ultra_fast": return "Ultra Fast（超快）";
      case "flex": return "Flex（灵活）";
      default: return "其他档位";
    }
  }

  function codexPlusFastDialogSafeError(error) {
    const message = typeof error?.fastPreferenceMessage === "string"
      ? error.fastPreferenceMessage.trim().replace(/[\u0000-\u001f\u007f]/g, " ").slice(0, 240)
      : "";
    return message || "读取或保存 Fast 速度偏好失败，请重试。";
  }

  function codexPlusFastDialogUsable(snapshot) {
    return !!snapshot && typeof snapshot === "object"
      && Object.prototype.hasOwnProperty.call(snapshot, "tier")
      && (snapshot.stored === null || typeof snapshot.stored === "string");
  }

  function codexPlusFastDialogMessage(state, text, tone = "") {
    const node = state.dialog.querySelector("[data-fast-pref-message]");
    if (!node) return;
    node.textContent = text;
    node.setAttribute("role", tone === "error" ? "alert" : "status");
    if (tone) node.dataset.tone = tone;
    else delete node.dataset.tone;
  }

  function codexPlusFastDialogBusy(state, busy) {
    state.busy = busy;
    state.dialog.setAttribute("aria-busy", String(busy));
    state.dialog.querySelectorAll("input[name='codex-plus-fast-choice']").forEach((input) => {
      input.disabled = busy || !state.snapshot;
    });
    const apply = state.dialog.querySelector("[data-fast-pref-apply]");
    if (apply) apply.disabled = busy || !state.snapshot || state.desired === state.snapshot.stored;
    const retry = state.dialog.querySelector("[data-fast-pref-retry]");
    if (retry) retry.hidden = !state.needsRetry;
  }

  function codexPlusFastDialogRender(state, snapshot, overridden = false) {
    state.snapshot = snapshot;
    state.desired = snapshot.stored;
    state.needsRetry = false;
    const saved = snapshot.stored === null ? "跟随默认" : codexPlusFastDialogLabel(snapshot.stored);
    const effective = codexPlusFastDialogLabel(snapshot.tier);
    const overriddenNow = overridden === true || (snapshot.stored != null && snapshot.stored !== snapshot.tier);
    state.dialog.querySelector("[data-fast-pref-saved]").textContent = `保存的选择：${saved}。`;
    state.dialog.querySelector("[data-fast-pref-effective]").textContent = overriddenNow
      ? `当前配置：${effective}（被更高优先级配置覆盖）。`
      : `当前配置：${effective}。`;
    const selected = codexPlusFastDialogChoice(snapshot.stored);
    state.dialog.querySelectorAll("input[name='codex-plus-fast-choice']").forEach((input) => {
      input.checked = input.value === selected;
      input.closest("label")?.setAttribute("data-selected", String(input.checked));
    });
    codexPlusFastDialogMessage(state, overriddenNow ? "已保存偏好；当前配置由更高优先级配置决定。" : "", overriddenNow ? "success" : "");
    codexPlusFastDialogBusy(state, false);
  }

  function codexPlusFastDialogCloseState(state, restoreFocus = true) {
    if (!state || state.closed) return;
    state.closed = true;
    state.operation += 1;
    state.observer?.disconnect();
    state.dialog.removeEventListener("cancel", state.onCancel);
    state.dialog.removeEventListener("close", state.onNativeClose);
    state.dialog.removeEventListener("click", state.onDialogClick);
    window.removeEventListener("keydown", state.onEscape, true);
    if (state.dialog.open) { try { state.dialog.close(); } catch {} }
    state.dialog.remove();
    if (window[codexPlusFastDialogKey] === state.close) delete window[codexPlusFastDialogKey];
    if (codexPlusFastDialogState === state) codexPlusFastDialogState = null;
    if (!restoreFocus || state.generation !== window.__codexPlusBackendGeneration) return;
    const target = state.button?.isConnected ? state.button : state.previousFocus?.isConnected ? state.previousFocus : null;
    try { target?.focus?.({ preventScroll: true }); } catch { try { target?.focus?.(); } catch {} }
  }

  async function codexPlusFastDialogRead(state, notice = "") {
    if (!codexPlusFastDialogCurrent(state)) return;
    const operation = ++state.operation;
    state.snapshot = null;
    state.desired = null;
    state.needsRetry = false;
    codexPlusFastDialogBusy(state, true);
    state.dialog.querySelector("[data-fast-pref-saved]").textContent = "保存的选择：读取中…";
    state.dialog.querySelector("[data-fast-pref-effective]").textContent = "当前配置：读取中…";
    codexPlusFastDialogMessage(state, notice || "正在读取本机速度偏好…", notice ? "error" : "");
    try {
      if (typeof codexPlusFastPreferenceClient !== "function" || typeof codexPlusReadFastPreference !== "function") throw new Error();
      const client = state.client || codexPlusFastPreferenceClient();
      if (!codexPlusFastDialogCurrent(state, operation)) return;
      if (!client) throw new Error();
      state.client = client;
      const snapshot = await codexPlusReadFastPreference(client);
      if (!codexPlusFastDialogCurrent(state, operation)) return;
      if (!codexPlusFastDialogUsable(snapshot)) throw new Error();
      codexPlusFastDialogRender(state, snapshot);
      if (notice) codexPlusFastDialogMessage(state, notice, "error");
    } catch (error) {
      if (!codexPlusFastDialogCurrent(state, operation)) return;
      state.snapshot = null;
      state.desired = null;
      state.needsRetry = true;
      state.dialog.querySelector("[data-fast-pref-saved]").textContent = "保存的选择：暂不可读取。";
      state.dialog.querySelector("[data-fast-pref-effective]").textContent = "当前配置：暂不可读取。";
      codexPlusFastDialogMessage(state, notice ? `${notice} ${codexPlusFastDialogSafeError(error)}` : codexPlusFastDialogSafeError(error), "error");
      codexPlusFastDialogBusy(state, false);
    }
  }

  async function codexPlusFastDialogApply(state) {
    if (!codexPlusFastDialogCurrent(state) || state.busy || !state.snapshot
        || state.desired === state.snapshot.stored) return;
    if (typeof codexPlusWriteFastPreference !== "function" || !state.client) {
      await codexPlusFastDialogRead(state, "本机速度偏好服务暂不可用，请重试。");
      return;
    }
    const operation = ++state.operation;
    const snapshot = state.snapshot;
    const desired = state.desired;
    state.snapshot = null;
    state.needsRetry = false;
    codexPlusFastDialogBusy(state, true);
    codexPlusFastDialogMessage(state, "正在保存设置…");
    try {
      const result = await codexPlusWriteFastPreference(state.client, snapshot, desired);
      if (!codexPlusFastDialogCurrent(state, operation)) return;
      if (!codexPlusFastDialogUsable(result)) throw new Error();
      codexPlusFastDialogRender(state, result, result.overridden === true);
      codexPlusFastDialogMessage(state, result.overridden === true
        ? "已保存选择，但当前配置仍由更高优先级配置决定。"
        : "速度偏好已保存。", "success");
    } catch (error) {
      if (!codexPlusFastDialogCurrent(state, operation)) return;
      await codexPlusFastDialogRead(state, `保存未完成。${codexPlusFastDialogSafeError(error)}`);
    }
  }

  function codexPlusFastDialogCreate(button) {
    const dialog = document.createElement("dialog");
    dialog.className = "codex-plus-fast-preference";
    dialog.setAttribute("aria-labelledby", "codex-plus-fast-dialog-title");
    dialog.setAttribute("aria-describedby", "codex-plus-fast-dialog-copy");
    dialog.innerHTML = `
      <div class="codex-plus-modal-content">
        <header class="codex-plus-modal-header">
          <h2 class="codex-plus-modal-title" id="codex-plus-fast-dialog-title">Fast 速度偏好</h2>
          <button class="codex-plus-modal-close" type="button" data-fast-pref-close aria-label="关闭">×</button>
        </header>
        <div class="codex-plus-fast-preference-body">
          <p class="codex-plus-fast-preference-copy" id="codex-plus-fast-dialog-copy">设置本机 Codex 的默认速度。实际档位与费用取决于模型和供应商；更改偏好本身不会改变当前速度。</p>
          <section class="codex-plus-fast-preference-status" aria-label="速度偏好与当前配置">
            <p data-fast-pref-saved>保存的选择：读取中…</p>
            <p class="codex-plus-fast-preference-effective" data-fast-pref-effective>当前配置：读取中…</p>
          </section>
          <fieldset class="codex-plus-fast-preference-options" data-fast-pref-options>
            <legend>保存的默认偏好</legend>
            ${codexPlusFastChoices.map(([value, , label, detail]) => `
              <label class="codex-plus-fast-preference-option">
                <input type="radio" name="codex-plus-fast-choice" value="${value}">
                <span class="codex-plus-fast-preference-option-copy"><strong>${label}</strong><span class="codex-plus-fast-preference-option-detail">${detail}</span></span>
              </label>`).join("")}
          </fieldset>
          <p class="codex-plus-fast-preference-message" data-fast-pref-message role="status" aria-live="polite"></p>
          <footer class="codex-plus-fast-preference-actions">
            <button type="button" data-fast-pref-retry hidden>重新读取</button>
            <button type="button" data-fast-pref-close>关闭</button>
            <button type="button" data-fast-pref-apply data-primary="true" disabled>保存</button>
          </footer>
        </div>
      </div>`;
    if (typeof markCodexPlusExtensionNode === "function") markCodexPlusExtensionNode(dialog, "builtin-fast-preference");
    else dialog.setAttribute("data-codex-plus-ext", "builtin-fast-preference");

    const state = {
      button, dialog, generation: codexPlusBackendGeneration, previousFocus: document.activeElement,
      snapshot: null, client: null, desired: null, operation: 0, busy: false, closed: false, needsRetry: false, observer: null,
      close: (restoreFocus = true) => codexPlusFastDialogCloseState(state, restoreFocus),
    };
    dialog.querySelectorAll("input[name='codex-plus-fast-choice']").forEach((input) => input.addEventListener("change", () => {
      if (!codexPlusFastDialogCurrent(state) || state.busy || !state.snapshot) return;
      state.desired = codexPlusFastChoices.find(([value]) => value === input.value)?.[1];
      dialog.querySelectorAll(".codex-plus-fast-preference-option").forEach((label) => {
        label.dataset.selected = String(label.querySelector("input")?.checked === true);
      });
      codexPlusFastDialogBusy(state, false);
      codexPlusFastDialogMessage(state, "");
    }));
    dialog.querySelector("[data-fast-pref-apply]").addEventListener("click", () => { void codexPlusFastDialogApply(state); });
    dialog.querySelector("[data-fast-pref-retry]").addEventListener("click", () => { void codexPlusFastDialogRead(state); });
    dialog.querySelectorAll("[data-fast-pref-close]").forEach((close) => close.addEventListener("click", () => state.close()));
    state.onCancel = (event) => { event.preventDefault(); state.close(); };
    state.onEscape = (event) => {
      if (event.key !== "Escape" || state.closed || !dialog.open) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      state.close();
    };
    state.onNativeClose = () => { if (!state.closed) state.close(); };
    state.onDialogClick = (event) => { if (event.target === dialog) state.close(); };
    dialog.addEventListener("cancel", state.onCancel);
    dialog.addEventListener("close", state.onNativeClose);
    dialog.addEventListener("click", state.onDialogClick);
    window.addEventListener("keydown", state.onEscape, true);
    return state;
  }

  function closeCodexPlusFastPreference() {
    if (codexPlusFastDialogState) codexPlusFastDialogState.close();
    else if (typeof window[codexPlusFastDialogKey] === "function") window[codexPlusFastDialogKey]();
  }

  async function openCodexPlusFastPreference(button) {
    if (!button || !codexPlusFastDialogEnabled()
        || codexPlusBackendGeneration !== window.__codexPlusBackendGeneration) return;
    const current = codexPlusFastDialogState;
    if (current && !current.closed && current.dialog?.open
        && current.generation === window.__codexPlusBackendGeneration) {
      codexPlusFastDialogState = current;
      (current.dialog.querySelector("input[name='codex-plus-fast-choice']:checked")
        || current.dialog.querySelector("[data-fast-pref-close]"))?.focus?.({ preventScroll: true });
      return;
    }
    current?.close?.(false);
    codexPlusFastDialogState = null;
    document.getElementById("codex-plus-fast-preference-dialog")?.remove();
    codexPlusFastDialogInstallStyle();
    const state = codexPlusFastDialogCreate(button);
    state.dialog.id = "codex-plus-fast-preference-dialog";
    codexPlusFastDialogState = state;
    window[codexPlusFastDialogKey] = state.close;
    document.body.append(state.dialog);
    try { state.dialog.showModal(); } catch { state.close(false); return; }
    state.observer = new MutationObserver(() => {
      if (state.generation !== window.__codexPlusBackendGeneration || !codexPlusFastDialogEnabled()) state.close();
    });
    state.observer.observe(document.body, { childList: true, subtree: true });
    void codexPlusFastDialogRead(state);
  }
