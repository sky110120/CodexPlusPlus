(() => {
  if (window.top !== window || window.self !== window || !window.electronBridge || !/^app:\/\/\-\//i.test(window.location.href)) return;
  if (window.__codexPlusUserScriptsBootstrap) return;
  window.__codexPlusUserScriptsBootstrap = true;
  const maxRetries = 3;
  const retryDelayMs = 100;
  const retry = (attempt, error) => {
    console.warn("[Codex++] user scripts:", error);
    if (attempt >= maxRetries) return;
    window.setTimeout(() => load(attempt + 1), retryDelayMs);
  };
  const load = (attempt = 0) => {
    // 每次页面加载都读取当前文件与开关，不保留启动时的旧脚本副本。
    const bridge = window.__codexSessionDeleteBridge;
    if (typeof bridge !== "function") {
      retry(attempt, new Error("script loading bridge is not ready"));
      return;
    }
    let request;
    try {
      request = bridge.call(window, "/user-scripts/load", {});
    } catch (error) {
      retry(attempt, error);
      return;
    }
    Promise.resolve(request).then((result) => {
      if (result?.status === "failed") retry(attempt, result.message || "script loading failed");
    }, (error) => retry(attempt, error));
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", () => load(), { once: true });
  else load();
})();
