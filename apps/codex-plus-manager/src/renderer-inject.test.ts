import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { readFile } from "node:fs/promises";

type FakeElementOptions = {
  className?: string;
  dismissLabel?: string;
  hasProgress?: boolean;
  styleDisplay?: string;
};

class FakeElement {
  children: FakeElement[] = [];
  dataset: Record<string, string> = {};
  parentElement: FakeElement | null = null;
  style: { display: string };
  private readonly className: string;
  private readonly dismissLabel: string;
  private readonly hasProgress: boolean;

  constructor(options: FakeElementOptions = {}) {
    this.className = options.className ?? "";
    this.dismissLabel = options.dismissLabel ?? "";
    this.hasProgress = options.hasProgress ?? false;
    this.style = { display: options.styleDisplay ?? "" };
  }

  appendChild(child: FakeElement) {
    child.parentElement = this;
    this.children.push(child);
  }

  getAttribute(name: string) {
    return name === "aria-label" ? this.dismissLabel : null;
  }

  matches(selector: string) {
    return selector === "div.w-full" && this.className.split(/\s+/).includes("w-full");
  }

  querySelector(selector: string) {
    return selector === 'progress[max="100"]' && this.hasProgress ? new FakeElement() : null;
  }

  querySelectorAll(selector: string) {
    return selector === "button" && this.dismissLabel ? [this] : [];
  }
}

function usageAlertRuntime(renderer: string, cards: FakeElement[], managed: FakeElement[]) {
  const start = renderer.indexOf("  function officialUsageAlertHidden(");
  const end = renderer.indexOf("\n  let zedRemoteStatusPromise", start);
  assert.ok(start >= 0 && end > start);
  const source = renderer.slice(start, end);
  const selectors: string[] = [];
  const document = {
    querySelectorAll(selector: string) {
      selectors.push(selector);
      return selector === '[data-codex-plus-usage-alert-hidden="true"]'
        ? managed.filter((node) => node.dataset.codexPlusUsageAlertHidden === "true")
        : cards;
    },
  };
  const windowValue: Record<string, unknown> = {};
  const create = new Function(
    "window",
    "document",
    "HTMLElement",
    `${source}\nreturn { officialUsageAlertHidden, refreshOfficialUsageAlertVisibility };`,
  ) as (
    windowValue: Record<string, unknown>,
    documentValue: typeof document,
    elementType: typeof FakeElement,
  ) => {
    officialUsageAlertHidden: () => boolean;
    refreshOfficialUsageAlertVisibility: () => void;
  };
  return { runtime: create(windowValue, document, FakeElement), selectors, windowValue };
}

