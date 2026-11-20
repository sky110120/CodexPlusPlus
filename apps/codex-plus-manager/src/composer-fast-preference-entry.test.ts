import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";

const host = readFileSync(new URL("../../../assets/inject/renderer-inject/92-extension-host.js", import.meta.url), "utf8");
const extract = (name: string) => {
  const source = host.match(new RegExp(`^  function ${name}\\([^]*?^  \\}`, "m"))?.[0];
  assert.ok(source, name);
  return source;
};
const marker = "data-codex-plus-composer-fast-preference";

class Node {
  id = "";
  textContent = "";
  parentElement: Node | null = null;
  readonly children: Node[] = [];
  readonly dataset: Record<string, string> = {};
  readonly attributes = new Map<string, string>();
  readonly listeners = new Map<string, () => void>();

  setAttribute(name: string, value: string) { this.attributes.set(name, value); }
  hasAttribute(name: string) { return this.attributes.has(name); }
  addEventListener(type: string, callback: (event: { preventDefault(): void; stopPropagation(): void }) => void) {
    this.listeners.set(type, () => callback({ preventDefault() {}, stopPropagation() {} }));
  }
  remove() {
    if (this.parentElement) this.parentElement.children.splice(this.parentElement.children.indexOf(this), 1);
    this.parentElement = null;
  }
  insertBefore(node: Node, next: Node | null) {
    node.remove();
    this.children.splice(next ? this.children.indexOf(next) : this.children.length, 0, node);
    node.parentElement = this;
  }
  append(node: Node) { this.insertBefore(node, null); }
}

test("Fast is a local settings entry, follows real preferences, and is replaced on reinjection", () => {
  const head = new Node();
  const oldStyle = new Node();
  oldStyle.id = "codex-plus-composer-actions-style";
  head.append(oldStyle);
  let footer = new Node();
  const originalFooter = footer;
  const dictation = new Node();
  dictation.setAttribute("data-dictation-controls", "");
  footer.append(dictation);
  const footers = [footer];
  const buttons = () => footers.flatMap((node) => node.children.filter((child) => child.hasAttribute(marker)));
  const window = { __codexPlusBackendGeneration: 1 };
  const settings = { enhancementsEnabled: true, codexAppNativeFastUnlock: true };
  const opened: Array<[number, Node]> = [];
  const tooltips: string[] = [];
  let hostScans = 0;

  function renderer(generation: number) {
    const context = vm.createContext({
      window,
      document: {
        head,
        createElement: () => new Node(),
        getElementById: (id: string) => head.children.find((node) => node.id === id),
        querySelectorAll: (selector: string) => footers.flatMap((node) =>
          node.children.filter((child) => child.hasAttribute(selector.slice(1, -1)))),
      },
      codexPlusBackendGeneration: generation,
      codexPlusBackendSettingsLoaded: false,
      codexPlusBackendSettings: settings,
      codexPlusRegistry: { composerActions: new Map() },
      codexPlusExtensionItems: (items: Map<string, unknown>) => [...items.values()],
      codexPlusComposerActionHosts: () => { hostScans += 1; return [{ footer }]; },
      attachCodexPlusRailTooltip: (_button: Node, label: string) => tooltips.push(label),
      markCodexPlusExtensionNode: (node: Node, key: string) => node.setAttribute("data-codex-plus-ext", key),
      openCodexPlusFastPreference: (button: Node) => opened.push([generation, button]),
    });
    vm.runInContext(
      ["codexPlusComposerFastPreferenceEnabled", "refreshCodexPlusComposerActions"].map(extract).join("\n"),
      context,
    );
    return context;
  }

  const first = renderer(1);
  first.refreshCodexPlusComposerActions();
  assert.deepEqual(buttons(), []);
  assert.equal(hostScans, 0);

  first.codexPlusBackendSettingsLoaded = true;
  first.refreshCodexPlusComposerActions();
  const [button] = buttons();
  assert.equal(hostScans, 1);
  assert.equal(footer.children[0], button);
  assert.equal(button.textContent, "Fast");
  assert.equal(button.attributes.get("aria-haspopup"), "dialog");
  assert.equal(button.attributes.has("aria-pressed"), false);
  assert.equal(button.attributes.get("data-codex-plus-ext"), "builtin-fast-preference");
  assert.match(tooltips[0], /本机默认速度/);
  assert.match(oldStyle.textContent, /\[data-codex-plus-composer-fast-preference\]/);
  first.refreshCodexPlusComposerActions();
  assert.deepEqual(buttons(), [button]);

  window.__codexPlusBackendGeneration = 2;
  button.listeners.get("click")?.();
  assert.deepEqual(opened, []);
  const second = renderer(2);
  second.codexPlusBackendSettingsLoaded = true;
  second.refreshCodexPlusComposerActions();
  assert.equal(button.parentElement, null);
  const [fresh] = buttons();
  first.refreshCodexPlusComposerActions();
  fresh.listeners.get("click")?.();
  assert.deepEqual(opened, [[2, fresh]]);

  footer = new Node();
  footers.push(footer);
  second.refreshCodexPlusComposerActions();
  const [moved] = buttons();
  assert.equal(fresh.parentElement, null);
  assert.equal(moved.parentElement, footer);
  assert.deepEqual(originalFooter.children, [dictation]);

  settings.codexAppNativeFastUnlock = false;
  moved.listeners.get("click")?.();
  assert.equal(opened.length, 1);
  second.refreshCodexPlusComposerActions();
  assert.deepEqual(buttons(), []);
  settings.codexAppNativeFastUnlock = true;
  settings.enhancementsEnabled = false;
  second.refreshCodexPlusComposerActions();
  assert.deepEqual(buttons(), []);
});
