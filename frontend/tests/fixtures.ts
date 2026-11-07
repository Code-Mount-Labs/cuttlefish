import type { Config, Request, Turn } from "../src/lib/contracts";
export const workspace = {
  workspace_id: "00000000-0000-0000-0000-000000000005",
  name: "Research",
  provider: "codex" as const,
  allow_edits: true,
};
export const request: Request = {
  command_id: "00000000-0000-0000-0000-000000000001",
  thread_id: "00000000-0000-0000-0000-000000000002",
  action: {
    type: "start",
    task_id: "task",
    session_id: "session",
    workspace_id: workspace.workspace_id,
    capabilities: ["codex"],
    message: { id: "message", text: "Read the brief", attachments: [] },
  },
};
export const turn: Turn = { request, state: "claimed", outcome: null };
export const config: Config = {
  name: "Mac",
  group_id: "group",
  role: "router_agent",
  features: { chat: true, activity: true, attachments: true },
  database: { state: "ready" },
  remote: { revision: 1, choice: { mode: "local" }, state: "local" },
  model: { configured: false },
};
export function json(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}
