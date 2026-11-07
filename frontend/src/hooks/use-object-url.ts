import { useMemo, useSyncExternalStore } from "react";
/** Browser URL ownership follows subscriptions, including Strict Mode remounts. */
export function createObjectUrlStore(blob: Blob | undefined) {
  let url: string | undefined;
  const listeners = new Set<() => void>();
  return {
    snapshot: () => url,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      if (blob && url === undefined) {
        url = URL.createObjectURL(blob);
        listeners.forEach((notify) => notify());
      }
      return () => {
        listeners.delete(listener);
        if (!listeners.size && url !== undefined) {
          URL.revokeObjectURL(url);
          url = undefined;
        }
      };
    },
  };
}
export function useObjectUrl(blob: Blob | undefined) {
  const store = useMemo(() => createObjectUrlStore(blob), [blob]);
  return useSyncExternalStore(store.subscribe, store.snapshot, () => undefined);
}