function installRendererStyle(renderer: string) {
  const start = renderer.indexOf("  function installStyle()");
  const end = renderer.indexOf("\n  function defaultCodexPlusSettings", start);
  assert.ok(start >= 0 && end > start);
  const source = renderer.slice(start, end);
  const requiredNames = new Set([
    "styleId",
    "codexDeleteStyleVersion",
    ...Array.from(source.matchAll(/\$\{([A-Za-z_$][A-Za-z0-9_$]*)/g), (match) => match[1]),
  ]);
  const declarations = Array.from(requiredNames, (name) => {
    const declaration = renderer.match(new RegExp(`^  const ${name} = .+;$`, "m"))
      ?? renderer.match(new RegExp(`^  const ${name} = [\\s\\S]*?^  };$`, "m"));
    assert.ok(declaration, `missing renderer declaration for ${name}`);
    return declaration[0];
  }).join("\n");
  const appended: Array<{ dataset: Record<string, string>; id?: string; textContent?: string }> = [];
  const document = {
    getElementById() {
      return null;
    },
    createElement() {
      return { dataset: {} };
    },
    documentElement: {
      appendChild(node: (typeof appended)[number]) {
        appended.push(node);
      },
    },
  };
  const install = new Function("document", `${declarations}\n${source}\ninstallStyle();`) as (documentValue: typeof document) => void;

  install(document);
  return appended;
}

function themeSyncRuntime(renderer: string) {
  const start = renderer.indexOf("  function codexPlusHostUsesLightTheme()");
  const end = renderer.indexOf("\n  function openCodexPlusModal()", start);
  assert.ok(start >= 0 && end > start);
  const source = renderer.slice(start, end);
  const themeNode = () => {
    const attributes = new Map<string, string>();
    return {
      attributes,
      className: "",
      getAttribute(name: string) {
        return attributes.get(name) ?? null;
      },
    };
  };
  const root = themeNode();
  const body = themeNode();
  const observers: FakeThemeObserver[] = [];
  class FakeThemeObserver {
    callback: () => void;
    disconnected = false;
    observed: Array<{ target: unknown; options: Record<string, unknown> }> = [];

    constructor(callback: () => void) {
      this.callback = callback;
      observers.push(this);
    }

    observe(target: unknown, options: Record<string, unknown>) {
      this.observed.push({ target, options });
    }

    disconnect() {
      this.disconnected = true;
    }
  }
  const mediaHandlers = new Set<() => void>();
  let legacyAdds = 0;
  const mediaQuery = {
    matches: false,
    addEventListener(_name: string, handler: () => void) {
      mediaHandlers.add(handler);
    },
    removeEventListener(_name: string, handler: () => void) {
      mediaHandlers.delete(handler);
    },
    addListener() {
      legacyAdds += 1;
    },
    removeListener() {
      legacyAdds -= 1;
    },
  };
  const document = {
    documentElement: root,
    body,
    querySelectorAll() {
      return [];
    },
  };
  const windowValue = {
    matchMedia() {
      return mediaQuery;
    },
  };
  const create = new Function(
    "window",
    "document",
    "MutationObserver",
    "getComputedStyle",
    `${source}\nreturn { installCodexPlusThemeSync, removeCodexPlusModalOverlay };`,
  ) as (
    windowArg: typeof windowValue,
    documentArg: typeof document,
    observerArg: typeof FakeThemeObserver,
    computedStyleArg: () => { colorScheme: string },
  ) => {
    installCodexPlusThemeSync: (overlay: ThemeOverlay) => void;
    removeCodexPlusModalOverlay: (overlay: ThemeOverlay) => void;
  };
  const runtime = create(windowValue, document, FakeThemeObserver, () => ({ colorScheme: "" }));
  return { body, legacyAdds: () => legacyAdds, mediaHandlers, mediaQuery, observers, root, runtime };
}

type ThemeOverlay = {
  __codexPlusThemeCleanup?: (() => void) | null;
  dataset: Record<string, string>;
  isConnected: boolean;
  remove: () => void;
  style: { setProperty: (name: string, value: string) => void };
  values: Record<string, string>;
};

function createThemeOverlay(): ThemeOverlay {
  const values: Record<string, string> = {};
  return {
    dataset: {},
    isConnected: true,
    remove() {
      this.isConnected = false;
    },
    style: {
      setProperty(name: string, value: string) {
        values[name] = value;
      },
    },
    values,
  };
}

describe("renderer injection header compatibility", () => {
  it("adds the session copy shortcut through the native fork action", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /原地复制会话 - Codex\+\+/);
    assert.match(renderer, /createSessionMoreMenuItem\("原地复制会话 - Codex\+\+"/);
    assert.match(renderer, /getAttribute\("aria-label"\)[\s\S]*聊天操作/);
    assert.match(renderer, /从这里创建聊天分支/);
    assert.match(renderer, /data-app-action-sidebar-thread-selected/);
    assert.match(renderer, /sessionCopyMenuActivationTimeoutMs/);
    assert.doesNotMatch(renderer, /\n\s*refreshSessionCopyMenuItems\(\);/);
  });

  it("automatically renames a session through the native title suggestion", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /自动重命名当前会话/);
    assert.match(renderer, /activateSessionAutoRenameMenuItem/);
    assert.match(renderer, /input\[aria-label="聊天标题"\], input\[aria-label="Chat title"\]/);
    assert.match(renderer, /button\.classList\.contains\("text-info"\)/);
    assert.match(renderer, /\^\(保存\|Save\)\$/);
    assert.match(renderer, /Codex 未能生成新名称/);
  });

  it("anchors the Codex++ menu to current and legacy application top bars only", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /appHeader:\s*'[^"]*\[class\*="ApplicationMenuTopBar"\][^']*\.app-header-tint'/);
    assert.doesNotMatch(renderer, /document\.querySelector\(["']header["']\)/);
    assert.match(renderer, /isApplicationMenuTopBar\s*\?\s*Math\.max\(4, headerRect\.top\)/);
    assert.match(renderer, /isApplicationMenuTopBar\s*\?\s*28\s*:\s*headerRect\.height/);
  });

  it("keeps injected surface theme compatibility without restoring hidden recommendation entries", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const [style] = installRendererStyle(renderer);

    assert.match(renderer, /codexPlusRendererRuntimeVersion = "8"/);
    assert.match(renderer, /function codexPlusHostUsesLightTheme\(\)/);
    assert.match(renderer, /function applyCodexPlusTheme\(overlay\)/);
    assert.match(renderer, /function installCodexPlusThemeSync\(overlay\)/);
    assert.match(renderer, /attributeFilter: \["class", "data-theme", "data-color-scheme", "data-appearance", "data-color-mode"\]/);
    assert.match(renderer, /mediaListenerMode = "event"/);
    assert.match(renderer, /mediaListenerMode = "legacy"/);
    assert.match(renderer, /if \(!overlay\.isConnected\) overlay\.__codexPlusThemeCleanup\?\.\(\)/);
    assert.match(renderer, /function removeCodexPlusModalOverlay\(overlay\)[\s\S]*__codexPlusThemeCleanup/);
    assert.match(renderer, /overlay\.className = "codex-plus-modal-overlay";\s*installCodexPlusThemeSync\(overlay\);/);
    assert.match(style.textContent ?? "", /Keep injected surfaces on Codex's own semantic palette in both themes/);
    assert.match(style.textContent ?? "", /background: var\(--codex-plus-bg-primary\)/);
    assert.match(style.textContent ?? "", /color: var\(--codex-plus-text\)/);
    assert.match(style.textContent ?? "", /\.codex-session-more-menu\s*\{[\s\S]*background: var\(--codex-plus-bg-elevated\)/);
    assert.match(style.textContent ?? "", /\.codex-session-action-tooltip\s*\{[\s\S]*background: var\(--color-token-bg-tooltip, var\(--codex-plus-bg-elevated\)\)/);
    assert.match(style.textContent ?? "", /\.codex-zed-remote-toast,[\s\S]*\.codex-delete-toast\s*\{[\s\S]*background: var\(--codex-plus-bg-elevated\)/);
    assert.match(style.textContent ?? "", /\.codex-delete-confirm-content,[\s\S]*\.codex-plus-modal-content\s*\{[\s\S]*background: var\(--codex-plus-bg-primary\)/);
    assert.match(renderer, /推荐内容页签暂时隐藏/);
    assert.match(renderer, /\/\/ if \(!codexPlusAdsLoaded\) fetchCodexPlusAds\(\);/);
    assert.doesNotMatch(renderer, /codexPlusSidebarNavId|codexPlusPageClass/);
  });

  it("refreshes and cleans up the retained modal theme at runtime", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const theme = themeSyncRuntime(renderer);
    const overlay = createThemeOverlay();
    theme.root.attributes.set("data-theme", "light");

    theme.runtime.installCodexPlusThemeSync(overlay);

    assert.equal(overlay.dataset.codexPlusTheme, "light");
    assert.equal(overlay.values["--codex-plus-bg-primary"], "#ffffff");
    assert.equal(theme.mediaHandlers.size, 1);
    assert.equal(theme.legacyAdds(), 0);

    theme.root.attributes.set("data-theme", "dark");
    const rootObserver = theme.observers.find((observer) =>
      observer.observed.some(({ target, options }) => target === theme.root && options.attributes === true),
    );
    assert.ok(rootObserver);
    rootObserver.callback();
    assert.equal(overlay.dataset.codexPlusTheme, "dark");
    assert.equal(overlay.values["--codex-plus-bg-primary"], "#212121");

    theme.runtime.removeCodexPlusModalOverlay(overlay);
    assert.equal(overlay.isConnected, false);
    assert.equal(theme.mediaHandlers.size, 0);
    assert.ok(theme.observers.every((observer) => observer.disconnected));

    const systemTheme = themeSyncRuntime(renderer);
    const systemOverlay = createThemeOverlay();
    systemTheme.runtime.installCodexPlusThemeSync(systemOverlay);
    assert.equal(systemOverlay.dataset.codexPlusTheme, "light");
    systemTheme.mediaQuery.matches = true;
    systemTheme.mediaHandlers.forEach((handler) => handler());
    assert.equal(systemOverlay.dataset.codexPlusTheme, "dark");
    systemTheme.runtime.removeCodexPlusModalOverlay(systemOverlay);
    assert.equal(systemTheme.mediaHandlers.size, 0);
  });

  it("does not install Codex++ UI in embedded browser documents", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /window\.top\s*!==\s*window/);
    assert.match(renderer, /!window\.electronBridge/);
    assert.ok(renderer.includes("/^app:\\\/\\\/\\-\\//i.test(window.location.href)"));
    assert.match(renderer, /codexPlusIsNodeTestHarness/);
  });

  it("initializes renderer styles without unresolved template identifiers", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    const appended = installRendererStyle(renderer);

    assert.equal(appended.length, 1);
    assert.match(appended[0].textContent ?? "", /#codex-plus-menu/);
  });

  it("does not override the host document root typography or foreground", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const appended = installRendererStyle(renderer);
    const css = appended[0].textContent ?? "";
    const rootRule = css.match(/:root\s*\{([^}]*)\}/)?.[1] ?? "";

    assert.doesNotMatch(rootRule, /(?:^|;)\s*font(?:-family)?\s*:/);
    assert.doesNotMatch(rootRule, /(?:^|;)\s*color\s*:/);
    assert.match(css, /:where\([^)]*codex-plus-modal-overlay[^)]*\)\s*\{[^}]*font-family:\s*inherit;/s);
  });

  it("hides only the official usage alert and restores it without changing upstream styles", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const wrapper = new FakeElement({ className: "w-full", styleDisplay: "grid" });
    const usageAlert = new FakeElement({ dismissLabel: "Dismiss usage alert", hasProgress: true });
    const otherStatus = new FakeElement({ dismissLabel: "Dismiss sync status", hasProgress: true });
    wrapper.appendChild(usageAlert);
    const { runtime, selectors, windowValue } = usageAlertRuntime(renderer, [usageAlert, otherStatus], [wrapper]);

    windowValue.__CODEX_PLUS_HIDE_OFFICIAL_USAGE_ALERT__ = true;
    runtime.refreshOfficialUsageAlertVisibility();

    assert.equal(wrapper.dataset.codexPlusUsageAlertHidden, "true");
    assert.equal(wrapper.style.display, "grid");
    assert.equal(otherStatus.dataset.codexPlusUsageAlertHidden, undefined);
    assert.deepEqual(selectors, [
      '[data-codex-plus-usage-alert-hidden="true"]',
      'aside.app-shell-left-panel [role="status"][aria-live="polite"]',
    ]);

    windowValue.__CODEX_PLUS_HIDE_OFFICIAL_USAGE_ALERT__ = false;
    runtime.refreshOfficialUsageAlertVisibility();

    assert.equal(wrapper.dataset.codexPlusUsageAlertHidden, undefined);
    assert.equal(wrapper.style.display, "grid");
    assert.equal(wrapper.children[0], usageAlert);
    assert.equal(selectors.at(-1), '[data-codex-plus-usage-alert-hidden="true"]');
  });

  it("refreshes active-profile usage alert settings through the existing backend heartbeat", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /typeof nextStatus\.hideOfficialUsageAlert === "boolean"/);
    assert.match(renderer, /window\.__CODEX_PLUS_HIDE_OFFICIAL_USAGE_ALERT__ = nextStatus\.hideOfficialUsageAlert/);
    assert.match(renderer, /\[data-codex-plus-usage-alert-hidden="true"\] \{ display: none !important; \}/);
    assert.doesNotMatch(renderer, /container\.style\.(?:setProperty|removeProperty)\("display"/);
  });

  it("removes session sharing when Codex enhancements are disabled", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /function removeSessionShareButtons\(\)/);
    assert.match(renderer, /function removeSessionShareImportListener\(\)/);
    assert.match(renderer, /function installSessionShareButton\(\)[\s\S]*enhancementsEnabled === false[\s\S]*removeSessionShareButtons\(\)/);
    assert.match(renderer, /function disableCodexPlusRuntimeFeatures\(\)[\s\S]*removeSessionShareButtons\(\)[\s\S]*removeSessionShareImportListener\(\)/);
  });

  it("keeps Windows Dream Skin compatible with the modern Codex main surface", async () => {
    const windowsRenderers = await Promise.all([
      readFile(new URL("../../../assets/inject/upstream/dream-skin/windows/renderer-inject.js", import.meta.url), "utf8"),
      readFile(new URL("../../../assets/inject/upstream/cidala-tiger/windows/renderer-inject.js", import.meta.url), "utf8"),
    ]);

    for (const renderer of windowsRenderers) {
      assert.match(renderer, /MainContentSurface/);
      assert.match(renderer, /data-codex-plus-dream-surface/);
      assert.match(renderer, /ensureShellMain/);
    }
  });
});

