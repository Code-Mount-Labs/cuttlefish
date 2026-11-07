import { it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Groups } from "../src/features/platform/groups";
import { json } from "./fixtures";
function mount() {
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={cache}>
      <Groups />
    </QueryClientProvider>,
  );
  return cache;
}
const group = { group_id: "group", name: "Team", agents: 2, online: 1 };
it("manages machine invitations and clears the code when the dialog closes", async () => {
  const user = userEvent.setup(),
    clipboard = vi
      .spyOn(navigator.clipboard, "writeText")
      .mockResolvedValue(undefined);
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    const path = String(input);
    if (path.includes("/invitations"))
      return json({ invitation: "one-use-secret" });
    if (path.includes("/installations"))
      return json([
        {
          installation_id: "mac",
          name: "Mac",
          role: "router_agent",
          online: true,
          router_dashboard_url: "https://example.org",
          agent_dashboard_url: "javascript:alert(1)",
        },
        {
          installation_id: "linux",
          name: "Linux",
          role: "agent",
          online: false,
        },
      ]);
    return json([group]);
  });
  mount();
  await user.click(await screen.findByRole("button", { name: /^Team/ }));
  expect(await screen.findByText("Mac")).toBeVisible();
  expect(
    screen.getByRole("link", { name: "Router dashboard" }),
  ).toHaveAttribute("href", "https://example.org/");
  expect(screen.queryByRole("link", { name: "Agent dashboard" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "Copy group ID" }));
  expect(clipboard).toHaveBeenCalledWith("group");
  await user.click(screen.getByRole("button", { name: "Connect machine" }));
  expect(await screen.findByRole("dialog")).toBeVisible();
  expect(screen.getByLabelText("Enrollment code")).toHaveValue(
    "one-use-secret",
  );
  await user.click(
    screen.getByRole("button", { name: "Copy enrollment code" }),
  );
  expect(clipboard).toHaveBeenCalledWith("one-use-secret");
  clipboard.mockRejectedValue(new Error("denied"));
  await user.click(
    screen.getByRole("button", { name: "Copy install command" }),
  );
  expect(
    await screen.findByText("Select and copy the value manually."),
  ).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Close" }));
  expect(screen.queryByDisplayValue("one-use-secret")).toBeNull();
  expect(screen.getByRole("button", { name: "Connect machine" })).toHaveFocus();
  await user.click(screen.getByRole("button", { name: "Refresh groups" }));
});
it("shows group, creation, installation and enrollment failures", async () => {
  const user = userEvent.setup();
  let stage = "groups";
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const path = String(input);
    if (path.includes("/invitations"))
      return json({ error: "Enrollment unavailable" }, 503);
    if (path.includes("/installations"))
      return json({ error: "Machines unavailable" }, 503);
    if (init?.method === "POST")
      return json({ error: "Group creation failed" }, 503);
    return stage === "groups"
      ? json({ error: "Groups unavailable" }, 503)
      : json([group]);
  });
  mount();
  expect(await screen.findByText("Groups unavailable")).toBeVisible();
  stage = "ready";
  await user.click(screen.getByRole("button", { name: "Refresh groups" }));
  await user.click(await screen.findByRole("button", { name: /^Team/ }));
  expect(await screen.findByText("Machines unavailable")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Connect machine" }));
  expect(await screen.findByText("Enrollment unavailable")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Create group" }));
  await user.type(screen.getByLabelText("Group name"), "New");
  fireEvent.submit(screen.getByLabelText("Group name").closest("form")!);
  expect(await screen.findByText("Group creation failed")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Close" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});
it("paginates large group and machine catalogs", async () => {
  const groupRows = Array.from({ length: 100 }, (_, i) => ({
    ...group,
    group_id: String(i + 1).padStart(36, "0"),
    name: `Team ${i + 1}`,
  }));
  const machines = Array.from({ length: 100 }, (_, i) => ({
    installation_id: String(i + 1).padStart(36, "0"),
    name: `Machine ${i + 1}`,
    role: "router",
    online: true,
  }));
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    const path = String(input);
    return json(
      path.endsWith("00000000-0000-0000-0000-000000000000")
        ? path.includes("/installations")
          ? machines
          : groupRows
        : [],
    );
  });
  mount();
  await screen.findByRole("button", { name: /Team 100/ });
  await userEvent.click(screen.getByRole("button", { name: /^Team 100/ }));
  expect(await screen.findByText("Machine 100")).toBeVisible();
});
