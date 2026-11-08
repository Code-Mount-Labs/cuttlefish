import { QueryClient } from "@tanstack/react-query";
import { ApiError } from "./api";
export function createQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: {
        refetchOnWindowFocus: true,
        retry: (count, error) =>
          !(error instanceof ApiError && error.status === 401) && count < 2,
      },
      mutations: { retry: false },
    },
  });
}

// A signed-out session is data, not a failed request. Keeping it in the cache
// prevents refetches from replacing an in-progress form with a loading screen.
export async function readSession<T>(
  read: () => Promise<T>,
): Promise<T | null> {
  try {
    return await read();
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) return null;
    throw error;
  }
}
