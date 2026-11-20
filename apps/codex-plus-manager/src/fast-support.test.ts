import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";
import { modelFastSupportMode, setModelFastSupportMode, synchronizeModelMetadataFastSupport, parseModelMetadataDocument, createActiveImportDraft, cancelActiveImportDraft } from "./model-metadata.ts";

const fragment = (name: string) => readFileSync(new URL(`../../../assets/inject/renderer-inject/${name}`, import.meta.url), "utf8");
const extract = (source: string, name: string) => {
  const body = source.match(new RegExp(`^  function ${name}\\([^]*?^  \\}`, "m"))?.[0];
  assert.ok(body, name); return body;
};

test("per-model Fast modes preserve all original metadata across support, denial and inheritance", () => {
  const original = { description: "retained", service_tiers: [{ id: "flex", name: "Flex", description: "Existing tier" }], additional_speed_tiers: ["slow"] };
  let current = original;
  for (const mode of ["supported", "unsupported", "inherit"] as const) {
    current = setModelFastSupportMode(current, mode) as typeof current;
    assert.equal(modelFastSupportMode(current), mode);
    assert.deepEqual(current.service_tiers, original.service_tiers);
    assert.deepEqual(current.additional_speed_tiers, original.additional_speed_tiers);
    assert.equal(current.description, original.description);
  }
  assert.equal("codex_plus_fast_support" in original, false);
  assert.equal(modelFastSupportMode({ service_tiers: [] }), "unsupported");
  assert.equal(modelFastSupportMode({ additional_speed_tiers: ["fast"] }), "supported");
  assert.equal(modelFastSupportMode({}), "inherit");
});

test("Fast editing touches only the selected model and cancellation keeps the saved declaration", () => {
  const source = JSON.stringify({ models: [{ slug: "grok", service_tiers: [{ id: "flex" }] }, { slug: "other", description: "untouched" }] });
  const changed = synchronizeModelMetadataFastSupport(source, "GROK[1M]", "supported")!;
  assert.ok(changed);
  assert.deepEqual(JSON.parse(changed.document).models[1], JSON.parse(source).models[1]);
  assert.deepEqual(JSON.parse(changed.document).models[0].service_tiers, [{ id: "flex" }]);
  assert.equal(changed.preview.metadata.codex_plus_fast_support, "supported");
  const inherited = synchronizeModelMetadataFastSupport(changed.document, "grok", "inherit")!;
  assert.equal(modelFastSupportMode(inherited.preview.metadata), "inherit");
  const draft = createActiveImportDraft({ index: 0, rowName: "grok", window: "1000000", autoCompact: "90%", document: source, preview: parseModelMetadataDocument(source, "grok").ok ? changed.preview : null });
  assert.equal(cancelActiveImportDraft({ ...draft, document: changed.document, preview: changed.preview }).draft, null);
  assert.equal(modelFastSupportMode(parseModelMetadataDocument(source, "grok").ok ? JSON.parse(source).models[0] : null), "unsupported");
  assert.equal(synchronizeModelMetadataFastSupport("not-json", "grok", "supported"), null);
  assert.equal(synchronizeModelMetadataFastSupport(JSON.stringify({ models: [{ slug: "grok" }, { slug: "grok" }] }), "grok", "supported"), null);
});

test("Fast support declarations stay available for model metadata without enforcing a request policy", () => {
  const metadata: Record<string, unknown> = {};
  const context = vm.createContext({
    codexServiceTierSupportedFastModels: new Set(["gpt-5.4"]),
    codexPlusModelMetadata: (model: string) => metadata[model.toLowerCase()],
    codexServiceTierUiModelName: () => "grok-custom",
  });
  vm.runInContext(["normalizeCodexServiceTierModelName", "codexServiceTierFastSupportedForModel", "codexServiceTierFastAvailability"]
    .map((name) => extract(fragment("20-menu.js"), name)).join("\n"), context);
  const supported = (model: string) => context.codexServiceTierFastAvailability(model).supported;
  assert.equal(supported("grok-fast-without-a-declaration"), false);
  metadata["grok-custom"] = { prioritySupportOverride: true };
  assert.equal(supported("grok-custom"), true);
  metadata["gpt-5.4"] = { prioritySupportOverride: false, serviceTiers: [{ id: "priority" }] };
  assert.equal(supported("gpt-5.4"), false);
  metadata["gpt-5.4"] = { serviceTiers: [] };
  assert.equal(supported("gpt-5.4"), true);
});

