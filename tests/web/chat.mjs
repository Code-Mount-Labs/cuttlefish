/** Browser contract test. Uses a disposable Chrome profile and a local JSON fixture server. */
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { once } from "node:events";

const browserPath = process.env.BROWSER_BIN;
assert.ok(browserPath, "Set BROWSER_BIN to a Chrome or Chromium executable.");
const root = resolve(import.meta.dirname, "../..");
const base = process.env.CHAT_TEST_BASE || "";
assert.match(base, /^(?:\/i\/[a-f0-9-]{36})?$/);
const directory = await mkdtemp(join(tmpdir(), "cuttlefish-chat-test-"));
const workspace = {
  workspace_id: "00000000-0000-0000-0000-000000000005",
  name: "Research",
  provider: "codex",
  allow_edits: true,
};
const automationData = { workflows: [], routes: [], runs: [] };
const requests = [],
  uploads = [];
let accepted = null,
  failNext = true,
  signedIn = true;
const server = createServer(async (req, res) => {
  try {
    const fullPath = new URL(req.url, "http://localhost").pathname;
    if (base && !fullPath.startsWith(base + "/")) {
      res.writeHead(404).end();
      return;
    }
    const path = fullPath.slice(base.length);
    res.setHeader(
      "Content-Security-Policy",
      "default-src 'self'; script-src 'self'; style-src 'self'; font-src 'self' data:; img-src 'self' blob: data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
    );
    if (!path.startsWith("/api/local/")) {
      const asset = {
        "/": "index.html",
        "/app.js": "app.js",
        "/app.css": "app.css",
      }[path];
      if (!asset) {
        res.writeHead(404).end();
        return;
      }
      res.setHeader(
        "Content-Type",
        asset.endsWith(".js")
          ? "text/javascript"
          : asset.endsWith(".css")
            ? "text/css"
            : "text/html",
      );
      res.end(await readFile(join(root, "web/local", asset)));
      return;
    }
    let bytes = "";
    for await (const chunk of req) bytes += chunk;
    const body = bytes ? JSON.parse(bytes) : null;
    const route = path.slice("/api/local/".length);
    let value;
    if (route === "config" && !signedIn) {
      res
        .writeHead(401, { "Content-Type": "application/json" })
        .end(JSON.stringify({ error: "unauthorized" }));
      return;
    }
    if (route === "status") value = { initialized: true };
    else if (route === "config")
      value = {
        features: { chat: true, attachments: true, activity: true },
        role: "router_agent",
        name: "Browser test",
        group_id: "test",
        database: { state: "ready" },
        remote: { revision: 1, choice: { mode: "local" }, state: "local" },
        model: { configured: false },
      };
    else if (route === "automations") {
      if (req.method === "POST") {
        if (body.type === "save_workflow") automationData.workflows = [{...body.workflow, revision: body.workflow.revision + 1}];
        if (body.type === "save_route") automationData.routes = [{...body.route, revision: body.route.revision + 1, next_run_ms: body.route.first_run_ms}];
        if (body.type === "run") automationData.runs = [{id:body.run_id,workflow_id:body.workflow_id,route_id:null,thread_id:body.run_id,created_ms:Date.now(),state:"pending",error:null}];
        value = {ok:true};
      } else value = automationData;
    }
    else if (route === "chat/workspaces") value = [{ workspace }];
    else if (route === "chat" && req.method === "GET")
      value = accepted
        ? [
            {
              thread_id: accepted.thread_id,
              workspace_id: workspace.workspace_id,
              request: accepted,
              title: accepted.action.message.text,
            },
          ]
        : [];
    else if (route.endsWith("/activity"))
      value = {
        events: accepted
          ? [
              {
                sequence: 1,
                command_id: accepted.command_id,
                provider: "claude",
                phase: "submitted",
                at_ms: Date.now() - 3000,
              },
              {
                sequence: 2,
                command_id: accepted.command_id,
                provider: "claude",
                phase: "reading_files",
                at_ms: Date.now() - 2000,
              },
            ]
          : [],
        last_seen_ms: Date.now(),
      };
    else if (route.startsWith("chat/"))
      value =
        accepted && route === "chat/" + accepted.thread_id
          ? [
              {
                request: accepted,
                state: "completed",
                outcome: {
                  type: "completed",
                  event: {
                    kind: {
                      type: "replied",
                      text: "**Done**\n\n| Item | Amount |\n| --- | --- |\n| Total | 42 |\n\n<script>window.injected=true</script>\n[Unsafe](javascript:alert)\n[Source](https://example.org)",
                    },
                    artifacts: [],
                  },
                },
              },
            ]
          : [];
    else if (route === "attachments" && req.method === "POST") {
      uploads.push(body);
      value = { ready: true };
    } else if (route === "chat" && req.method === "POST") {
      requests.push(body);
      if (failNext) {
        failNext = false;
        res
          .writeHead(503, { "Content-Type": "application/json" })
          .end(JSON.stringify({ error: "temporary_delivery_failure" }));
        return;
      }
      accepted = body;
      value = { request: body, state: "claimed", outcome: null };
    } else if (route === "logout") {
      signedIn = false;
      value = { ok: true };
    } else {
      res
        .writeHead(404)
        .end(JSON.stringify({ error: "unknown_fixture_route" }));
      return;
    }
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify(value));
  } catch (error) {
    res.writeHead(500).end(JSON.stringify({ error: error.message }));
  }
});
server.listen(0, "127.0.0.1");
await once(server, "listening");
const origin = `http://127.0.0.1:${server.address().port}${base}/`;
const browser = spawn(
  browserPath,
  [
    "--headless",
    "--remote-debugging-port=0",
    "--user-data-dir=" + join(directory, "profile"),
    "--no-first-run",
    "about:blank",
  ],
  { stdio: ["ignore", "ignore", "pipe"] },
);
let log = "";
browser.stderr.on("data", (b) => (log += b));
let ws;
async function until(check) {
  for (let i = 0; i < 150; i++) {
    const result = await check();
    if (result) return result;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw Error("Browser assertion timed out");
}
try {
  const address = await until(
    () => log.match(/DevTools listening on (ws:\/\/[^\s]+)/)?.[1],
  );
  ws = new WebSocket(address);
  await new Promise((r, j) => {
    ws.onopen = r;
    ws.onerror = j;
  });
  let sequence = 0;
  const pending = new Map();
  ws.onmessage = (event) => {
    const value = JSON.parse(event.data);
    const task = pending.get(value.id);
    if (task) {
      pending.delete(value.id);
      value.error ? task[1](value.error) : task[0](value.result);
    }
  };
  const send = (method, params = {}, sessionId) =>
    new Promise((r, j) => {
      const id = ++sequence;
      pending.set(id, [r, j]);
      ws.send(JSON.stringify({ id, method, params, sessionId }));
    });
  const { targetId } = await send("Target.createTarget", {
    url: "about:blank",
  });
  const { sessionId } = await send("Target.attachToTarget", {
    targetId,
    flatten: true,
  });
  const cmd = (method, params) => send(method, params, sessionId);
  const evaluate = async (expression) => {
    const result = await cmd("Runtime.evaluate", {
      expression,
      returnByValue: true,
      awaitPromise: true,
    });
    assert.ok(
      !result.exceptionDetails,
      JSON.stringify(result.exceptionDetails),
    );
    return result.result.value;
  };
  await cmd("Page.enable");
  await cmd("Page.addScriptToEvaluateOnNewDocument", {
    source:
      "window.cspViolations=[];document.addEventListener('securitypolicyviolation',e=>window.cspViolations.push(e.violatedDirective));",
  });
  await cmd("Runtime.enable");
  await cmd("Emulation.setDeviceMetricsOverride", {
    width: 1440,
    height: 1000,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await cmd("Page.navigate", { url: origin });
  await until(() =>
    evaluate("document.getElementById('chat-workspace')?.options.length===1"),
  );
  await evaluate("document.fonts.ready");
  assert.equal(await evaluate("document.fonts.check('12px \"Geist Variable\"') && document.fonts.check('14px \"IBM Plex Sans Variable\"')"), true, 'preset fonts load locally');
  const themeMetrics = await evaluate(`(() => {
    const root = getComputedStyle(document.documentElement);
    const input = getComputedStyle(document.getElementById('chat-search'));
    const send = getComputedStyle(document.getElementById('chat-send'));
    return {rootSize:root.fontSize,inputHeight:input.height,inputBorder:input.borderTopWidth,sendWidth:send.width,sendHeight:send.height,card:root.getPropertyValue('--card'),background:root.getPropertyValue('--background')};
  })()`);
  assert.equal(themeMetrics.rootSize, '16px', 'browser default scale is preserved');
  assert.match(await evaluate("getComputedStyle(document.documentElement).getPropertyValue('--primary').trim()"), /^oklch\((?:45%|0\.45) 0?\.085 224\.283\)$/, 'selected preset owns the primary color');
  assert.match(await evaluate("getComputedStyle(document.body).fontFamily"), /Geist Variable/, 'selected body typeface');
  assert.equal(themeMetrics.inputHeight, '28px', 'shadcn input size wins over layout CSS');
  assert.equal(themeMetrics.inputBorder, '1px', 'search retains its input treatment');
  assert.equal(themeMetrics.sendWidth, '28px', 'shadcn icon button sizing');
  assert.equal(themeMetrics.sendHeight, '28px');
  assert.notEqual(themeMetrics.card, themeMetrics.background, 'card and page tokens remain independent');
  const darkBackground = await evaluate("getComputedStyle(document.body).backgroundColor");
  await evaluate("document.documentElement.classList.remove('dark')");
  assert.notEqual(await evaluate("getComputedStyle(document.body).backgroundColor"), darkBackground, 'light palette responds to theme class');
  await evaluate("document.documentElement.classList.add('dark'); document.getElementById('chat-message').focus()");
  await until(() => evaluate("getComputedStyle(document.getElementById('chat-message')).boxShadow.includes('2px')"));
  await evaluate("document.querySelector('button[aria-label=Plugins]').click()");
  await until(() => evaluate("document.body.textContent.includes('No plugins installed.')"));
  await evaluate("document.querySelector('button[aria-label=Workflows]').click()");
  await until(() => evaluate("document.querySelector('.section-header button')!==null"));
  await evaluate("document.querySelector('.section-header button').click()");
  await until(() => evaluate("document.querySelector('[role=dialog] textarea')!==null"));
  assert.equal(await evaluate("getComputedStyle(document.querySelector('[data-slot=dialog-title]')).fontSize"), '14px', 'dialog typography is not reset by global headings');
  await evaluate("Promise.all(document.getAnimations().filter(a => a.effect.getComputedTiming().iterations !== Infinity).map(a => a.finished.catch(() => {})))");
  const editorScreenshot = await cmd("Page.captureScreenshot", {format:"png"});
  await writeFile("/tmp/cuttlefish-workflow-editor.png",Buffer.from(editorScreenshot.data,"base64"));
  await evaluate(`document.querySelector('[role=dialog] input[name=name]').value='Project review';document.querySelector('[role=dialog] textarea').value='Review recent project changes and summarize the next steps.';document.querySelector('[role=dialog] form').requestSubmit()`);
  await until(() => evaluate("document.querySelector('button[aria-label=\"Edit Project review\"]')!==null"));
  await evaluate("[...document.querySelectorAll('.automation-list button')].find(b=>b.textContent==='Run').click()");
  await until(() => evaluate("document.body.textContent.includes('queued. Open the run')"));
  await evaluate("document.querySelector('button[aria-label=Routes]').click()");
  await until(() => evaluate("document.querySelector('.section-header button')?.disabled===false"));
  await evaluate("document.querySelector('.section-header button').click()");
  await until(() => evaluate("document.querySelector('[role=dialog] input[name=first]')!==null"));
  await evaluate(`document.querySelector('[role=dialog] input[name=name]').value='Morning review';document.querySelector('[role=dialog] form').requestSubmit()`);
  await until(() => evaluate("document.querySelector('button[aria-label=\"Edit Morning review\"]')!==null"));
  await evaluate("[...document.querySelectorAll('.automation-list button')].find(b=>b.textContent==='Pause').click()");
  await until(() => evaluate("document.body.textContent.includes('Paused')"));
  await evaluate("[...document.querySelectorAll('.automation-list button')].find(b=>b.textContent==='Resume').click()");
  await until(() => evaluate("document.body.textContent.includes('Next run')"));
  await evaluate("Promise.all(document.getAnimations().filter(a => a.effect.getComputedTiming().iterations !== Infinity).map(a => a.finished.catch(() => {})))");
  const routeScreenshot = await cmd("Page.captureScreenshot", {format:"png"});
  await writeFile("/tmp/cuttlefish-routes-desktop.png",Buffer.from(routeScreenshot.data,"base64"));
  await evaluate("document.querySelector('button[aria-label=Workflows]').click()");
  await evaluate("Promise.all(document.getAnimations().filter(a => a.effect.getComputedTiming().iterations !== Infinity).map(a => a.finished.catch(() => {})))");
  const workflowScreenshot = await cmd("Page.captureScreenshot", {format:"png"});
  await writeFile("/tmp/cuttlefish-workflows-desktop.png",Buffer.from(workflowScreenshot.data,"base64"));
  await evaluate("document.querySelector('button[aria-label=Chats]').click()");
  await writeFile(join(directory, "costs.csv"), "item,amount\nDesign,42\n");
  const { root: documentRoot } = await cmd("DOM.getDocument");
  const { nodeId } = await cmd("DOM.querySelector", {
    nodeId: documentRoot.nodeId,
    selector: "#file-picker",
  });
  await cmd("DOM.setFileInputFiles", {
    nodeId,
    files: [join(directory, "costs.csv")],
  });
  await until(() =>
    evaluate(
      "document.querySelectorAll('#draft-attachments .file-card').length===1",
    ),
  );
  await evaluate(
    "Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(document.getElementById('chat-message'),'Summarize the costs');document.getElementById('chat-message').dispatchEvent(new Event('input',{bubbles:true}))",
  );
  await evaluate("document.getElementById('chat-form').requestSubmit()");
  await until(() =>
    evaluate(
      "document.getElementById('notice').textContent.length>0&&!document.getElementById('chat-send').disabled",
    ),
  );
  assert.equal(requests.length, 1);
  assert.equal(uploads.length, 1);
  assert.equal(
    Buffer.from(uploads[0].data, "base64").toString(),
    "item,amount\nDesign,42\n",
  );
  await cmd("Page.reload");
  await until(() =>
    evaluate(
      "document.getElementById('chat-send')?.getAttribute('aria-label')==='Retry same request'&&!document.getElementById('chat-send').disabled",
    ),
  );
  await evaluate("document.getElementById('chat-form').requestSubmit()");
  await until(() =>
    evaluate("document.querySelectorAll('#messages .agent').length===1"),
  );
  assert.equal(
    await evaluate("document.querySelector('.activity-title').textContent"),
    "Completed",
  );
  await evaluate("document.querySelector('.task-activity summary').click()");
  assert.equal(
    await evaluate("document.querySelector('.task-activity details').open"),
    true,
  );
  assert.ok(
    (
      await evaluate("document.querySelector('.activity-timeline').textContent")
    ).includes("Claude Code is reading files"),
  );
  assert.equal(requests.length, 2);
  assert.deepEqual(requests[0], requests[1]);
  assert.equal(
    uploads.length,
    1,
    "Reloaded retry must reuse uploaded references",
  );
  assert.equal(
    await evaluate("document.querySelectorAll('#messages table').length"),
    1,
  );
  assert.equal(
    await evaluate(
      "document.querySelectorAll('#messages script,#messages a[href^=javascript]').length",
    ),
    0,
  );
  assert.equal(await evaluate("window.injected===true"), false);
  assert.equal(
    await evaluate("document.documentElement.scrollWidth>innerWidth"),
    false,
  );
  await evaluate("Promise.all(document.getAnimations().filter(a => a.effect.getComputedTiming().iterations !== Infinity).map(a => a.finished.catch(() => {})))");
  const desktopScreenshot = await cmd("Page.captureScreenshot", {
    format: "png",
  });
  await writeFile(
    "/tmp/cuttlefish-react-desktop.png",
    Buffer.from(desktopScreenshot.data, "base64"),
  );
  await cmd("Emulation.setDeviceMetricsOverride", {
    width: 390,
    height: 844,
    deviceScaleFactor: 1,
    mobile: true,
  });
  assert.equal(
    await evaluate("document.documentElement.scrollWidth>innerWidth"),
    false,
  );
  await until(() => evaluate("!!document.getElementById('mobile-menu')"));
  await evaluate("document.getElementById('mobile-menu').click()");
  await until(() => evaluate("!!document.querySelector('[role=dialog]')"));
  assert.equal(
    await evaluate("!!document.querySelector('[role=dialog]')"),
    true,
  );
  await cmd("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: 350,
    y: 400,
    button: "left",
    clickCount: 1,
  });
  await cmd("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: 350,
    y: 400,
    button: "left",
    clickCount: 1,
  });
  await until(() => evaluate("!document.querySelector('[role=dialog]')"));
  assert.equal(await evaluate("document.activeElement.id"), "mobile-menu");
  await evaluate("document.getElementById('mobile-menu').click()");
  await until(() => evaluate("!!document.querySelector('[role=dialog]')"));
  assert.deepEqual(
    await evaluate("window.cspViolations"),
    [],
    "UI must respect the production content security policy",
  );
  await evaluate("Promise.all(document.getAnimations().filter(a => a.effect.getComputedTiming().iterations !== Infinity).map(a => a.finished.catch(() => {})))");
  const screenshot = await cmd("Page.captureScreenshot", { format: "png" });
  await writeFile(
    "/tmp/cuttlefish-react-mobile.png",
    Buffer.from(screenshot.data, "base64"),
  );
  await evaluate("document.getElementById('logout').click()");
  await until(() => evaluate("!!document.getElementById('auth')"));
  assert.equal(
    await evaluate("document.querySelectorAll('#messages .message').length"),
    0,
  );
  console.log(
    "PASS: workflows, scheduled routes, navigation, attachments, durable retries, safe Markdown, responsive layout, and sign-out cleanup",
  );
} finally {
  ws?.close();
  browser.kill("SIGTERM");
  await once(browser, "exit");
  server.closeAllConnections();
  await new Promise((r) => server.close(r));
  // Chrome helper processes can finish writing just after the main process exits.
  await rm(directory, {
    recursive: true,
    force: true,
    maxRetries: 10,
    retryDelay: 100,
  });
}
