// Local preview of the landing source. Serves this folder plus the brand SSOT under assets/brand/ at the
// same relative paths the Pages build emits, so index.html runs unchanged. Loopback only; never a deploy.
// The phone app route app/ is not served here: run `npm --prefix site run build` and open dist/ for that.

import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { dirname, extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const siteRoot = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = dirname(siteRoot);
const port = Number(process.env.PORT ?? 4173);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
};

function resolvePath(urlPath) {
  const clean = normalize(decodeURIComponent(urlPath)).replaceAll("\\", "/");
  if (clean.includes("..")) {
    return null;
  }
  if (clean === "/" || clean === "/index.html") {
    return join(siteRoot, "index.html");
  }
  if (clean.startsWith("/assets/brand/")) {
    return join(repositoryRoot, "assets", "brand", clean.slice("/assets/brand/".length));
  }
  if (clean === "/app" || clean.startsWith("/app/") || clean.startsWith("/dist/")) {
    return null;
  }
  return join(siteRoot, clean.slice(1));
}

const server = createServer(async (request, response) => {
  const path = resolvePath(new URL(request.url ?? "/", "http://localhost").pathname);
  try {
    if (path === null || !(await stat(path)).isFile()) {
      throw new Error("not a file");
    }
    const body = await readFile(path);
    response.writeHead(200, { "content-type": TYPES[extname(path)] ?? "application/octet-stream", "cache-control": "no-store" });
    response.end(body);
  } catch {
    // A missing path is a 404 for the browser, not a crash for the preview.
    response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    response.end(`404 ${request.url}\n`);
  }
});

server.listen(port, "127.0.0.1", () => {
  console.log(`runtrol site preview: http://127.0.0.1:${port}/`);
});
