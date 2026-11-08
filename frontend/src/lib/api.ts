import { z } from "./schema";
export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message);
    this.name = "ApiError";
  }
}
export function relayBase(path = location.pathname) {
  return path.match(/^\/i\/[a-f0-9-]{36}(?=\/|$)/)?.[0] ?? "";
}
export async function request<T>(
  path: string,
  schema: z.ZodType<T>,
  options: { body?: unknown; signal?: AbortSignal } = {},
): Promise<T> {
  const response = await fetch(path, {
    method: options.body === undefined ? "GET" : "POST",
    credentials: "same-origin",
    signal: options.signal,
    headers:
      options.body === undefined ? {} : { "Content-Type": "application/json" },
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });
  let value: unknown;
  try {
    value = await response.json();
  } catch {
    throw new ApiError(
      "The server returned an unexpected response. Please try again.",
      response.status,
    );
  }
  if (!response.ok) {
    const error = z.object({ error: z.string() }).safeParse(value);
    throw new ApiError(
      error.success ? error.data.error : `Request failed (${response.status}).`,
      response.status,
    );
  }
  const parsed = schema.safeParse(value);
  if (!parsed.success)
    throw new ApiError(
      "The server returned data this version cannot read. Please refresh or update Cuttlefish.",
      502,
    );
  return parsed.data;
}
export function local<T>(
  path: string,
  schema: z.ZodType<T>,
  options: { body?: unknown; signal?: AbortSignal } = {},
) {
  return request(relayBase() + "/api/local/" + path, schema, options);
}
export function errorMessage(error: unknown) {
  return error instanceof Error
    ? error.message
    : "Something went wrong. Please try again.";
}
export function safeWebUrl(
  value: string | undefined | null,
  httpsOnly = false,
) {
  try {
    const u = new URL(value ?? "");
    return (
      httpsOnly
        ? u.protocol === "https:"
        : ["http:", "https:"].includes(u.protocol)
    )
      ? u.href
      : undefined;
  } catch {
    return undefined;
  }
}
