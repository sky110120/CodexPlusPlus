import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { it } from "node:test";

it("启动与导航不再请求广告，工具列表仍独立加载", async () => {
  const source = await readFile(new URL("./App.tsx", import.meta.url), "utf8");
  assert.match(source, /void refreshTools\(true\)/);
  assert.doesNotMatch(source, /load_ads|refreshAds|setSponsorAds/);
});
