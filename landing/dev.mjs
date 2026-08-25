// Local preview server. Serves this folder, the shared release-assets module from site/, and the
// brand SSOT from assets/brand/ so index.html works unchanged against the same relative paths the
// Pages build emits. Loopback only: this is a development surface, never a deployment.

import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { dirname, extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const landingRoot = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = dirname(landingRoot);
const port = Number(process.env.PORT ?? 4173);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".json": "application/json; charset=utf-8",
};

function resolvePath(urlPath) {
  const clean = normalize(decodeURIComponent(urlPath)).replaceAll("\\", "/");
  if (clean.includes("..")) {
    return null;
  }
  if (clean === "/" || clean === "/index.html") {
    return join(landingRoot, "index.html");
  }
  if (clean === "/release-assets.mjs") {
    return join(repositoryRoot, "site", "release-assets.mjs");
  }
  if (clean.startsWith("/assets/brand/")) {
    return join(repositoryRoot, "assets", "brand", clean.slice("/assets/brand/".length));
  }
  if (clean === "/app" || clean.startsWith("/app/")) {
    return null;
  }
  return join(landingRoot, clean.slice(1));
}

const server = createServer(async (request, response) => {
  const path = resolvePath(new URL(request.url ?? "/", "http://localhost").pathname);
  try {
    if (path === null || !(await stat(path)).isFile()) {
      throw new Error("not a file");
    }
    const body = await readFile(path);
    response.writeHead(200, {
      "content-type": TYPES[extname(path)] ?? "application/octet-stream",
      "cache-control": "no-store",
    });
    response.end(body);
  } catch {
    // A missing path is a 404 for the browser, not a crash for the server.
    response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    response.end(`404 ${request.url}\n`);
  }
});

server.listen(port, "127.0.0.1", () => {
  console.log(`runtrol landing preview: http://127.0.0.1:${port}/  (phone app route /app/ is not served here)`);
});
