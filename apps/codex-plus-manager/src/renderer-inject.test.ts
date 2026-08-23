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
    assert.match(style.textContent ?? "", /Theme compatibility for retained modal and non-modal injected surfaces/);
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
