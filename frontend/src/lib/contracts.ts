import { z } from "./schema";
export const providerSchema = z.enum(["codex", "claude"]);
export const workspaceSchema = z.object({
  workspace_id: z.string(),
  name: z.string(),
  provider: providerSchema,
  allow_edits: z.boolean(),
});
export const workspaceRowSchema = z.object({ workspace: workspaceSchema });
export const attachmentSchema = z.object({
  id: z.string(),
  workspace_id: z.string(),
  name: z.string(),
  media_type: z.string(),
  size: z
    .number()
    .int()
    .min(0)
    .max(8 * 1024 * 1024),
  sha256: z.string().regex(/^[a-f0-9]{64}$/),
});
export const messageSchema = z.object({
  id: z.string(),
  text: z.string(),
  attachments: z.array(attachmentSchema).default([]),
});
export const actionSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("start"),
    task_id: z.string(),
    session_id: z.string(),
    workspace_id: z.string(),
    capabilities: z.array(z.string()),
    message: messageSchema,
  }),
  z.object({ type: z.literal("send"), message: messageSchema }),
  z.object({ type: z.literal("cancel") }),
  z.object({
    type: z.literal("restart"),
    session_id: z.string(),
    message: messageSchema,
  }),
]);
export const requestSchema = z.object({
  command_id: z.string(),
  thread_id: z.string(),
  action: actionSchema,
});
const outcomeSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("rejected"), reason: z.string() }),
  z.object({
    type: z.literal("completed"),
    event: z.object({
      kind: z.object({ type: z.string(), text: z.string().optional() }),
      artifacts: z.array(attachmentSchema).default([]),
    }),
  }),
]);
export const turnSchema = z.object({
  request: requestSchema,
  state: z.string(),
  outcome: outcomeSchema.nullish(),
});
export const conversationSchema = z.object({
  thread_id: z.string(),
  workspace_id: z.string().nullish(),
  title: z.string().nullish(),
  request: requestSchema,
});
export const phaseSchema = z.enum([
  "preparing",
  "connecting",
  "submitted",
  "working",
  "reading_files",
  "writing_files",
  "running_command",
  "searching_web",
  "generating_image",
  "delegating",
  "using_tool",
  "waiting",
  "collecting_results",
  "needs_attention",
]);
export const activityEventSchema = z.object({
  sequence: z.number(),
  command_id: z.string(),
  provider: providerSchema,
  phase: phaseSchema,
  at_ms: z.number(),
});
export const activitySchema = z.object({
  events: z.array(activityEventSchema),
  last_seen_ms: z.number().nullable(),
});
export const remoteSchema = z.object({
  revision: z.number(),
  choice: z.object({
    mode: z.enum(["local", "managed", "existing"]),
    origin: z.string().optional(),
  }),
  state: z.string(),
  public_url: z.string().nullish(),
  error_code: z.string().nullish(),
});
export const configSchema = z.object({
  name: z.string(),
  group_id: z.string(),
  role: z.enum(["router", "agent", "router_agent"]),
  features: z
    .object({
      chat: z.boolean().optional(),
      attachments: z.boolean().optional(),
      activity: z.boolean().optional(),
    })
    .optional(),
  database: z.object({
    state: z.string(),
    mode: z.string().nullish(),
    error_code: z.string().nullish(),
  }),
  remote: remoteSchema,
  model: z.object({ configured: z.boolean(), model: z.string().nullish() }),
});
export type Provider = z.infer<typeof providerSchema>;
export type Attachment = z.infer<typeof attachmentSchema>;
export type Workspace = z.infer<typeof workspaceSchema>;
export type Request = z.infer<typeof requestSchema>;
export type Turn = z.infer<typeof turnSchema>;
export type ActivityEvent = z.infer<typeof activityEventSchema>;
export type Phase = z.infer<typeof phaseSchema>;
export type Config = z.infer<typeof configSchema>;
