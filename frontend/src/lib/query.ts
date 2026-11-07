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
