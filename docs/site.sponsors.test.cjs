const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("node:fs");
const path = require("node:path");

const read = (relativePath) => fs.readFileSync(path.join(__dirname, "..", relativePath), "utf8");
const visibleHtml = (html) => html.replace(/<!--[\s\S]*?-->/g, "");
const visibleJs = (javascript) => javascript.replace(/\/\*[\s\S]*?\*\//g, "");

test("public site HTML keeps sponsor promotion disabled in every language", () => {
  for (const file of ["docs/index.html", "site/index.html", "site/en.html", "docs/readme-preview.html"]) {
    const html = visibleHtml(read(file));
    assert.doesNotMatch(html, /ads\.codexpp\.cc\/ads\.json|感谢赞助商|Thanks to our sponsors|sponsor-block|sponsors-title/i, file);
  }
});

test("public README variants do not expose sponsor or affiliate links", () => {
  for (const file of ["README.md", "README.preview.md", "README_EN.md"]) {
    const markdown = visibleHtml(read(file));
    assert.doesNotMatch(markdown, /register\?aff=|[?&]promo=|apinebula\.ai\/ref\/|感谢赞助商|赞助横幅/i, file);
  }
});

test("site entry scripts do not request or render the remote sponsor list", () => {
  const docsScript = visibleJs(read("docs/site.js"));
  const siteScript = visibleJs(read("site/js/site.js"));
  assert.doesNotMatch(docsScript, /^\s*setupSponsors\(\);\s*$/m);
  assert.doesNotMatch(siteScript, /ads\.codexpp\.cc\/ads\.json|function sponsors\(/i);
});
