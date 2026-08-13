import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { inferNativeTarget, selectTargetVsix } from "./release-assets.mjs";

const root = dirname(fileURLToPath(import.meta.url));
const html = await readFile(join(root, "index.html"), "utf8");
const css = await readFile(join(root, "styles.css"), "utf8");
const script = await readFile(join(root, "app.js"), "utf8");

function validateHtml(candidate) {
  assert.match(candidate, /<html lang="en"/u, "English must remain the static default");
  assert.match(candidate, /One VS Code window\./u, "the North Star must work without JavaScript");
  assert.match(candidate, /id="install"/u, "installation instructions must be statically available");
  assert.match(candidate, /30 conversations/u, "the 30-session contract must be visible");
  assert.match(
    candidate,
    /https:\/\/marketplace\.visualstudio\.com\/items\?itemName=runtrol\.runtrol-studio/u,
    "the public Marketplace route must be statically available",
  );
  assert.match(candidate, /Install from Marketplace/u, "the public Marketplace action must be visible");
  assert.match(candidate, /Secure phone app available/u, "the shipped PWA state must be visible");
  assert.match(candidate, /href="app\/"/u, "the phone app route must be statically available");
  assert.doesNotMatch(candidate, /<link[^>]+(?:fonts\.googleapis|cdn\.)/u, "external style or font CDN is forbidden");
  assert.doesNotMatch(candidate, /<script[^>]+https?:\/\//u, "external script CDN is forbidden");
}

function validateScript(candidate) {
  for (const locale of ["en", "ko", "zh", "ja"]) {
    assert.match(candidate, new RegExp(`\\b${locale}: \\{`, "u"), `missing locale: ${locale}`);
  }
  assert.match(candidate, /releases\/latest/u, "release discovery must use the latest release API");
  assert.match(candidate, /selectTargetVsix\(assets, target\)/u, "manual install must select a native target");
  assert.doesNotMatch(candidate, /version\s*[:=]\s*["']\d+\.\d+/u, "the page must not hardcode a release version");
  assert.doesNotMatch(
    candidate,
    /localStorage\.(?:setItem|getItem)\(\s*["'](?:conversation|transcript)/iu,
    "client code must not persist conversation content",
  );
}

function validateTypography(...candidates) {
  for (const candidate of candidates) {
    assert.equal(candidate.includes("\u2013"), false, "en dash is forbidden");
    assert.equal(candidate.includes("\u2014"), false, "em dash is forbidden");
  }
}

function expectFailure(check, candidate, label) {
  assert.throws(() => check(candidate), undefined, `${label} mutation must fail`);
}

validateHtml(html);
validateScript(script);
validateTypography(html, css, script);

const fixtures = ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64", "win32-arm64", "win32-x64"]
  .map((target) => ({
    name: `runtrol-studio-${target}.vsix`,
    browser_download_url: `https://github.com/eddmpython/runtrol/releases/download/test/${target}.vsix`,
  }));
for (const target of ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64", "win32-arm64", "win32-x64"]) {
  assert.equal(selectTargetVsix(fixtures, target)?.name, `runtrol-studio-${target}.vsix`);
}
assert.equal(inferNativeTarget({ platform: "Win32", architecture: "x86", bitness: "64" }), "win32-x64");
assert.equal(inferNativeTarget({ platform: "Linux aarch64", architecture: "arm", bitness: "64" }), "linux-arm64");
assert.equal(inferNativeTarget({ platform: "MacIntel", architecture: "arm", bitness: "64" }), "darwin-arm64");
assert.equal(inferNativeTarget({ platform: "MacIntel" }), null);
assert.equal(selectTargetVsix(fixtures, "freebsd-x64"), null);

expectFailure(validateHtml, html.replace('lang="en"', 'lang="ko"'), "non-English default");
expectFailure(
  validateHtml,
  html.replaceAll("itemName=runtrol.runtrol-studio", "itemName=runtrol.wrong-extension"),
  "wrong Marketplace identity",
);
expectFailure(validateHtml, html.replace("Secure phone app available", "Phone app unavailable"), "missing PWA claim");
expectFailure(
  validateScript,
  script.replace("selectTargetVsix(assets, target)", "assets.at(0)"),
  "unmatched native release asset",
);
expectFailure(validateTypography, `${html}\u2014`, "forbidden dash");

console.log("Site contract tests passed, including six native targets and five red-path mutations.");
