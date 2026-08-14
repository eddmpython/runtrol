import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

import { PyProcControlClient } from "pyproc/control";

const options = commandOptions(process.argv.slice(2));
const client = await PyProcControlClient.start(options.config);
let sessionRef = null;
try {
  const opened = await client.openTarget(options.url, {
    expectedRisk: "externalEffect",
    waitUntil: "load",
    timeoutMs: 30_000,
  });
  const attached = await client.attachSession(opened.output.targetRef, { timeoutMs: 10_000 });
  sessionRef = attached.output;
  const captured = await client.act(sessionRef, [
    {
      kind: "waitFor",
      expectedRisk: "read",
      selector: options.selector,
      state: "visible",
      timeoutMs: 10_000,
    },
    {
      kind: "screenshot",
      expectedRisk: "read",
      format: "png",
      inline: true,
    },
  ], { timeoutMs: 30_000 });
  const screenshot = captured.attachments.find((attachment) => attachment.kind === "screen.capture");
  if (!screenshot || screenshot.mimeType !== "image/png") {
    throw new Error("pyproc returned no verified PNG screenshot attachment");
  }
  await mkdir(path.dirname(options.output), { recursive: true });
  await writeFile(options.output, screenshot.bytes);
  process.stdout.write(`${JSON.stringify({
    output: options.output,
    bytes: screenshot.byteLength,
    sha256: screenshot.sha256,
  })}\n`);
} finally {
  try {
    if (sessionRef) await client.detachSession(sessionRef, { timeoutMs: 10_000 });
  } finally {
    await client.close();
  }
}

function commandOptions(arguments_) {
  const values = new Map();
  for (let index = 0; index < arguments_.length; index += 2) {
    const name = arguments_[index];
    const value = arguments_[index + 1];
    if (!name?.startsWith("--") || !value) throw new Error("visual options must be --name value pairs");
    values.set(name.slice(2), value);
  }
  const config = values.get("config");
  const url = values.get("url");
  const output = values.get("output");
  const selector = values.get("selector");
  if (!config || !path.isAbsolute(config)) throw new Error("--config must be an absolute path");
  if (!output || !path.isAbsolute(output)) throw new Error("--output must be an absolute path");
  if (!url || new URL(url).origin !== "http://127.0.0.1:4173") {
    throw new Error("--url must use the authorized local PWA origin");
  }
  if (!selector || selector.length > 200) throw new Error("--selector is required and bounded");
  return { config, url, output, selector };
}
