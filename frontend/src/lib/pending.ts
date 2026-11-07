import { requestSchema, type Request } from "./contracts";
import { relayBase } from "./api";
const key = () => `cuttlefish:pending:${relayBase()}`;
const listeners = new Set<() => void>();
export const pendingSnapshot = () => sessionStorage.getItem(key());
export function parsePending(raw: string | null): Request | null {
  if (!raw) return null;
  try {
    const parsed = requestSchema.safeParse(JSON.parse(raw));
    return parsed.success ? parsed.data : null;
  } catch {
    return null;
  }
}
export function readPending() {
  return parsePending(pendingSnapshot());
}
export function subscribePending(listener: () => void) {
  listeners.add(listener);
  window.addEventListener("storage", listener);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", listener);
  };
}
export function savePending(value: Request | null) {
  if (value) sessionStorage.setItem(key(), JSON.stringify(value));
  else sessionStorage.removeItem(key());
  listeners.forEach((notify) => notify());
}
