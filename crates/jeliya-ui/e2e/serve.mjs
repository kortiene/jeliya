// A tiny, dependency-free static server for the built `dist/`, used only by the
// no-network render smoke. It binds loopback and serves the reproducible
// artifact so the page loads with NO network access (§11, Verification).
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const port = Number(process.env.RENDER_PORT ?? 7461);
const distDir = fileURLToPath(new URL("../dist/", import.meta.url));

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
};

createServer(async (req, res) => {
  const rawPath = decodeURIComponent((req.url ?? "/").split("?")[0]);
  // Mirror the daemon's SPA fallback: an extension-less path serves the root
  // document, so nested routes (e.g. /rooms/r-99) load the app exactly as
  // serve_static does — which is what makes the nested-route smoke honest.
  const spa = rawPath === "/" || extname(rawPath) === "" ? "/index.html" : rawPath;
  const rel = normalize(spa).replace(/^(\.\.[/\\])+/, "");
  try {
    const bytes = await readFile(join(distDir, rel));
    res.writeHead(200, { "content-type": MIME[extname(rel)] ?? "application/octet-stream" });
    res.end(bytes);
  } catch {
    res.writeHead(404, { "content-type": "text/plain" });
    res.end("not found");
  }
}).listen(port, "127.0.0.1", () => {
  process.stdout.write(`render-smoke static server on http://127.0.0.1:${port}\n`);
});