/** 从注入脚本里取出 `shouldScheduleScan`，配上可控的依赖来跑。 */
function shouldScheduleScanRuntime(renderer: string) {
  const start = renderer.indexOf("  function shouldScheduleScan(");
  const end = renderer.indexOf("\n  function runScheduledScan(", start);
  assert.ok(start >= 0 && end > start, "shouldScheduleScan not found in renderer-inject.js");
  const source = renderer.slice(start, end);
  const factory = new Function(
    "isChatContentMutation",
    "isExtensionUiNode",
    "nodeSelfOrAncestorMatchesScanRelevance",
    "isScanRelevantNode",
    `${source}\nreturn shouldScheduleScan;`,
  );
  return factory(
    () => false,
    (node: { extension?: boolean }) => Boolean(node?.extension),
    // Codex 的容器（header / 侧栏 nav）本身就是 scan-relevant，这是自喂循环的关键前提。
    (node: { relevant?: boolean }) => Boolean(node?.relevant),
    (node: { relevant?: boolean; extension?: boolean }) =>
      Boolean(node?.relevant) && !node?.extension,
  ) as (mutations: unknown[]) => boolean;
}

const codexContainer = { nodeType: 1, relevant: true };

function mutation(addedNodes: unknown[] = [], removedNodes: unknown[] = []) {
  return { target: codexContainer, addedNodes, removedNodes };
}

