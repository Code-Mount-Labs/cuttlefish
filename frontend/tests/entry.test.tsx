import { afterEach, it, expect, vi } from "vitest";
import { renderToString } from "react-dom/server";
import { usePendingRequest } from "../src/hooks/use-pending-request";
const { render, createRoot } = vi.hoisted(() => {
  const render = vi.fn();
  return { render, createRoot: vi.fn(() => ({ render })) };
});
vi.mock("react-dom/client", () => ({ createRoot }));
afterEach(() => {
  vi.unstubAllGlobals();
});
it.each(["platform", "local"])("mounts the %s entry point", async (mode) => {
  vi.resetModules();
  document.body.innerHTML = '<div id="root"></div>';
  vi.stubGlobal("__APP_KIND__", mode);
  await import("../src/main");
  expect(createRoot).toHaveBeenCalledWith(document.getElementById("root"));
  expect(render).toHaveBeenCalled();
});
it("rejects a missing application root", async () => {
  vi.resetModules();
  document.body.innerHTML = "";
  await expect(import("../src/main")).rejects.toThrow(
    "Application root is missing",
  );
});
it("renders an empty pending request on the server", () => {
  function Pending() {
    return <span>{usePendingRequest() === null ? "Empty" : "Pending"}</span>;
  }
  expect(renderToString(<Pending />)).toContain("Empty");
});
