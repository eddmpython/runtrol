// Contract test for the public landing: what must hold without JavaScript, what must never be fetched from
// outside, the two-tone mark, the channel row, and the sidebar scene. Every check is proven able to fail.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { ICON_NAMES, iconSvg } from "./icons.js";

const root = dirname(fileURLToPath(import.meta.url));
const html = await readFile(join(root, "index.html"), "utf8");
const css = await readFile(join(root, "styles.css"), "utf8");
const script = await readFile(join(root, "app.js"), "utf8");
const scene = await readFile(join(root, "scene.js"), "utf8");
const icons = await readFile(join(root, "icons.js"), "utf8");

const EN_DASH = String.fromCharCode(0x2013);
const EM_DASH = String.fromCharCode(0x2014);

function validateHtml(candidate) {
  assert.match(candidate, /<html lang="en"/u, "English must remain the static default");
  assert.match(candidate, /One VS Code sidebar\./u, "the North Star must work without JavaScript");
  assert.match(candidate, /id="install"/u, "installation instructions must be statically available");
  assert.match(candidate, /30 conversations/u, "the 30-session contract must be visible");
  assert.match(candidate, /id="scene-cursor"/u, "the hero scene must carry its cursor");
  assert.match(
    candidate,
    /https:\/\/marketplace\.visualstudio\.com\/items\?itemName=runtrol\.runtrol-studio/u,
    "the public Marketplace route must be statically available",
  );
  assert.match(candidate, /Install from Marketplace/u, "the public Marketplace action must be visible");
  assert.match(candidate, /Secure phone app available/u, "the shipped PWA state must be visible");
  assert.match(candidate, /href="app\/"/u, "the phone app route must be statically available");
  assert.match(candidate, /class="mark-accent"/u, "the mark must carry accent arms");
  assert.match(candidate, /class="mark-ink"/u, "the mark must carry ink arms so it is two-tone, not orange only");
  assert.match(candidate, /id="sidebar"/u, "the sidebar scene anchor must exist");
  assert.match(candidate, /id="usage-claude"/u, "the usage meter must be part of the scene");
  assert.match(candidate, /lucide\.dev/u, "the Lucide attribution must be visible");
  const channelRow = candidate.match(/<nav class="channels"[\s\S]*?<\/nav>/u)?.[0] ?? "";
  for (const channel of ["github.com/eddmpython/runtrol", "buymeacoffee.com/eddmpython", "youtube.com/@eddmpython", "threads.com/@eddmpython"]) {
    assert.ok(channelRow.includes(channel), `channel row must link ${channel}`);
  }
  assert.doesNotMatch(candidate, /<link[^>]+(?:fonts\.googleapis|cdn\.)/u, "external style or font CDN is forbidden");
  assert.doesNotMatch(candidate, /<script[^>]+https?:\/\//u, "external script CDN is forbidden");
  assert.doesNotMatch(candidate, /#FF5A2F/iu, "the landing accent is the coral, not the legacy orange");
}

function validateIcons(candidateHtml) {
  const used = [...candidateHtml.matchAll(/data-icon="([a-z-]+)"/gu)].map((match) => match[1]);
  assert.ok(used.length > 0, "the page must place lucide icons");
  for (const name of used) {
    assert.ok(ICON_NAMES.includes(name), `icon used in HTML but not vendored: ${name}`);
  }
  assert.match(iconSvg("zap"), /^<svg class="lucide lucide-zap"/u);
  assert.throws(() => iconSvg("does-not-exist"));
}

function validateScript(candidate) {
  for (const locale of ["en", "ko", "zh", "ja"]) {
    assert.match(candidate, new RegExp(`\\b${locale}: \\{`, "u"), `missing locale: ${locale}`);
  }
  assert.match(candidate, /releases\/latest/u, "release discovery must use the latest release API");
  assert.doesNotMatch(candidate, /navigator\.language/u, "English is the default; the browser locale must not pick the language");
  assert.match(candidate, /selectTargetVsix\(assets, target\)/u, "manual install must select a native target");
  assert.doesNotMatch(candidate, /version\s*[:=]\s*["']\d+\.\d+/u, "the page must not hardcode a release version");
  assert.doesNotMatch(
    candidate,
    /localStorage\.(?:setItem|getItem)\(\s*["'](?:conversation|transcript)/iu,
    "client code must not persist conversation content",
  );
}

function validateScene(candidate) {
  assert.match(candidate, /prefers-reduced-motion/u, "the scene must respect reduced motion");
  assert.match(candidate, /IntersectionObserver/u, "the scene must pause off screen");
  assert.match(candidate, /LOOP_MS/u, "the scene must loop from one clock");
}

function validateCss(candidate) {
  assert.match(candidate, /--accent: #f56565/u, "the accent token must be the coral");
  assert.match(candidate, /\.mark-ink\s*\{\s*stroke: currentColor/u, "ink arms must follow the theme");
  assert.match(candidate, /html\[data-theme="dark"\]/u, "a dark theme must exist");
}

function validateTypography(...candidates) {
  for (const candidate of candidates) {
    assert.equal(candidate.includes(EN_DASH), false, "en dash is forbidden");
    assert.equal(candidate.includes(EM_DASH), false, "em dash is forbidden");
  }
}

function expectFailure(check, candidate, label) {
  assert.throws(() => check(candidate), undefined, `${label} mutation must fail`);
}

validateHtml(html);
validateIcons(html);
validateScript(script);
validateScene(scene);
validateCss(css);
validateTypography(html, css, script, scene, icons);

expectFailure(validateHtml, html.replace('lang="en"', 'lang="ko"'), "non-English default");
expectFailure(
  validateHtml,
  html.replaceAll("itemName=runtrol.runtrol-studio", "itemName=runtrol.wrong-extension"),
  "wrong Marketplace identity",
);
expectFailure(validateHtml, html.replaceAll('class="mark-ink"', 'class="mark-accent"'), "orange-only mark");
expectFailure(validateHtml, html.replace("Secure phone app available", "Phone app unavailable"), "missing PWA claim");
expectFailure(validateHtml, html.replace("https://www.threads.com/@eddmpython", "#"), "missing channel");
expectFailure(validateIcons, html.replace('data-icon="zap"', 'data-icon="not-vendored"'), "unvendored icon");
expectFailure(validateCss, css.replace("--accent: #f56565", "--accent: #ff5a2f"), "legacy accent");
expectFailure(validateScene, scene.replace("prefers-reduced-motion", "prefers-motion"), "ignored reduced motion");
expectFailure(validateTypography, `${html}${EM_DASH}`, "forbidden dash");

console.log("Site contract tests passed, including nine red-path mutations.");