describe("renderer injection scan scheduling", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  // issue #1960：我们把自己的节点挂进 Codex 的容器，容器是 scan-relevant，
  // 于是每次写入都会再排一次 scan，scan 又重新写入，空闲时 CPU 被吃满。
  it("ignores mutations that only move the extension's own nodes", async () => {
    const shouldScheduleScan = shouldScheduleScanRuntime(await readFile(rendererPath, "utf8"));
    const ownNode = { nodeType: 1, extension: true };

    assert.equal(shouldScheduleScan([mutation([ownNode])]), false);
    // appendChild 一个已经在位的子节点会同时报 removed + added。
    assert.equal(shouldScheduleScan([mutation([ownNode], [ownNode])]), false);
  });

  it("still scans when Codex itself changes the same container", async () => {
    const shouldScheduleScan = shouldScheduleScanRuntime(await readFile(rendererPath, "utf8"));
    const codexNode = { nodeType: 1, relevant: true };
    const ownNode = { nodeType: 1, extension: true };

    assert.equal(shouldScheduleScan([mutation([codexNode])]), true);
    // 混合变更里只要有一个不是我们的，就不能跳过。
    assert.equal(shouldScheduleScan([mutation([ownNode, codexNode])]), true);
    // 属性变更没有 added/removed 节点，仍按容器相关性判定。
    assert.equal(shouldScheduleScan([mutation()]), true);
  });
});

