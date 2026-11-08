import assert from "node:assert/strict";
import { createServer as createHttpServer } from "node:http";
import { once } from "node:events";
import { createServer, preview } from "vite";

// Exercise the actual Vite entry points and their API proxy, including POST origins.
const api = createHttpServer((request, response) => {
  response.setHeader("content-type", "application/json");
  response.end(
    JSON.stringify({ origin: request.headers.origin, method: request.method }),
  );
});
api.listen(0, "127.0.0.1");
await once(api, "listening");
const target = `http://127.0.0.1:${api.address().port}`;
process.env.CUTTLEFISH_DEV_API = target;

try {
  for (const mode of ["development", "platform"]) {
    const server = await createServer({ mode });
    try {
      await server.listen();
      await verify(server.resolvedUrls.local[0], false);
    } finally {
      await server.close();
    }
  }
  for (const mode of ["production", "platform"]) {
    const server = await preview({ mode });
    try {
      await verify(server.resolvedUrls.local[0], true);
    } finally {
      await new Promise((resolve, reject) =>
        server.httpServer.close((error) => (error ? reject(error) : resolve())),
      );
    }
  }
} finally {
  api.closeAllConnections();
  await new Promise((resolve) => api.close(resolve));
}
console.log(
  "Vite dev and production preview passed for both web applications.",
);

async function verify(base, built) {
  const page = await fetch(base);
  assert.equal(page.status, 200);
  const html = await page.text();
  assert.match(html, /id="root"/);
  const script = html.match(/<script[^>]*src="([^"]+)"/)[1];
  assert.equal((await fetch(new URL(script, base))).status, 200);
  if (built) {
    assert.match(script, /\/assets\/.*-[\w-]+\.js$/);
    const stylesheet = html.match(/<link[^>]*href="([^"]+\.css)"/)[1];
    assert.equal((await fetch(new URL(stylesheet, base))).status, 200);
  }
  const own = await fetch(new URL("/api/verify", base), {
    method: "POST",
    headers: { Origin: new URL(base).origin },
  });
  assert.deepEqual(await own.json(), { origin: target, method: "POST" });
  const foreign = await fetch(new URL("/api/verify", base), {
    method: "POST",
    headers: { Origin: "https://unrelated.example" },
  });
  assert.deepEqual(await foreign.json(), {
    origin: "https://unrelated.example",
    method: "POST",
  });
}
