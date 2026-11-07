import { it, expect, vi } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AuthForm } from "../src/features/auth/auth-form";
import { SettingsPage } from "../src/features/settings/settings-page";
import { config, json } from "./fixtures";
it("shows and disables pending authentication", async () => {
  let resolve!: () => void;
  const pending = new Promise<void>((r) => (resolve = r));
  render(
    <QueryClientProvider client={new QueryClient()}>
      <AuthForm onSubmit={() => pending} />
    </QueryClientProvider>,
  );
  fireEvent.submit(document.getElementById("auth-form")!);
  expect(
    await screen.findByRole("button", { name: "Signing in…" }),
  ).toBeDisabled();
  await act(async () => resolve());
});
it("shows pending configuration until the server responds", async () => {
  let resolve!: (response: Response) => void;
  vi.spyOn(globalThis, "fetch").mockImplementation(
    () => new Promise((r) => (resolve = r)),
  );
  render(
    <QueryClientProvider client={new QueryClient()}>
      <SettingsPage config={{ ...config, role: "router" }} onLogout={vi.fn()} />
    </QueryClientProvider>,
  );
  fireEvent.submit(screen.getByLabelText("Access").closest("form")!);
  expect(await screen.findByText("Saving…")).toBeVisible();
  expect(screen.getByRole("button", { name: "Save access" })).toBeDisabled();
  await act(async () => resolve(json({})));
});
