"use strict";
const base = location.pathname.match(/^\/i\/[a-f0-9-]{36}(?=\/|$)/)?.[0] || "";
const $ = (id) => document.getElementById(id);
let initialized = false,
  signedIn = false,
  remoteRevision = null,
  activityEnabled = false;
async function api(path, body) {
  const response = await fetch(base + "/api/local/" + path, {
    method: body === undefined ? "GET" : "POST",
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await response.json();
  if (!response.ok) {
    const error = new Error(data.error || "Request failed");
    error.status = response.status;
    throw error;
  }
  return data;
}
function notice(text) {
  $("notice").textContent = text;
}
async function action(form, job) {
  const buttons = [...form.querySelectorAll("button")];
  buttons.forEach((b) => (b.disabled = true));
  notice("");
  try {
    await job();
  } catch (error) {
    notice(error.message);
  } finally {
    buttons.forEach((b) => (b.disabled = false));
    if (form.id === "chat-form" || form.id === "chat-panel")
      await refreshChat().catch((error) => notice(error.message));
  }
}
function auth() {
  signedIn = false;
  $("mobile-menu").hidden = true;
  $("settings").hidden = true;
  $("auth").hidden = false;
  $("logout").hidden = true;
  $("loading").hidden = true;
  $("workspace-nav").hidden = true;
  $("chat-panel").hidden = true;
  $("token-label").hidden = initialized;
  $("setup-token").required = !initialized;
  $("auth-title").textContent = initialized
    ? "Welcome back."
    : "Set up this machine.";
  $("password").autocomplete = initialized
    ? "current-password"
    : "new-password";
}
async function refresh() {
  const data = await api("config");
  signedIn = true;
  $("mobile-menu").hidden = false;
  $("loading").hidden = true;
  $("auth").hidden = true;
  $("settings").hidden = false;
  $("logout").hidden = false;
  $("workspace-nav").hidden = false;
  activityEnabled = data.features?.activity === true;
  const canChat = data.features?.chat === true && data.role !== "agent";
  $("provider-settings").hidden =
    data.role === "router" || !data.features?.chat;
  $("show-chat").disabled = !canChat;
  if (!canChat || data.database.state !== "ready") chatVisible = false;
  $("chat-panel").hidden = !chatVisible;
  $("settings").hidden = chatVisible;
  $("machine-name").textContent = data.name;
  $("identity").textContent =
    `${data.role.replaceAll("_", " + ")} · Group ${data.group_id}`;
  const router = data.role !== "agent";
  $("router-settings").hidden = !router;
  $("agent-settings").hidden = data.role === "router";
  $("db-state").textContent = data.database.state;
  $("db-error").textContent = data.database.error_code || "";
  $("docker-help").hidden = !(data.database.error_code || "").startsWith(
    "docker_",
  );
  $("database-form").hidden =
    data.database.state !== "unconfigured" &&
    !(data.database.state === "failed" && data.database.mode === "existing");
  if (data.database.state === "failed" && data.database.mode === "existing") {
    $("database-mode").value = "existing";
    $("database-mode").dispatchEvent(new Event("change"));
  }
  $("database-retry").hidden = data.database.state !== "failed";
  $("database-ready").hidden = data.database.state !== "ready";
  showRemote(data.remote);
  $("model-status").textContent = data.model.configured
    ? `Configured: ${data.model.model}`
    : "No routing model configured. A single eligible agent is selected automatically.";
  if (chatVisible) await refreshChat();
}
$("auth-form").addEventListener("submit", (event) => {
  event.preventDefault();
  action(event.currentTarget, async () => {
    const input = {
      username: $("username").value,
      password: $("password").value,
    };
    if (!initialized) input.token = $("setup-token").value;
    await api(initialized ? "login" : "setup", input);
    initialized = true;
    chatVisible = true;
    $("chat-panel").hidden = false;
    $("password").value = "";
    $("setup-token").value = "";
    await refresh();
  });
});
$("logout").addEventListener("click", () =>
  action(document.querySelector("header"), async () => {
    await api("logout", {});
    savePending(null);
    clearDrafts();
    activeThread = null;
    chatRows = [];
    rendered = "";
    $("messages").replaceChildren();
    $("chat-list").replaceChildren();
    $("chat-message").value = "";
    history.replaceState(null, "", location.pathname);
    auth();
  }),
);
$("database-mode").addEventListener("change", () => {
  const mode = $("database-mode").value;
  $("port-label").hidden = mode !== "managed";
  $("url-label").hidden = mode !== "existing";
  $("managed-help").hidden = mode !== "managed";
  $("database-url").required = mode === "existing";
});
$("database-form").addEventListener("submit", (event) => {
  event.preventDefault();
  action(event.currentTarget, async () => {
    const mode = $("database-mode").value;
    const input = { mode };
    if (mode === "managed") input.port = Number($("database-port").value);
    if (mode === "existing") input.url = $("database-url").value;
    try {
      await api("database", input);
      $("database-url").value = "";
      notice(
        mode === "later"
          ? "You can set up the database here whenever you are ready."
          : "Database setup started. You can keep this page open to follow its progress.",
      );
    } finally {
      await refresh();
    }
  });
});
$("database-retry").addEventListener("click", () =>
  action($("router-settings"), async () => {
    try {
      await api("database/retry", {});
    } finally {
      await refresh();
    }
  }),
);
$("model-form").addEventListener("submit", (event) => {
  event.preventDefault();
  action(event.currentTarget, async () => {
    await api("model", {
      endpoint: $("model-endpoint").value,
      model: $("model-name").value,
      api_key: $("model-key").value,
    });
    $("model-key").value = "";
    await refresh();
    notice("Routing model saved on this machine.");
  });
});
setInterval(() => {
  if (signedIn && !document.hidden)
    refresh().catch((error) => {
      if (error.status === 401) auth();
      else notice(error.message);
    });
}, 5000);
(async () => {
  try {
    initialized = (await api("status")).initialized;
    try {
      await refresh();
    } catch (error) {
      if (error.status === 401) auth();
      else throw error;
    }
  } catch (error) {
    $("loading").hidden = true;
    notice(error.message);
  }
})();

function showRemote(remote) {
  if (remoteRevision !== remote.revision) {
    remoteRevision = remote.revision;
    $("remote-mode").value = remote.choice.mode;
    $("remote-origin").value = remote.choice.origin || "";
    $("remote-mode").dispatchEvent(new Event("change"));
  }
  $("remote-state").textContent =
    `${remote.state.replaceAll("_", " ")}${remote.error_code ? " · " + remote.error_code : ""}`;
  $("remote-link").hidden = true;
  if (remote.public_url && remote.state === "ready") {
    try {
      const url = new URL(remote.public_url);
      if (url.protocol === "https:") {
        $("remote-link").href = url.href.replace(/\/$/, "") + "/";
        $("remote-link").hidden = false;
      }
    } catch {}
  }
}
$("remote-mode").addEventListener("change", () => {
  const existing = $("remote-mode").value === "existing";
  $("remote-origin-label").hidden = !existing;
  $("remote-origin").required = existing;
});
$("remote-form").addEventListener("submit", (event) => {
  event.preventDefault();
  action(event.currentTarget, async () => {
    const choice = { mode: $("remote-mode").value };
    if (choice.mode === "existing") choice.origin = $("remote-origin").value;
    await api("remote", choice);
    await refresh();
    notice(
      "Remote access choice saved. Setup will resume automatically after a restart.",
    );
  });
});

let chatVisible = true,
  activeThread = location.hash.match(/^#chat\/([a-f0-9-]{36})$/)?.[1] || null,
  chatBusy = false,
  sending = false,
  chatRows = [],
  workspaceRows = [],
  draftFiles = [],
  rendered = "";
const pendingKey = "cuttlefish:pending:" + base;
let pendingChat = null;
try {
  pendingChat = JSON.parse(sessionStorage.getItem(pendingKey) || "null");
} catch {}
function savePending(value) {
  pendingChat = value;
  if (value) sessionStorage.setItem(pendingKey, JSON.stringify(value));
  else sessionStorage.removeItem(pendingKey);
}
function showPanel(chat) {
  document.body.classList.remove("sidebar-open");
  chatVisible = chat;
  $("chat-panel").hidden = !chat;
  $("settings").hidden = chat;
  if (chat) refreshChat().catch((error) => notice(error.message));
}
$("show-chat").addEventListener("click", () => showPanel(true));
$("show-settings").addEventListener("click", () => showPanel(false));
$("check-providers").addEventListener("click", () =>
  action($("provider-settings"), async () => {
    const providers = await api("providers");
    $("provider-status").textContent = providers
      .map(
        (p) =>
          `${p.provider === "codex" ? "Codex" : "Claude Code"}: ${!p.installed ? "install the CLI on this machine" : p.signed_in ? "signed in locally" : "sign in on this machine"}`,
      )
      .join(" · ");
    await localWorkspaces();
  }),
);
async function localWorkspaces() {
  const rows = await api("workspaces");
  $("local-workspaces").replaceChildren();
  for (const row of rows) {
    const p = document.createElement("p");
    p.textContent = `${row.workspace.name} · ${row.workspace.provider} · ${row.workspace.allow_edits ? "file edits allowed" : "read-only"} · ${row.directory}`;
    $("local-workspaces").append(p);
  }
}
let pendingWorkspace = null;
$("workspace-form").addEventListener("submit", (event) => {
  event.preventDefault();
  action(event.currentTarget, async () => {
    pendingWorkspace ??= {
      workspace_id: crypto.randomUUID(),
      name: $("workspace-name").value,
      provider: $("workspace-provider").value,
      allow_edits: $("workspace-edits").checked,
    };
    await api("workspaces", pendingWorkspace);
    pendingWorkspace = null;
    await localWorkspaces();
    notice(
      "Workspace created. It will appear in the chat picker after the agent checks in.",
    );
  });
});
function inline(parent, text, files = []) {
  const pattern = /(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\([^\s)]+\))/g;
  let offset = 0;
  for (const match of text.matchAll(pattern)) {
    parent.append(document.createTextNode(text.slice(offset, match.index)));
    const token = match[0];
    let el;
    if (token[0] === "`") {
      el = document.createElement("code");
      el.textContent = token.slice(1, -1);
    } else if (token.startsWith("**")) {
      el = document.createElement("strong");
      el.textContent = token.slice(2, -2);
    } else {
      const parts = token.match(/^\[([^\]]+)\]\((.*)\)$/);
      const file = files.find(
        (f) =>
          (parts[2].startsWith("artifacts/") || parts[2].startsWith("/")) &&
          parts[2].endsWith("/" + f.name),
      );
      if (file) {
        el = document.createElement("button");
        el.className = "inline-download";
        el.type = "button";
        el.textContent = parts[1];
        el.addEventListener("click", () =>
          downloadFile(file).catch((e) => notice(e.message)),
        );
      } else if (/^https?:\/\//.test(parts[2])) {
        el = document.createElement("a");
        el.textContent = parts[1];
        el.href = parts[2];
        el.target = "_blank";
        el.rel = "noopener noreferrer";
      } else {
        el = document.createElement("span");
        el.textContent = token;
      }
    }
    parent.append(el);
    offset = match.index + token.length;
  }
  parent.append(document.createTextNode(text.slice(offset)));
}
function markdown(parent, text, files = []) {
  const lines = text.split("\n");
  let paragraph = [];
  const flush = () => {
    if (paragraph.length) {
      const p = document.createElement("p");
      inline(p, paragraph.join("\n"), files);
      parent.append(p);
      paragraph = [];
    }
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (line.startsWith("```")) {
      flush();
      const code = document.createElement("code");
      const pre = document.createElement("pre");
      let body = [];
      while (++i < lines.length && !lines[i].startsWith("```"))
        body.push(lines[i]);
      code.textContent = body.join("\n");
      pre.append(code);
      parent.append(pre);
      continue;
    }
    if (
      line.includes("|") &&
      i + 1 < lines.length &&
      /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)+\|?\s*$/.test(lines[i + 1])
    ) {
      flush();
      const table = document.createElement("table");
      const row = (value, header) => {
        const tr = document.createElement("tr");
        for (const cell of value.replace(/^\s*\||\|\s*$/g, "").split("|")) {
          const td = document.createElement(header ? "th" : "td");
          inline(td, cell.trim(), files);
          tr.append(td);
        }
        table.append(tr);
      };
      row(line, true);
      i += 2;
      while (i < lines.length && lines[i].includes("|")) row(lines[i++], false);
      i--;
      const wrap = document.createElement("div");
      wrap.className = "table-scroll";
      wrap.append(table);
      parent.append(wrap);
      continue;
    }
    const heading = line.match(/^(#{1,4})\s+(.+)$/);
    if (heading) {
      flush();
      const h = document.createElement(
        "h" + Math.min(heading[1].length + 1, 4),
      );
      inline(h, heading[2], files);
      parent.append(h);
      continue;
    }
    if (/^\s*([-*]|\d+\.)\s+/.test(line)) {
      flush();
      const ordered = /^\s*\d+\./.test(line);
      const list = document.createElement(ordered ? "ol" : "ul");
      do {
        const li = document.createElement("li");
        inline(li, lines[i].replace(/^\s*([-*]|\d+\.)\s+/, ""), files);
        list.append(li);
        i++;
      } while (i < lines.length && /^\s*([-*]|\d+\.)\s+/.test(lines[i]));
      i--;
      parent.append(list);
      continue;
    }
    if (!line.trim()) flush();
    else paragraph.push(line);
  }
  flush();
}
const previewUrls = [];
function bytesLabel(size) {
  return size < 1024
    ? size + " B"
    : size < 1024 * 1024
      ? (size / 1024).toFixed(1) + " KB"
      : (size / 1024 / 1024).toFixed(1) + " MB";
}
function fileCard(name, size) {
  const card = document.createElement("div");
  card.className = "file-card";
  const icon = document.createElement("span");
  icon.textContent = "▤";
  const label = document.createElement("span");
  label.textContent = name;
  const meta = document.createElement("small");
  meta.textContent = bytesLabel(size);
  label.append(meta);
  card.append(icon, label);
  return card;
}
async function fileBlob(file) {
  if (
    !Number.isSafeInteger(file.size) ||
    file.size < 0 ||
    file.size > 8 * 1024 * 1024 ||
    !/^[a-f0-9]{64}$/.test(file.sha256)
  )
    throw new Error("Invalid file reference.");
  const parts = [];
  for (
    let index = 0;
    index < Math.max(1, Math.ceil(file.size / (192 * 1024)));
    index++
  ) {
    const chunk = await api(`attachments/${file.id}/chunks/${index}`);
    if (chunk.attachment.sha256 !== file.sha256 || chunk.index !== index)
      throw new Error("File identity changed.");
    parts.push(Uint8Array.from(atob(chunk.data), (c) => c.charCodeAt(0)));
  }
  const blob = new Blob(parts, { type: file.media_type });
  if (blob.size !== file.size) throw new Error("Incomplete file download.");
  const digest = Array.from(
    new Uint8Array(
      await crypto.subtle.digest("SHA-256", await blob.arrayBuffer()),
    ),
    (b) => b.toString(16).padStart(2, "0"),
  ).join("");
  if (digest !== file.sha256) throw new Error("File checksum mismatch.");
  return blob;
}
async function downloadFile(file) {
  const url = URL.createObjectURL(await fileBlob(file));
  const link = document.createElement("a");
  link.href = url;
  link.download = file.name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
function attachResult(parent, file) {
  const card = fileCard(file.name, file.size);
  const download = document.createElement("button");
  download.type = "button";
  download.textContent = "Download";
  download.addEventListener("click", async () => {
    download.disabled = true;
    try {
      await downloadFile(file);
    } catch (e) {
      notice(e.message);
    } finally {
      download.disabled = false;
    }
  });
  card.append(download);
  parent.append(card);
  if (
    ["image/png", "image/jpeg", "image/webp", "image/gif"].includes(
      file.media_type,
    )
  ) {
    const img = document.createElement("img");
    img.className = "artifact-preview";
    img.alt = file.name;
    img.loading = "lazy";
    parent.append(img);
    fileBlob(file)
      .then((blob) => {
        if (!img.isConnected) return;
        const url = URL.createObjectURL(blob);
        previewUrls.push(url);
        img.src = url;
      })
      .catch((e) => {
        img.remove();
        notice(e.message);
      });
  }
}
function addMessage(role, text, files = []) {
  const item = document.createElement("div");
  item.className = "message " + role;
  const label = document.createElement("strong");
  label.textContent = role === "user" ? "You" : "Cuttlefish";
  const content = document.createElement("div");
  content.className = "message-body";
  if (role === "user") content.textContent = text;
  else markdown(content, text, files);
  item.append(label, content);
  for (const file of files) attachResult(item, file);
  if (role !== "user") {
    const actions = document.createElement("div");
    actions.className = "message-actions";
    const copy = document.createElement("button");
    copy.type = "button";
    copy.textContent = "Copy response";
    copy.addEventListener("click", () =>
      navigator.clipboard
        .writeText(text)
        .then(() => {
          copy.textContent = "Copied";
        })
        .catch(() => notice("Copy is unavailable in this browser.")),
    );
    actions.append(copy);
    item.append(actions);
  }
  $("messages").append(item);
}
function pendingControls() {
  const pending = !!pendingChat;
  $("chat-message").readOnly = pending || sending;
  $("chat-message").required = !pending && !draftFiles.length;
  $("chat-workspace").disabled = pending || !!activeThread || sending;
  $("new-chat").disabled = pending || sending;
  $("attach-files").disabled = pending || sending;
  $("chat-send").textContent = pending ? "↻" : "↑";
  $("chat-send").setAttribute(
    "aria-label",
    pending ? "Retry same request" : "Send message",
  );
}
function renderChatList() {
  const signature = JSON.stringify([
    chatRows,
    activeThread,
    !!pendingChat,
    sending,
    $("chat-search").value,
  ]);
  if ($("chat-list").dataset.signature === signature) return;
  $("chat-list").dataset.signature = signature;
  $("chat-list").replaceChildren();
  for (const row of chatRows) {
    const title =
      row.title || row.request.action.message?.text || "Conversation";
    if (!title.toLowerCase().includes($("chat-search").value.toLowerCase()))
      continue;
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = title.slice(0, 100);
    button.title = title;
    button.setAttribute("aria-current", String(row.thread_id === activeThread));
    button.disabled = !!pendingChat || sending;
    button.addEventListener("click", () => {
      clearDrafts();
      activeThread = row.thread_id;
      history.replaceState(null, "", "#chat/" + activeThread);
      document.body.classList.remove("sidebar-open");
      refreshChat().catch((e) => notice(e.message));
    });
    $("chat-list").append(button);
  }
}
const providerLabel = (provider) =>
  provider === "claude"
    ? "Claude Code"
    : provider === "codex"
      ? "Codex"
      : "Your agent";
function activityLabel(phase, provider) {
  const name = providerLabel(provider);
  return (
    {
      preparing: "Preparing task…",
      connecting: `Connecting to ${name}…`,
      submitted: `Passed task to ${name}`,
      working: `${name} is working…`,
      reading_files: `${name} is reading files…`,
      writing_files: `${name} is updating files…`,
      running_command: `${name} is running a command…`,
      searching_web: `${name} is searching the web…`,
      generating_image: `${name} is creating an image…`,
      delegating: `${name} is coordinating sub-agents…`,
      using_tool: `${name} is using a tool…`,
      waiting: `Waiting for ${name}…`,
      collecting_results: "Saving results…",
      needs_attention: "This task needs attention",
    }[phase] || "Waiting for an update…"
  );
}
function activityState(turn, events, lastSeen) {
  const latest = events.at(-1);
  const done = turn.state === "completed";
  const failed = turn.outcome?.type === "rejected";
  const attention = !done && latest?.phase === "needs_attention";
  const offline = !done && lastSeen && Date.now() - lastSeen > 20000;
  const title = done
    ? failed
      ? "Task couldn’t be completed"
      : "Completed"
    : attention
      ? "This task needs attention"
      : offline
        ? "Agent connection interrupted"
        : latest
          ? activityLabel(latest.phase, latest.provider)
          : turn.state === "queued"
            ? "Waiting for an available agent…"
            : "Handed to your agent · waiting for an update…";
  const hint = attention
    ? "Execution stopped with an uncertain result. Check the agent before retrying; this task won’t be replayed automatically."
    : offline
      ? "The task may still be running. Updates will resume when the agent reconnects."
      : done
        ? ""
        : "You can leave this chat and return. Your progress is saved.";
  return {
    title,
    hint,
    active: !done && !attention && !offline,
    tone:
      failed || attention
        ? "attention"
        : offline
          ? "offline"
          : done
            ? "complete"
            : "active",
  };
}
function renderActivity(turns, activity) {
  for (const turn of turns) {
    const slot = document.getElementById("activity-" + turn.request.command_id);
    if (!slot) continue;
    const events = (activity.events || []).filter(
      (event) => event.command_id === turn.request.command_id,
    );
    const state = activityState(turn, events, activity.last_seen_ms);
    const signature = JSON.stringify([events, state]);
    if (slot.dataset.signature === signature) continue;
    slot.dataset.signature = signature;
    const wasOpen = slot.querySelector("details")?.open;
    slot.replaceChildren();
    slot.className = "task-activity " + state.tone;
    const details = document.createElement("details");
    details.open = wasOpen ?? false;
    const summary = document.createElement("summary");
    const dot = document.createElement("span");
    dot.className = state.active
      ? "activity-indicator spinning"
      : "activity-indicator";
    dot.setAttribute("aria-hidden", "true");
    dot.textContent =
      state.tone === "complete" ? "✓" : state.tone === "attention" ? "!" : "";
    const title = document.createElement("span");
    title.className = "activity-title";
    title.textContent = state.title;
    const meta = document.createElement("span");
    meta.className = "activity-meta";
    meta.textContent = events.length
      ? `${events.length} updates`
      : "Live activity";
    summary.append(dot, title, meta);
    details.append(summary);
    const list = document.createElement("ol");
    list.className = "activity-timeline";
    for (const event of events) {
      const item = document.createElement("li");
      const label = document.createElement("span");
      label.textContent = activityLabel(event.phase, event.provider).replace(
        /…$/,
        "",
      );
      const time = document.createElement("time");
      const date = new Date(event.at_ms);
      if (Number.isFinite(date.getTime())) time.dateTime = date.toISOString();
      time.textContent = date.toLocaleTimeString([], {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
      });
      item.append(label, time);
      list.append(item);
    }
    if (!events.length) {
      const item = document.createElement("li");
      item.textContent =
        state.tone === "complete"
          ? "Detailed activity is not available for this earlier task."
          : "Detailed activity will appear when the agent reports its next step.";
      list.append(item);
    }
    details.append(list);
    slot.append(details);
    if (state.hint) {
      const hint = document.createElement("p");
      hint.className = "activity-hint";
      hint.textContent = state.hint;
      slot.append(hint);
    }
  }
}

async function refreshChat() {
  if (chatBusy || sending) return;
  chatBusy = true;
  try {
    workspaceRows = await api("chat/workspaces");
    chatRows = await api("chat");
    const selected = $("chat-workspace").value;
    const workspaceSignature = JSON.stringify(workspaceRows);
    if ($("chat-workspace").dataset.signature !== workspaceSignature) {
      $("chat-workspace").dataset.signature = workspaceSignature;
      $("chat-workspace").replaceChildren();
      for (const row of workspaceRows) {
        const option = document.createElement("option");
        option.value = row.workspace.workspace_id;
        option.textContent = `${row.workspace.name} · ${row.workspace.provider}`;
        $("chat-workspace").append(option);
      }
    }
    if (workspaceRows.some((r) => r.workspace.workspace_id === selected))
      $("chat-workspace").value = selected;
    if (pendingChat) {
      activeThread = pendingChat.thread_id;
      $("chat-message").value = pendingChat.action.message?.text || "";
    }
    let turns = activeThread ? await api("chat/" + activeThread) : [];
    const activity =
      activeThread && activityEnabled
        ? await api("chat/" + activeThread + "/activity")
        : { events: [], last_seen_ms: null };
    let busy = false,
      ended = false;
    for (const turn of turns) {
      if (pendingChat?.command_id === turn.request.command_id) {
        savePending(null);
        $("chat-message").value = "";
      }
      busy = turn.state !== "completed";
      if (
        turn.outcome?.type === "completed" &&
        turn.outcome.event.kind.type === "cancelled"
      )
        ended = true;
    }
    const current = chatRows.find((r) => r.thread_id === activeThread);
    if (current?.workspace_id) $("chat-workspace").value = current.workspace_id;
    const signature = JSON.stringify([activeThread, turns]);
    if (signature !== rendered) {
      const scroll = $("conversation-scroll");
      const atBottom =
        scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 100;
      for (const url of previewUrls.splice(0)) URL.revokeObjectURL(url);
      $("messages").replaceChildren();
      $("chat-welcome").hidden = !!activeThread;
      for (const turn of turns) {
        const message = turn.request.action.message;
        if (message)
          addMessage("user", message.text, message.attachments || []);
        const slot = document.createElement("div");
        slot.id = "activity-" + turn.request.command_id;
        slot.setAttribute("aria-live", "off");
        slot.setAttribute("aria-label", "Task activity");
        $("messages").append(slot);
        if (turn.outcome?.type === "completed") {
          const event = turn.outcome.event;
          if (event.kind.type === "replied")
            addMessage("agent", event.kind.text, event.artifacts || []);
        } else if (turn.outcome?.type === "rejected")
          addMessage("agent", turn.outcome.reason);
      }
      rendered = signature;
      if (atBottom)
        requestAnimationFrame(() => (scroll.scrollTop = scroll.scrollHeight));
    }
    renderActivity(turns, activity);
    const latest = turns.at(-1);
    const latestState = latest
      ? activityState(
          latest,
          (activity.events || []).filter(
            (e) => e.command_id === latest.request.command_id,
          ),
          activity.last_seen_ms,
        )
      : null;
    const statusText = busy
      ? latestState.title
      : ended
        ? "Session ended. Start a new chat to continue."
        : workspaceRows.length
          ? ""
          : "Create a provider workspace in Settings to start chatting.";
    if ($("chat-status").textContent !== statusText)
      $("chat-status").textContent = statusText;
    $("chat-send").disabled = busy || ended || !workspaceRows.length;
    $("end-chat").hidden = !activeThread || busy || ended || !!pendingChat;
    pendingControls();
    renderChatList();
  } catch (error) {
    $("chat-status").textContent =
      error.status === 401
        ? "Sign in to continue."
        : "Connection interrupted · reconnecting automatically. Your task may still be running.";
    throw error;
  } finally {
    chatBusy = false;
  }
}
function clearDrafts() {
  for (const item of draftFiles) if (item.url) URL.revokeObjectURL(item.url);
  draftFiles = [];
  renderDrafts();
}
function renderDrafts() {
  $("draft-attachments").replaceChildren();
  for (const item of draftFiles) {
    const card = fileCard(item.file.name, item.file.size);
    if (item.url) {
      const image = document.createElement("img");
      image.src = item.url;
      image.alt = item.file.name;
      card.prepend(image);
    }
    const remove = document.createElement("button");
    remove.type = "button";
    remove.textContent = "×";
    remove.setAttribute("aria-label", "Remove " + item.file.name);
    remove.disabled = sending;
    remove.addEventListener("click", () => {
      draftFiles = draftFiles.filter((f) => f !== item);
      if (item.url) URL.revokeObjectURL(item.url);
      renderDrafts();
    });
    card.append(remove);
    $("draft-attachments").append(card);
  }
  pendingControls();
}
$("attach-files").addEventListener("click", () => $("file-picker").click());
function addFiles(files) {
  for (const file of files) {
    if (draftFiles.length >= 8) {
      notice("Attach up to 8 files per message.");
      break;
    }
    if (file.size > 8 * 1024 * 1024) {
      notice(file.name + " exceeds the 8 MiB file limit.");
      continue;
    }
    draftFiles.push({
      file,
      id: crypto.randomUUID(),
      url: ["image/png", "image/jpeg", "image/webp", "image/gif"].includes(
        file.type,
      )
        ? URL.createObjectURL(file)
        : null,
    });
  }
  renderDrafts();
}
$("file-picker").addEventListener("change", () => {
  addFiles($("file-picker").files);
  $("file-picker").value = "";
});
$("chat-form").addEventListener("dragover", (event) => {
  if (event.dataTransfer.types.includes("Files")) event.preventDefault();
});
$("chat-form").addEventListener("drop", (event) => {
  event.preventDefault();
  if (!sending && !pendingChat) addFiles(event.dataTransfer.files);
});
$("chat-message").addEventListener("paste", (event) => {
  if (event.clipboardData.files.length && !sending && !pendingChat) {
    event.preventDefault();
    addFiles(event.clipboardData.files);
  }
});
function encode64(bytes) {
  let result = "";
  for (let i = 0; i < bytes.length; i += 8192)
    result += String.fromCharCode(...bytes.subarray(i, i + 8192));
  return btoa(result);
}
async function uploadDrafts(workspace) {
  const files = [];
  for (const item of draftFiles) {
    const data = new Uint8Array(await item.file.arrayBuffer());
    const sha256 = Array.from(
      new Uint8Array(await crypto.subtle.digest("SHA-256", data)),
      (b) => b.toString(16).padStart(2, "0"),
    ).join("");
    const attachment = {
      id: item.id,
      workspace_id: workspace,
      name: item.file.name,
      media_type: item.file.type || "application/octet-stream",
      size: data.length,
      sha256,
    };
    const count = Math.max(1, Math.ceil(data.length / (192 * 1024)));
    for (let index = 0; index < count; index++) {
      $("chat-status").textContent =
        `Uploading ${item.file.name} · ${Math.round((index / count) * 100)}%`;
      const response = await api("attachments", {
        attachment,
        index,
        data: encode64(
          data.subarray(index * 192 * 1024, (index + 1) * 192 * 1024),
        ),
      });
      if (index === count - 1 && !response.ready)
        throw new Error("Upload did not finish. Retry to resume.");
    }
    files.push(attachment);
  }
  return files;
}
$("new-chat").addEventListener("click", () => {
  clearDrafts();
  activeThread = null;
  history.replaceState(null, "", location.pathname);
  $("chat-message").value = "";
  document.body.classList.remove("sidebar-open");
  refreshChat().catch((e) => notice(e.message));
});
$("chat-form").addEventListener("submit", (event) => {
  event.preventDefault();
  if (sending || $("chat-send").disabled) return;
  sending = true;
  $("chat-status").textContent = "Preparing your message…";
  pendingControls();
  $("chat-send").disabled = true;
  notice("");
  (async () => {
    try {
      if (!pendingChat) {
        const row = workspaceRows.find(
          (row) => row.workspace.workspace_id === $("chat-workspace").value,
        );
        if (!row) throw new Error("Select a workspace first.");
        const attachments = await uploadDrafts(row.workspace.workspace_id);
        const message = {
          id: crypto.randomUUID(),
          text:
            $("chat-message").value.trim() ||
            "Please examine the attached files.",
          attachments,
        };
        const action = activeThread
          ? { type: "send", message }
          : {
              type: "start",
              task_id: crypto.randomUUID(),
              session_id: crypto.randomUUID(),
              workspace_id: row.workspace.workspace_id,
              capabilities: [row.workspace.provider],
              message,
            };
        savePending({
          command_id: crypto.randomUUID(),
          thread_id: activeThread || crypto.randomUUID(),
          action,
        });
        clearDrafts();
      }
      $("chat-status").textContent = "Sending task to your agent…";
      const reply = await api("chat", pendingChat);
      activeThread = reply.request.thread_id;
      history.replaceState(null, "", "#chat/" + activeThread);
      savePending(null);
      $("chat-message").value = "";
      $("chat-message").style.height = "auto";
      rendered = "";
    } catch (error) {
      notice(error.message);
    } finally {
      sending = false;
      await refreshChat().catch((e) => notice(e.message));
    }
  })();
});
$("end-chat").addEventListener("click", () =>
  action($("chat-panel"), async () => {
    savePending({
      command_id: crypto.randomUUID(),
      thread_id: activeThread,
      action: { type: "cancel" },
    });
    await api("chat", pendingChat);
    savePending(null);
    await refreshChat();
  }),
);
$("toggle-sidebar").addEventListener("click", () =>
  document.body.classList.toggle("sidebar-open"),
);
$("chat-search").addEventListener("input", renderChatList);
$("chat-message").addEventListener("input", (event) => {
  event.target.style.height = "auto";
  event.target.style.height = Math.min(event.target.scrollHeight, 200) + "px";
});
$("chat-message").addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    if (!$("chat-send").disabled) $("chat-form").requestSubmit();
  }
});
for (const button of document.querySelectorAll("[data-starter]"))
  button.addEventListener("click", () => {
    $("chat-message").placeholder = button.dataset.starter;
    $("chat-message").focus();
  });

$("mobile-menu").addEventListener("click", () =>
  document.body.classList.toggle("sidebar-open"),
);

// Activity polling is lightweight metadata only; it never invokes the model.
setInterval(() => {
  if (signedIn && chatVisible && activeThread && !document.hidden && !sending)
    refreshChat().catch(() => {});
}, 2000);