test("retired Fast preferences cannot rewrite the native service tier or enable native unlock", () => {
  const style = fragment("10-style.js");
  const map = style.match(/  const codexPlusBackendSettingMap = \{[^]*?^  \};/m)?.[0];
  assert.ok(map);
  const stored = { serviceTierControls: true, nativeFastUnlock: false };
  const context = vm.createContext({
    conversationViewDefaultWidth: 900, window: {},
    localStorage: { getItem: () => JSON.stringify(stored) },
    codexPlusSettingsKey: "settings",
    codexPlusBackendSettings: { enhancementsEnabled: true, codexAppServiceTierControls: true },
    applyCodexRemoteSessionProviderOverride: (_method: string, params: unknown) => params,
  });
  vm.runInContext([map, ...["defaultCodexPlusSettings", "backendCodexPlusSettings", "codexPlusSettings"]
    .map((name) => extract(style, name)), extract(fragment("30-service-tier.js"), "applyCodexServiceTierRequestOverride")].join("\n"), context);
  assert.equal(context.codexPlusSettings().serviceTierControls, undefined);
  assert.equal(context.codexPlusSettings().nativeFastUnlock, false);
  for (const params of [{ model: "custom", serviceTier: "priority" }, { model: "custom", service_tier: "flex" }, { model: "custom" }]) {
    assert.equal(context.applyCodexServiceTierRequestOverride("turn/start", params), params);
  }
});

test("reinjection removes the retired composer Fast badge", () => {
  let removed = 0;
  const context = vm.createContext({ document: { querySelectorAll: (selector: string) => {
    assert.equal(selector, '[data-codex-service-tier-badge="true"]');
    return [{ remove: () => { removed += 1; } }];
  } } });
  vm.runInContext(extract(fragment("90-action-groups.js"), "removeLegacyCodexServiceTierBadges"), context);
  context.removeLegacyCodexServiceTierBadges();
  assert.equal(removed, 1);
  assert.doesNotMatch(fragment("90-action-groups.js"), /function installCodexServiceTierBadge/);
});

const modelTierRenderer = (metadata: Record<string, unknown>, modelReasoningEfforts: () => unknown) => {
  const source = fragment("70-model-catalog.js");
  const context = vm.createContext({
    codexPlusModelMetadata: () => metadata,
    modelReasoningEfforts,
    codexModelCatalog: { provider_name: "Provider" },
  });
  vm.runInContext([
    "codexPlusModelTierIsFast", "codexPlusModelTierIdentity", "codexPlusMergeModelTiers",
    "codexPlusModelDeclaredTierMetadata", "applyCodexPlusModelMetadata", "codexPlusModelDescriptor",
    "codexPlusModelInputModalities", "codexPlusModelImageDetailOriginal",
  ].map((name) => extract(source, name)).join("\n"), context);
  return context;
};
const plain = (value: unknown) => JSON.parse(JSON.stringify(value));

test("declared Fast merges into native descriptors without replacing other tiers or account access", () => {
  const metadata: Record<string, unknown> = {
    prioritySupportOverride: true,
    serviceTiers: [{ id: "priority", name: "Fast", description: "Provider-declared priority" }],
    additionalSpeedTiers: ["fast", "FAST"],
    supportedReasoningEfforts: [],
  };
  const context = modelTierRenderer(metadata, () => { throw new Error("Fast does not configure reasoning choices"); });
  const descriptor: Record<string, unknown> = {
    displayName: "Native Grok", description: "Native details", defaultReasoningEffort: "high",
    supportedReasoningEfforts: [{ reasoningEffort: "high" }], serviceTierAccess: true,
    serviceTiers: [
      { id: "flex", name: "Flex" },
      { id: "priority", name: "Native priority", description: "Native description", usageMultiplier: 2 },
      { id: "ultrafast", name: "Ultrafast", nativeBadge: "Beta" },
    ],
    additionalSpeedTiers: ["ultrafast", "slow", "ULTRAFAST"],
  };
  assert.equal(context.applyCodexPlusModelMetadata(descriptor, "grok-custom"), true);
  assert.deepEqual(plain(descriptor.serviceTiers), [
    { id: "flex", name: "Flex" },
    { id: "priority", name: "Fast", description: "Provider-declared priority", usageMultiplier: 2 },
    { id: "ultrafast", name: "Ultrafast", nativeBadge: "Beta" },
  ]);
  assert.deepEqual(plain(descriptor.additionalSpeedTiers), ["ultrafast", "slow", "fast"]);
  assert.equal(descriptor.displayName, "Native Grok");
  assert.equal(descriptor.description, "Native details");
  assert.equal(descriptor.defaultReasoningEffort, "high");
  assert.equal(descriptor.serviceTierAccess, true);
  const stableTiers = descriptor.serviceTiers;
  assert.equal(context.applyCodexPlusModelMetadata(descriptor, "grok-custom"), false);
  assert.equal(descriptor.serviceTiers, stableTiers, "repeated injection leaves the descriptor unchanged");

  context.modelReasoningEfforts = () => [];
  const created = plain(context.codexPlusModelDescriptor("grok-custom"));
  assert.deepEqual(created.serviceTiers, metadata.serviceTiers);
  assert.deepEqual(created.additionalSpeedTiers, ["fast"]);
  assert.equal("serviceTierAccess" in created, false);

  Object.assign(metadata, { displayName: "Explicit name", description: "Explicit details", defaultReasoningEffort: "low" });
  assert.equal(context.applyCodexPlusModelMetadata(descriptor, "grok-custom"), true);
  assert.equal(descriptor.displayName, "Explicit name");
  assert.equal(descriptor.description, "Explicit details");
  assert.equal(descriptor.defaultReasoningEffort, "low");
});