interface MarketplacePatchHarness {
  install: () => void;
  sweeps: () => number;
  diagnostics: () => string[];
  settle: () => Promise<void>;
}

function marketplacePatchRuntime(renderer: string, patchSucceeds: boolean): MarketplacePatchHarness {
  const start = renderer.indexOf("  const pluginMarketplaceRequestPatchMaxMisses = ");
  const end = renderer.indexOf("\n  function pluginPatchDisabledInRelayMode(", start);
  assert.ok(start >= 0 && end > start, "marketplace patch block not found in renderer-inject.js");
  const source = renderer.slice(start, end);

  let sweeps = 0;
  let pending: Array<() => void> = [];
  const diagnostics: string[] = [];
  const fakeWindow: Record<string, unknown> = {};

  const factory = new Function(
    "window",
    "codexPluginMarketplaceUnlockVersion",
    "pluginPatchDisabledInRelayMode",
    "codexPlusSettings",
    "loadAppServerRequestCandidates",
    "patchPluginMarketplaceRequestClient",
    "sendCodexPlusDiagnostic",
    "__note",
    `${source}\nreturn installPluginMarketplaceRequestPatch;`,
  );

  const install = factory(
    fakeWindow,
    1,
    () => false,
    () => ({ pluginMarketplaceUnlock: true }),
    // 每轮 sweep 在真实实现里会 fetch 全部 app asset，这里只计数并挂起，
    // 好让测试能在「上一轮尚未结束」的时刻再次调用 install。
    () =>
      new Promise((resolve) => {
        sweeps += 1;
        pending.push(() => resolve({ modules: [{}], candidates: [{}], sources: [], discovery: "fallback" }));
      }),
    () => patchSucceeds,
    (event: string) => diagnostics.push(event),
  ) as () => void;

  const settle = async () => {
    // 放行所有挂起的 sweep，并把微任务队列排空。
    while (pending.length) {
      const flush = pending;
      pending = [];
      flush.forEach((resolve) => resolve());
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    }
  };

  return { install, sweeps: () => sweeps, diagnostics: () => diagnostics, settle };
}

/** 取出共用的 module loader，用假时钟驱动它的失败冷却。 */
function moduleLoaderRuntime(renderer: string) {
  const start = renderer.indexOf("  // issue #1960：失败必须被记住。");
  const end = renderer.indexOf("\n  async function loadOptionalCodexAppModule(", start);
  assert.ok(start >= 0 && end > start, "loadCodexAppModule not found in renderer-inject.js");
  const source = renderer.slice(start, end);

  let sweeps = 0;
  let clock = 1_000_000;
  const factory = new Function(
    "codexServiceTierModulePromises",
    "codexAppModuleFailures",
    "codexAppModuleRetryCooldownMs",
    "codexAppModuleMaxAttempts",
    "codexAppAssetUrl",
    "codexAppAssetUrlFromScriptText",
    "Date",
    `${source}\nreturn loadCodexAppModule;`,
  );
  const load = factory(
    new Map(),
    new Map(),
    30000,
    8,
    () => "",
    // 真实实现在这里会把全部 app asset 拉一遍；这里只计数并同样返回“没找到”。
    async () => {
      sweeps += 1;
      return "";
    },
    { now: () => clock },
  ) as (namePart: string) => Promise<unknown>;

  const attempt = async (namePart = "vscode-api-") => {
    try {
      await load(namePart);
    } catch {
      /* 预期失败 */
    }
  };
  return { attempt, sweeps: () => sweeps, advance: (ms: number) => { clock += ms; } };
}

