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
const automationData = { workflows: [], routines: [], routes: [], runs: [] };
let interrupted = false, recoveryView = null;
const recoveryActions = [];
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
        features: { chat: true, attachments: true, activity: true, recovery: true },
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
        if (body.type === "save_routine") automationData.routines = [{...body.routine, revision: body.routine.revision + 1, next_run_ms: body.routine.first_run_ms}];
        if (body.type === "run") automationData.runs = [{id:body.run_id,workflow_id:body.workflow_id,route_id:null,thread_id:body.run_id,created_ms:Date.now(),state:"pending",error:null}];
        if (body.type === "run_routine") automationData.runs = [{id:body.run_id,workflow_id:null,routine_id:body.routine_id,thread_id:body.run_id,created_ms:Date.now(),state:"pending",error:null}];
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
    else if (route.endsWith("/recovery")) {
      if (req.method === "POST") {
        recoveryActions.push(body);
        if (body.type === "inspect") recoveryView = {id:body.id, command_id:body.command_id,state:"ready",detail:null,checkpoint:{provider:"claude",session_id:"00000000-0000-0000-0000-000000000004",files:3,progress:"Created the initial project",digest:"a".repeat(64)}};
        else { assert.equal(body.id,recoveryView.id); assert.equal(body.digest,recoveryView.checkpoint.digest); recoveryView={...recoveryView,state:"resumed"}; }
        value = recoveryView;
      } else value = recoveryView ? [recoveryView] : [];
    }
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
                phase: interrupted ? "needs_attention" : "reading_files",
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
                state: interrupted ? "claimed" : "completed",
                outcome: interrupted ? null : {
                  type: "completed",
                  event: {
                    kind: {
                      type: "replied",
                      text: "**Done**\n\n| Item | Amount |\n| --- | --- |\n| Total | 42 |\n\n<script>window.injected=true</script>\n[Unsafe](javascript:alert)\n[Source](https://example.org)\n\n## Recommended next steps\n\nThe total is 42. Here is a practical plan for reviewing the project budget and keeping the next iteration focused.\n\n1. **Confirm the inputs.** Review each line with the team responsible for the purchase and record any missing context.\n2. **Separate recurring expenses.** Keep subscriptions visible so they do not get lost among one-time purchases.\n3. **Check the forecast.** Compare the approved budget with actual spending before committing to new work.\n4. **Assign an owner.** Give each follow-up a clear owner and a date so the review leads to action.\n5. **Save the result.** Keep the source file and the summary together for the next review.\n6. **Schedule a review.** Use a routine when you want the agent to repeat this check automatically.",
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
    "--disable-background-timer-throttling",
    "--disable-renderer-backgrounding",
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
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.sidebar')).paddingLeft"), '12px', 'layout layer remains above the reset');
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.conversation-header')).paddingLeft"), '24px');
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.transcript')).marginLeft !== '0px'"), true, 'transcript is centered');
  assert.match(await evaluate("getComputedStyle(document.documentElement).getPropertyValue('--primary').trim()"), /^oklch\((?:45%|0\.45) 0?\.085 224\.283\)$/, 'selected preset owns the primary color');
  assert.match(await evaluate("getComputedStyle(document.body).fontFamily"), /Geist Variable/, 'selected body typeface');
  assert.equal(themeMetrics.inputHeight, '28px', 'shadcn input size wins over layout CSS');
  assert.equal(themeMetrics.inputBorder, '0px', 'input group owns the shared search boundary');
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.history-search')).borderTopWidth"), '1px');
  assert.equal(themeMetrics.sendWidth, '28px', 'shadcn icon button sizing');
  assert.equal(themeMetrics.sendHeight, '28px');
  assert.notEqual(themeMetrics.card, themeMetrics.background, 'card and page tokens remain independent');
  const darkBackground = await evaluate("getComputedStyle(document.body).backgroundColor");
  await evaluate("document.documentElement.classList.remove('dark')");
  assert.notEqual(await evaluate("getComputedStyle(document.body).backgroundColor"), darkBackground, 'light palette responds to theme class');
  await evaluate("document.documentElement.classList.add('dark'); document.getElementById('chat-message').focus()");
  await until(() => evaluate("getComputedStyle(document.querySelector('.composer-input-group')).boxShadow.includes('2px')"));
  await evaluate("document.querySelector('button[aria-label=Plugins]').click()");
  await until(() => evaluate("document.body.textContent.includes('No plugins installed.')"));
  await evaluate("document.querySelector('button[aria-label=Routines]').click()");
  await until(() => evaluate("document.querySelector('.section-header button')?.disabled===false"));
  assert.equal(automationData.workflows.length, 0, 'routine requires no workflow');
  await evaluate("document.querySelector('.section-header button').click()");
  await until(() => evaluate("document.querySelector('[role=dialog] input[name=first]')!==null"));
  await evaluate(`document.querySelector('[role=dialog] input[name=name]').value='Morning briefing';document.querySelector('[role=dialog] textarea').value='Review project updates and summarize today’s priorities.';document.querySelector('[role=dialog] form').requestSubmit()`);
  await until(() => evaluate("document.querySelector('button[aria-label=\"Edit Morning briefing\"]')!==null"));
  assert.equal(automationData.workflows.length,0);
  await evaluate("document.querySelector('button[aria-label=\"Run Morning briefing\"]').click()");
  await until(() => automationData.runs.length===1);
  await until(() => evaluate('document.querySelector(' + JSON.stringify('button[aria-label="Pause Morning briefing"]') + ')?.disabled === false'));
  await evaluate("document.querySelector('button[aria-label=\"Pause Morning briefing\"]').click()");
  await until(() => evaluate("document.body.textContent.includes('Paused')"));
  await evaluate("document.querySelector('button[aria-label=\"Resume Morning briefing\"]').click()");
  await until(() => automationData.routines[0].enabled);
  await evaluate("Promise.all(document.getAnimations().filter(a => a.effect.getComputedTiming().iterations !== Infinity).map(a => a.finished.catch(() => {})))");
  await writeFile("/tmp/cuttlefish-routines-desktop.png",Buffer.from((await cmd("Page.captureScreenshot",{format:"png"})).data,"base64"));
  await evaluate("document.querySelector('button[aria-label=Workflows]').click()");
  await until(() => evaluate("document.querySelector('.section-header button')!==null"));
  await evaluate("document.querySelector('.section-header button').click()");
  await until(() => evaluate("document.querySelector('[role=dialog] textarea')!==null"));
  assert.equal(await evaluate("getComputedStyle(document.querySelector('[data-slot=dialog-title]')).fontSize"), '14px', 'dialog typography retains preset styling');
  await evaluate("Promise.all(document.getAnimations().filter(a => a.effect.getComputedTiming().iterations !== Infinity).map(a => a.finished.catch(() => {})))");
  await writeFile("/tmp/cuttlefish-workflow-editor.png",Buffer.from((await cmd("Page.captureScreenshot",{format:"png"})).data,"base64"));
  await evaluate(`document.querySelector('[role=dialog] input[name=name]').value='Project review';document.querySelector('[role=dialog] textarea').value='Review recent project changes and summarize the next steps.';document.querySelector('[role=dialog] form').requestSubmit()`);
  await until(() => evaluate("document.querySelector('button[aria-label=\"Edit Project review\"]')!==null"));
  await evaluate("document.querySelector('button[aria-label=\"Run Project review\"]').click()");
  await until(() => evaluate("document.body.textContent.includes('Queued')"));
  await evaluate("Promise.all(document.getAnimations().filter(a => a.effect.getComputedTiming().iterations !== Infinity).map(a => a.finished.catch(() => {})))");
  await writeFile("/tmp/cuttlefish-workflows-desktop.png",Buffer.from((await cmd("Page.captureScreenshot",{format:"png"})).data,"base64"));
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
  await evaluate(`document.querySelector('[aria-label="Conversation options"]').focus()`);
  await cmd("Input.dispatchKeyEvent", {type:"keyDown",key:"Enter",code:"Enter"});
  await cmd("Input.dispatchKeyEvent", {type:"keyUp",key:"Enter",code:"Enter"});
  await until(() => evaluate("!!document.querySelector('[role=menuitem]')"));
  assert.match(await evaluate("document.querySelector('[role=menuitem]').textContent"), /End session/);
  await cmd("Input.dispatchKeyEvent", {type:"keyDown",key:"Escape",code:"Escape"});
  await cmd("Input.dispatchKeyEvent", {type:"keyUp",key:"Escape",code:"Escape"});
  await until(() => evaluate("!document.querySelector('[role=menuitem]')"));
  await evaluate("document.getElementById('new-chat').focus()");
  await until(() => evaluate("!!document.querySelector('[role=tooltip]')"));
  await evaluate("document.getElementById('chat-message').focus()");
  await until(() => evaluate("!document.querySelector('[role=tooltip]')"));
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.composer-actions')).justifyContent"), "space-between");
  assert.equal(
    await evaluate("document.documentElement.scrollWidth>innerWidth"),
    false,
  );
  await evaluate("Promise.all(document.getAnimations().filter(a => a.effect.getComputedTiming().iterations !== Infinity).map(a => a.finished.catch(() => {})))");
  await evaluate("document.getElementById('conversation-scroll').scrollTop=0");
  const desktopScreenshot = await cmd("Page.captureScreenshot", {
    format: "png",
  });
  await writeFile(
    "/tmp/cuttlefish-react-desktop.png",
    Buffer.from(desktopScreenshot.data, "base64"),
  );
  interrupted = true;
  await cmd("Page.reload");
  await until(() => evaluate(`!!document.querySelector('[aria-label="Recover interrupted task"] button:not([disabled])')`));
  await evaluate(`document.querySelector('[aria-label="Recover interrupted task"] button').click()`);
  await until(() => evaluate(`document.querySelector('[aria-label="Recover interrupted task"]')?.textContent.includes('Continue from checkpoint')`));
  assert.equal(recoveryActions.length,1,"inspection is a separate read-only operation");
  await cmd("Page.reload");
  await until(() => evaluate(`document.querySelector('[aria-label="Recover interrupted task"]')?.textContent.includes('Continue from checkpoint')`));
  await evaluate(`document.querySelector('[aria-label="Recover interrupted task"]').scrollIntoView({block:'center'})`);
  await writeFile("/tmp/cuttlefish-recovery-review.png",Buffer.from((await cmd("Page.captureScreenshot",{format:"png"})).data,"base64"));
  await evaluate(`document.querySelector('[aria-label="Recover interrupted task"] button').click()`);
  await until(() => evaluate(`document.querySelector('[aria-label="Recover interrupted task"]')?.textContent.includes('Continuation saved')`));
  assert.equal(recoveryActions.length,2);
  assert.equal(requests.length,2,"recovery must not repost the original chat request");
  interrupted = false;
  await cmd("Page.reload");
  await until(() => evaluate(`document.querySelector('.activity-title')?.textContent==='Completed'`));
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
  assert.equal(await evaluate("document.activeElement.id"), "navigation-title");
  assert.equal(await evaluate("!!document.querySelector('[role=tooltip]')"), false);
  await evaluate("document.querySelector('button[aria-label=Routines]').click()");
  await until(() => evaluate("document.activeElement.id==='routines-heading'"));
  await until(() => evaluate(`!!document.querySelector('button[aria-label="Edit Morning briefing"]')`));
  await evaluate(`document.querySelector('button[aria-label="Edit Morning briefing"]').click()`);
  await until(() => evaluate("!!document.querySelector('[role=dialog] textarea')"));
  assert.equal(await evaluate("document.documentElement.scrollWidth>innerWidth"), false);
  await writeFile("/tmp/cuttlefish-routine-editor-mobile.png", Buffer.from((await cmd("Page.captureScreenshot",{format:"png"})).data,"base64"));
  await cmd("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape",code:"Escape"});
  await cmd("Input.dispatchKeyEvent",{type:"keyUp",key:"Escape",code:"Escape"});
  await until(() => evaluate("!document.querySelector('[role=dialog]')"));
  await evaluate("document.getElementById('mobile-menu').click()");
  await until(() => evaluate("!!document.querySelector('button[aria-label=Chats]')"));
  await evaluate("document.querySelector('button[aria-label=Chats]').click()");
  await until(() => evaluate("document.activeElement.id==='chat-message'"));
  await writeFile("/tmp/cuttlefish-chat-mobile.png", Buffer.from((await cmd("Page.captureScreenshot",{format:"png"})).data,"base64"));
  await evaluate("document.getElementById('mobile-menu').click()");
  await until(() => evaluate("!!document.getElementById('logout')"));
  await evaluate("document.getElementById('logout').click()");
  await until(() => evaluate("!!document.getElementById('auth')"));
  assert.equal(
    await evaluate("document.querySelectorAll('#messages .message').length"),
    0,
  );
  await evaluate("document.querySelector('input[name=username]').focus()");
  await cmd("Input.insertText", {text:"draft-user"});
  await evaluate("document.querySelector('input[name=password]').focus(); window.passwordField=document.activeElement; void 0");
  await cmd("Input.insertText", {text:"unfinished-password"});
  // Cross two of the previous five-second refresh intervals while typing.
  await new Promise(resolve => setTimeout(resolve, 11000));
  assert.equal(await evaluate("document.querySelector('input[name=password]')===window.passwordField && document.activeElement===window.passwordField && window.passwordField.value==='unfinished-password'"),true,"sign-in stays mounted and focused across polling intervals");
  assert.equal(await evaluate("document.querySelector('input[name=username]').value"),"draft-user");
  console.log(
    "PASS: workflows, independent routines, navigation, attachments, durable retries, safe Markdown, responsive layout, sign-out cleanup, and uninterrupted sign-in drafts",
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