test("empty model tier metadata does not erase tiers already present on a native descriptor", () => {
  const metadata: Record<string, unknown> = { serviceTiers: [], additionalSpeedTiers: [] };
  const context = modelTierRenderer(metadata, () => { throw new Error("no reasoning declaration"); });
  const descriptor: Record<string, unknown> = {
    serviceTiers: [{ id: "priority", name: "Fast" }],
    additionalSpeedTiers: ["fast"],
  };
  assert.equal(context.applyCodexPlusModelMetadata(descriptor, "grok-custom"), false);
  assert.deepEqual(descriptor.serviceTiers, [{ id: "priority", name: "Fast" }]);
  assert.deepEqual(descriptor.additionalSpeedTiers, ["fast"]);
});

test("explicit Fast denial filters declarations and cached descriptor tiers but preserves other tiers", () => {
  const metadata: Record<string, unknown> = {
    prioritySupportOverride: false,
    service_tiers: [
      { id: "priority", name: "Fast" },
      { id: "flex", name: "Flex" },
      { id: "fast", name: "Fast" },
      { id: "economy", name: "Economy" },
    ],
    additionalSpeedTiers: ["fast", "batch"],
  };
  const context = modelTierRenderer(metadata, () => []);
  const descriptor: Record<string, unknown> = {
    serviceTiers: [
      { id: "priority", name: "Fast" },
      { name: "Fast" },
      { id: "native", name: "Native" },
      { id: "flex", name: "Fast", qualifier: "Native Flex metadata" },
      { id: "ultrafast", name: "Ultrafast" },
    ],
    additionalSpeedTiers: ["FAST", "native-speed", "ultrafast"],
    serviceTierAccess: true,
  };

  assert.equal(context.applyCodexPlusModelMetadata(descriptor, "grok-custom"), true);
  assert.deepEqual(plain(descriptor.serviceTiers), [
    { id: "native", name: "Native" },
    { id: "flex", name: "Flex", qualifier: "Native Flex metadata" },
    { id: "ultrafast", name: "Ultrafast" },
    { id: "economy", name: "Economy" },
  ]);
  assert.deepEqual(plain(descriptor.additionalSpeedTiers), ["native-speed", "ultrafast", "batch"]);
  assert.equal(context.applyCodexPlusModelMetadata(descriptor, "grok-custom"), false);
  assert.equal(descriptor.serviceTierAccess, true);

  const created = plain(context.codexPlusModelDescriptor("grok-custom"));
  assert.deepEqual(created.serviceTiers, [{ id: "flex", name: "Flex" }, { id: "economy", name: "Economy" }]);
  assert.deepEqual(created.additionalSpeedTiers, ["batch"]);
  assert.equal("serviceTierAccess" in created, false);
});

test("a Fast denial with no nonempty declaration filters only native Fast tiers", () => {
  const context = modelTierRenderer({ prioritySupportOverride: false, serviceTiers: [] }, () => []);
  const descriptor = {
    serviceTiers: [{ id: "priority", name: "Fast" }, { id: "custom", name: "Fast" }, { id: "ultrafast", name: "Ultrafast" }],
    additionalSpeedTiers: ["priority", "ultrafast", "flex"],
  };
  assert.equal(context.applyCodexPlusModelMetadata(descriptor, "model"), true);
  assert.deepEqual(plain(descriptor.serviceTiers), [{ id: "custom", name: "Fast" }, { id: "ultrafast", name: "Ultrafast" }]);
  assert.deepEqual(plain(descriptor.additionalSpeedTiers), ["ultrafast", "flex"]);
});
