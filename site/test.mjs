import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const html = await readFile(join(root, "index.html"), "utf8");
const css = await readFile(join(root, "styles.css"), "utf8");
const script = await readFile(join(root, "app.js"), "utf8");

function validateHtml(candidate) {
  assert.match(candidate, /<html lang="en"/u, "English must remain the static default");
  assert.match(candidate, /One VS Code window\./u, "the North Star must work without JavaScript");
  assert.match(candidate, /id="install"/u, "installation instructions must be statically available");
  assert.match(candidate, /30 conversations/u, "the 30-session contract must be visible");
  assert.match(candidate, /Marketplace release pending/u, "unpublished Marketplace state must be honest");
  assert.match(candidate, /Secure phone app in progress/u, "unfinished PWA state must be honest");
  assert.doesNotMatch(candidate, /<link[^>]+(?:fonts\.googleapis|cdn\.)/u, "external style or font CDN is forbidden");
  assert.doesNotMatch(candidate, /<script[^>]+https?:\/\//u, "external script CDN is forbidden");
}

function validateScript(candidate) {
  for (const locale of ["en", "ko", "zh", "ja"]) {
    assert.match(candidate, new RegExp(`\\b${locale}: \\{`, "u"), `missing locale: ${locale}`);
  }
  assert.match(candidate, /releases\/latest/u, "release discovery must use the latest release API");
  assert.match(candidate, /endsWith\("\.vsix"\)/u, "manual install must require a real VSIX asset");
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

expectFailure(validateHtml, html.replace('lang="en"', 'lang="ko"'), "non-English default");
expectFailure(validateHtml, html.replace("Marketplace release pending", "Download now"), "false Marketplace claim");
expectFailure(validateHtml, html.replace("Secure phone app in progress", "Install phone app"), "false PWA claim");
expectFailure(validateScript, script.replace('endsWith(".vsix")', 'endsWith(".zip")'), "non-VSIX release asset");
expectFailure(validateTypography, `${html}\u2014`, "forbidden dash");

console.log("Site contract tests passed, including five red-path mutations.");
