import { z } from "../../lib/schema";
export const updateSchema = z.object({
  current_version: z.string(),
  available_version: z.string().nullable(),
  checked: z.boolean(),
  check_error: z.string().nullable(),
  supported: z.boolean(),
  automatic_restart: z.boolean(),
  job: z
    .object({
      id: z.string(),
      version: z.string(),
      restart_only: z.boolean(),
      phase: z.enum([
        "queued",
        "downloading",
        "restarting",
        "complete",
        "restart_required",
        "failed",
      ]),
      error: z.string().nullable(),
    })
    .nullable(),
});