describe("renderer injection codex app module loader", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  // issue #1960：失败以前只是把 promise 删掉，等于没有负缓存，
  // 调用方一重试就重新 fetch 全部 app asset（实测 301 次请求/秒）。
  it("does not re-sweep every asset while the failure is still in cooldown", async () => {
    const loader = moduleLoaderRuntime(await readFile(rendererPath, "utf8"));

    for (let i = 0; i < 20; i += 1) await loader.attempt();

    assert.equal(loader.sweeps(), 1);
  });

  it("retries once per cooldown window, then gives up for good", async () => {
    const loader = moduleLoaderRuntime(await readFile(rendererPath, "utf8"));

    // 冷却期满就允许再试一次，避免 Codex 更新后 asset 回来了却永远发现不了。
    for (let i = 0; i < 30; i += 1) {
      await loader.attempt();
      loader.advance(30001);
    }

    // 连续失败达到上限(8)后彻底停手，而不是每个冷却窗口都再扫一遍。
    assert.equal(loader.sweeps(), 8);
  });

  it("keeps failures separate per asset prefix", async () => {
    const loader = moduleLoaderRuntime(await readFile(rendererPath, "utf8"));

    await loader.attempt("vscode-api-");
    await loader.attempt("app-initial-");
    await loader.attempt("vscode-api-");

    // 两个前缀各自试了一次；第三次命中 vscode-api- 自己的冷却。
    assert.equal(loader.sweeps(), 2);
  });
});

function dictationPatchRuntime(renderer: string, module: Record<string, unknown> | null) {
  const start = renderer.indexOf("  // --- Dictation / Voice patch for apikey");
  const end = renderer.indexOf("\n  async function loadBackendSettingsState", start);
  assert.ok(start >= 0 && end > start, "dictation patch block not found in renderer-inject.js");
  const source = renderer.slice(start, end);
  const fakeWindow: Record<string, unknown> = {};
  const backendSettings = { enhancementsEnabled: true };
  const timers = new Map<number, unknown>();
  const cleared: number[] = [];
  let nextTimer = 1;
  const fakeDocument = {
    querySelectorAll(selector: string) {
      assert.equal(selector, "button");
      return [];
    },
  };
  const factory = new Function(
    "window",
    "document",
    "isCurrentCodexPlusRendererRuntime",
    "codexPlusBackendSettings",
    "loadOptionalCodexAppModule",
    "sendCodexPlusDiagnostic",
    "setInterval",
    "clearInterval",
    `${source}\nreturn { installDictationSupportPatch, cleanupCodexDictationDomPatch };`,
  ) as (
    windowValue: Record<string, unknown>,
    documentValue: typeof fakeDocument,
    isCurrent: () => boolean,
    settings: typeof backendSettings,
    loadModule: (namePart: string) => Promise<Record<string, unknown> | null>,
    diagnostic: (...args: unknown[]) => void,
    setTimer: (callback: () => void, delay: number) => number,
    clearTimer: (timer: number) => void,
  ) => {
    installDictationSupportPatch: () => Promise<boolean | undefined>;
    cleanupCodexDictationDomPatch: () => void;
  };
  const runtime = factory(
    fakeWindow,
    fakeDocument,
    () => true,
    backendSettings,
    async () => module,
    () => undefined,
    (callback, delay) => {
      const timer = nextTimer++;
      timers.set(timer, { callback, delay });
      return timer;
    },
    (timer) => {
      cleared.push(timer);
      timers.delete(timer);
    },
  );
  return { ...runtime, backendSettings, fakeWindow, timers, cleared };
}

describe("renderer injection dictation support", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  it("only overrides a failed support check for API-key authentication", async () => {
    const renderer = await readFile(rendererPath, "utf8");
    const module = {
      supported: function checkSupport(args: { authMethod?: string }) {
        return args?.authMethod === "chatgpt" ? false : false;
      },
    };
    const runtime = dictationPatchRuntime(renderer, module);

    await runtime.installDictationSupportPatch();

    assert.equal(module.supported({ authMethod: "chatgpt" }), false);
    assert.equal(module.supported({ authMethod: "apikey" }), true);
    assert.equal(runtime.fakeWindow.__codexDictationSupportPatched, "1");
    assert.equal(runtime.timers.size, 0);
  });

  it("clears the DOM fallback timer when the dictation patch is cleaned up", async () => {
    const renderer = await readFile(rendererPath, "utf8");
    const runtime = dictationPatchRuntime(renderer, null);

    await runtime.installDictationSupportPatch();
    assert.equal(runtime.timers.size, 1);
    assert.equal(runtime.fakeWindow.__codexDictationDomPatched, true);

    runtime.cleanupCodexDictationDomPatch();

    assert.deepEqual(runtime.cleared, [1]);
    assert.equal(runtime.timers.size, 0);
    assert.equal(runtime.fakeWindow.__codexDictationDomTimer, null);
    assert.equal(runtime.fakeWindow.__codexDictationDomPatched, false);
  });

  it("does not claim success when the module export cannot be replaced", async () => {
    const renderer = await readFile(rendererPath, "utf8");
    const original = function checkSupport(args: { authMethod?: string }) {
      return args?.authMethod === "chatgpt" ? false : true;
    };
    const module: Record<string, unknown> = {};
    Object.defineProperty(module, "supported", {
      configurable: false,
      enumerable: true,
      value: original,
      writable: false,
    });
    const runtime = dictationPatchRuntime(renderer, module);

    await runtime.installDictationSupportPatch();

    assert.equal(module.supported, original);
    assert.notEqual(runtime.fakeWindow.__codexDictationSupportPatched, "1");
    assert.equal(runtime.timers.size, 1);
  });
});

