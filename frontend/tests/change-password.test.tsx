import { it, expect, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChangePasswordForm } from "../src/features/auth/change-password-form";
import { LocalApp } from "../src/app/local-app";
import { PlatformApp } from "../src/app/platform-app";
import { config, json } from "./fixtures";
function mount(child: React.ReactNode) {
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return {
    cache,
    ...render(
      <QueryClientProvider client={cache}>{child}</QueryClientProvider>,
    ),
  };
}
async function fill(password = "replacement-password") {
  const user = userEvent.setup();
  await user.type(
    screen.getByLabelText("Current password"),
    "original-password",
  );
  await user.type(screen.getByLabelText("New password"), password);
  await user.type(screen.getByLabelText("Confirm new password"), password);
  return user;
}
it.each(["local", "platform"] as const)(
  "changes the %s password with duplicate submissions blocked",
  async (scope) => {
    let finish!: (response: Response) => void;
    const fetch = vi.spyOn(globalThis, "fetch").mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const onChanged = vi.fn();
    mount(<ChangePasswordForm scope={scope} onChanged={onChanged} />);
    const user = await fill();
    await user.click(screen.getByRole("button", { name: "Change password" }));
    expect(
      screen.getByRole("button", { name: "Changing password…" }),
    ).toBeDisabled();
    expect(screen.getByLabelText("Current password")).toBeDisabled();
    fireEvent.submit(screen.getByRole("form", { name: "Change password" }));
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(fetch.mock.calls[0][0]).toBe(
      scope === "local" ? "/api/local/password" : "/api/v1/auth/password",
    );
    expect(JSON.parse(String(fetch.mock.calls[0][1]?.body))).toEqual({
      current_password: "original-password",
      new_password: "replacement-password",
    });
    finish(json({ ok: true }));
    await waitFor(() => expect(onChanged).toHaveBeenCalledOnce());
    expect(screen.getByLabelText("Current password")).toHaveValue("");
    expect(screen.getByLabelText("New password")).toHaveAttribute(
      "autocomplete",
      "new-password",
    );
  },
);
it("validates matching passwords and byte limits, and clears validation when edited", async () => {
  const fetch = vi.spyOn(globalThis, "fetch");
  mount(<ChangePasswordForm scope="local" onChanged={vi.fn()} />);
  const user = await fill("short");
  await user.type(screen.getByLabelText("Confirm new password"), "different");
  await user.click(screen.getByRole("button", { name: "Change password" }));
  expect(screen.getByRole("alert")).toHaveTextContent("do not match");
  await user.clear(screen.getByLabelText("Confirm new password"));
  await user.type(screen.getByLabelText("Confirm new password"), "short");
  expect(screen.queryByRole("alert")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Change password" }));
  expect(screen.getByRole("alert")).toHaveTextContent("12 and 1024 bytes");
  for (const label of ["New password", "Confirm new password"])
    fireEvent.change(screen.getByLabelText(label), {
      target: { value: "é".repeat(600) },
    });
  fireEvent.submit(screen.getByRole("form", { name: "Change password" }));
  expect(screen.getByRole("alert")).toHaveTextContent("12 and 1024 bytes");
  expect(fetch).not.toHaveBeenCalled();
});
it("keeps fields for correction after failure and clears stale server errors", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    json({ error: "Current password is incorrect" }, 401),
  );
  const done = vi.fn();
  mount(<ChangePasswordForm scope="local" onChanged={done} />);
  const user = await fill();
  await user.click(screen.getByRole("button", { name: "Change password" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Current password is incorrect",
  );
  expect(done).not.toHaveBeenCalled();
  await user.type(screen.getByLabelText("Current password"), "x");
  expect(screen.queryByRole("alert")).toBeNull();
});
it("handles a successful response after its form unmounts", async () => {
  let finish!: (response: Response) => void;
  vi.spyOn(globalThis, "fetch").mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const done = vi.fn();
  const view = mount(<ChangePasswordForm scope="local" onChanged={done} />);
  const user = await fill();
  await user.click(screen.getByRole("button", { name: "Change password" }));
  view.unmount();
  finish(json({ ok: true }));
  await waitFor(() => expect(done).toHaveBeenCalledOnce());
});
it.each([true, false])(
  "password change clears %s app auth state without calling logout",
  async (localApp) => {
    let signedIn = localApp;
    const fetch = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(async (input) => {
        const path = String(input);
        if (path.endsWith("/register")) {
          signedIn = true;
          return json({ ok: true });
        }
        if (path.endsWith("/password")) {
          signedIn = false;
          return json({ ok: true });
        }
        if (path.endsWith("/status")) return json({ initialized: true });
        if (path.endsWith("/config"))
          return signedIn
            ? json({ ...config, role: "router", features: { chat: false } })
            : json({ error: "unauthorized" }, 401);
        if (path.endsWith("/session"))
          return signedIn ? json({}) : json({ error: "unauthorized" }, 401);
        if (path.endsWith("/logout"))
          throw Error("Revoked session must not be logged out again");
        return json([]);
      });
    const view = mount(localApp ? <LocalApp /> : <PlatformApp />);
    if (localApp) await screen.findByRole("heading", { name: "Settings" });
    else {
      const signup = userEvent.setup();
      await signup.click(
        await screen.findByRole("button", { name: "Create account" }),
      );
      await signup.type(screen.getByLabelText("Username"), "new-user");
      await signup.type(screen.getByLabelText("Password"), "a-secure-password");
      await signup.click(
        screen.getByRole("button", { name: "Create account" }),
      );
      await userEvent.click(
        await screen.findByRole("button", { name: "Account" }),
      );
    }
    view.cache.setQueryData(["private-content"], "must clear");
    const user = await fill();
    await user.click(screen.getByRole("button", { name: "Change password" }));
    expect(
      await screen.findByRole("heading", { name: "Sign in" }),
    ).toBeVisible();
    expect(
      screen.getByText("Password changed. Sign in with your new password."),
    ).toBeVisible();
    expect(view.cache.getQueryData(["private-content"])).toBeUndefined();
    expect(
      fetch.mock.calls.some(([path]) => String(path).endsWith("/logout")),
    ).toBe(false);
  },
);
