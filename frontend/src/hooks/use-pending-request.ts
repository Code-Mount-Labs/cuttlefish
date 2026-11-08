import { useMemo, useSyncExternalStore } from "react";
import {
  parsePending,
  pendingSnapshot,
  subscribePending,
} from "../lib/pending";
export function usePendingRequest() {
  const raw = useSyncExternalStore(
    subscribePending,
    pendingSnapshot,
    () => null,
  );
  return useMemo(() => parsePending(raw), [raw]);
}