interface DispatcherPatchHarness {
  install: () => void;
  attempts: () => number;
  diagnostics: () => string[];
  settle: () => Promise<void>;
}

function dispatcherPatchRuntime(renderer: string, dispatcherFound: boolean): DispatcherPatchHarness {
  const start = renderer.indexOf("  const serviceTierDispatcherPatchMaxMisses = ");
  const end = renderer.indexOf("\n  async function loadBackendSettingsState(", start);
  assert.ok(start >= 0 && end > start, "service tier dispatcher patch block not found");
  const source = renderer.slice(start, end);

  let attempts = 0;
  let pending: Array<() => void> = [];
  const diagnostics: string[] = [];
  const fakeWindow: Record<string, unknown> = {};

  const factory = new Function(
    "window",
    "codexServiceTierRequestOverrideVersion",
    "loadCodexAppModule",
    "codexServiceTierDispatcherFromModule",
    "dispatchCodexPlusMessage",
    "installCodexRemoteSessionDispatcherSubscription",
    "sendCodexPlusDiagnostic",
    `${source}\nreturn installCodexServiceTierDispatcherPatch;`,
  );

  const install = factory(
    fakeWindow,
    1,
    // 真实实现每轮会依次试三个前缀，每个 miss 都触发一轮全量 asset 扫描。
    () =>
      new Promise((resolve, reject) => {
        attempts += 1;
        pending.push(() => (dispatcherFound ? resolve({}) : reject(new Error("未找到 Codex App asset"))));
      }),
    () => (dispatcherFound ? { dispatchMessage() {}, subscribe() {} } : null),
    () => undefined,
    () => undefined,
    (event: string) => diagnostics.push(event),
  ) as () => void;

  const settle = async () => {
    while (pending.length) {
      const flush = pending;
      pending = [];
      flush.forEach((resolve) => resolve());
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    }
  };

  return { install, attempts: () => attempts, diagnostics: () => diagnostics, settle };
}

describe("renderer injection service tier dispatcher patch", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  // issue #1960：这个补丁挂在 scanLightweight() 里每轮都跑，是 #1324 同一缺陷的第三个实例。
  it("does not start a new sweep while the previous one is still running", async () => {
    const harness = dispatcherPatchRuntime(await readFile(rendererPath, "utf8"), false);

    for (let i = 0; i < 20; i += 1) harness.install();

    assert.equal(harness.attempts(), 1);
    await harness.settle();
  });

  it("stops retrying and stops re-reporting once the dispatcher is clearly gone", async () => {
    const harness = dispatcherPatchRuntime(await readFile(rendererPath, "utf8"), false);

    for (let i = 0; i < 40; i += 1) {
      harness.install();
      await harness.settle();
    }

    // 三个前缀里第一个就抛，loadDispatcher 会继续试下一个，所以每轮不止一次尝试；
    // 关键是达到 maxMisses(8) 之后彻底停手。
    assert.equal(harness.diagnostics().filter((e) => e === "service_tier_dispatcher_patch_failed").length, 1);
    assert.deepEqual(harness.diagnostics().at(-1), "service_tier_dispatcher_patch_skipped");
    const settled = harness.attempts();
    harness.install();
    await harness.settle();
    assert.equal(harness.attempts(), settled);
  });

  it("keeps working normally when the dispatcher is found", async () => {
    const harness = dispatcherPatchRuntime(await readFile(rendererPath, "utf8"), true);

    harness.install();
    await harness.settle();
    for (let i = 0; i < 10; i += 1) harness.install();

    assert.equal(harness.attempts(), 1);
    assert.deepEqual(harness.diagnostics(), ["service_tier_dispatcher_patch_installed"]);
  });
});

describe("renderer injection plugin marketplace patch", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  // issue #1960：scanDeferred() 每轮都调用这个补丁，而早退守卫只在打上补丁后才写入。
  // Codex 侧 asset 改名后这层永远成功不了，过去既不去重也不放弃，
  // 于是每轮 scan 都把全部 app asset 重新 fetch 一遍（实测 530 次 fetch/秒）。
  it("does not start a new sweep while the previous one is still running", async () => {
    const harness = marketplacePatchRuntime(await readFile(rendererPath, "utf8"), false);

    // 模拟连续多轮 scan：上一轮还挂着，后续调用必须被 in-flight 守卫挡掉。
    for (let i = 0; i < 20; i += 1) harness.install();

    assert.equal(harness.sweeps(), 1);
    await harness.settle();
  });

  it("stops retrying once the asset is clearly unavailable", async () => {
    const harness = marketplacePatchRuntime(await readFile(rendererPath, "utf8"), false);

    // 每次都跑完再发起下一轮，模拟长时间运行中的反复 scan。
    for (let i = 0; i < 40; i += 1) {
      harness.install();
      await harness.settle();
    }

    // 达到 maxMisses(8) 之后必须彻底停掉，而不是无限重试。
    assert.equal(harness.sweeps(), 8);
    // 首次 miss 上报一次，停用时再报一次，中间保持噤声。
    assert.deepEqual(harness.diagnostics(), [
      "plugin_marketplace_request_patch_not_found",
      "plugin_marketplace_request_patch_skipped",
    ]);
  });

  it("keeps working normally when the patch actually lands", async () => {
    const harness = marketplacePatchRuntime(await readFile(rendererPath, "utf8"), true);

    harness.install();
    await harness.settle();
    // 打上补丁后守卫生效，后续 scan 不再重复扫描。
    for (let i = 0; i < 10; i += 1) harness.install();

    assert.equal(harness.sweeps(), 1);
    assert.deepEqual(harness.diagnostics(), ["plugin_marketplace_request_patch_installed"]);
  });
});

