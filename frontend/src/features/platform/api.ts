import { z } from "../../lib/schema";
import { request } from "../../lib/api";
export const groupSchema = z.object({
  group_id: z.string(),
  name: z.string(),
  agents: z.number(),
  online: z.number(),
});
export const machineSchema = z.object({
  installation_id: z.string(),
  name: z.string(),
  role: z.enum(["router", "agent", "router_agent"]),
  online: z.boolean(),
  router_url: z.string().nullish(),
  router_dashboard_url: z.string().nullish(),
  agent_dashboard_url: z.string().nullish(),
});
export type Group = z.infer<typeof groupSchema>;
export async function allPages<T>(
  path: string,
  schema: z.ZodType<T>,
  id: (row: T) => string,
  signal?: AbortSignal,
) {
  const rows: T[] = [];
  let after = "00000000-0000-0000-0000-000000000000";
  for (;;) {
    const page = await request(
      `${path}?after=${encodeURIComponent(after)}`,
      z.array(schema),
      { signal },
    );
    rows.push(...page);
    if (page.length < 100) return rows;
    const next = id(page[page.length - 1]);
    if (next <= after) throw new Error("Invalid page. Please refresh.");
    after = next;
  }
}
export function installCommand(origin: string) {
  return (
    "curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/Code-Mount-Labs/cuttlefish/v0.2.0-alpha.5/install.sh | sh -s -- --platform '" +
    origin.replaceAll("'", "'\"'\"'") +
    "'"
  );
}