describe("relay pureApi provider resolution", () => {
  function providerRuntime(
    renderer: string,
    backendSettings: Record<string, unknown>,
    catalog: Record<string, unknown>,
    profile: Record<string, unknown>,
  ) {
    const start = renderer.indexOf("function codexRelayConfigModelProvider(");
    const codeStart = renderer.indexOf("function codexRemoteSessionTargetProvider(");
    const end = renderer.indexOf("\n  function codexRemoteSessionProviderRequestMethod", codeStart);
    assert.ok(start >= 0 && codeStart >= 0 && end > codeStart);
    const source = renderer.slice(start, end);
    const create = new Function(
      "codexPlusBackendSettings",
      "codexModelCatalog",
      "codexRemoteSessionActiveProfile",
      `${source}\nreturn { codexRelayConfigModelProvider, codexRemoteSessionTargetProvider };`,
    ) as (
      backend: Record<string, unknown>,
      cat: Record<string, unknown>,
      activeProfile: () => Record<string, unknown>,
    ) => {
      codexRelayConfigModelProvider: (configContents: string) => string;
      codexRemoteSessionTargetProvider: () => string;
    };
    return create(backendSettings, catalog, () => profile);
  }

  it("resolves the real model_provider from a pureApi relay profile instead of hardcoding custom", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(
      renderer,
      {},
      { codex_model_provider: "deepseek" },
      { relayMode: "pureApi", configContents: 'model = "deepseek-v4-flash-vision-exp"\nmodel_provider = "deepseek"' },
    );

    assert.equal(runtime.codexRelayConfigModelProvider('model_provider = "deepseek"'), "deepseek");
    assert.equal(runtime.codexRemoteSessionTargetProvider(), "deepseek");
  });

  it("still returns custom for pureApi relays that genuinely declare the custom provider", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(
      renderer,
      {},
      { codex_model_provider: "custom" },
      { relayMode: "pureApi", configContents: 'model_provider = "custom"\n[model_providers.custom]' },
    );

    assert.equal(runtime.codexRemoteSessionTargetProvider(), "custom");
  });

  it("falls back to custom when a pureApi relay declares no provider", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(renderer, {}, { codex_model_provider: "" }, { relayMode: "pureApi", configContents: "" });

    assert.equal(runtime.codexRemoteSessionTargetProvider(), "custom");
  });

  // activeRelayCodexProvider 是全局缓存，切换供应商后可能还留着上一个的值。
  // pureApi 时优先信 profile 自己的 configContents；profile 没声明就回到
  // "custom"，不采信这个缓存——cdp_bridge.rs 的 refreshedPureApiResumeProvider
  // 正是钉这个：陈旧缓存是 stale_custom_provider 时必须仍解析成 custom。
  it("ignores a possibly stale activeRelayCodexProvider for pureApi relays", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(
      renderer,
      { activeRelayCodexProvider: "stale_custom_provider" },
      { codex_model_provider: "" },
      { relayMode: "pureApi", configContents: "" },
    );

    assert.equal(runtime.codexRemoteSessionTargetProvider(), "custom");
  });

  // 非 pureApi 才拿 activeRelayCodexProvider 兜底。
  it("still falls back to activeRelayCodexProvider outside pureApi", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(
      renderer,
      { activeRelayCodexProvider: "deepseek" },
      { codex_model_provider: "" },
      { relayMode: "mixedApi", configContents: "" },
    );

    assert.equal(runtime.codexRemoteSessionTargetProvider(), "deepseek");
  });

  // profile 自己声明了供应方时，优先级高于全局缓存。
  it("prefers the profile's own configContents over the cached provider", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(
      renderer,
      { activeRelayCodexProvider: "stale_custom_provider" },
      { codex_model_provider: "" },
      { relayMode: "pureApi", configContents: 'model_provider = "deepseek"' },
    );

    assert.equal(runtime.codexRemoteSessionTargetProvider(), "deepseek");
  });
});
