import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderToString } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ErrorBoundary } from "../src/components/error-boundary";
import { FormField } from "../src/components/form-field";
import { Input } from "../src/components/ui/input";
import { Button } from "../src/components/ui/button";
import {
  Dialog,
  DialogTrigger,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
  DialogClose,
} from "../src/components/ui/dialog";
import { Sidebar } from "../src/components/sidebar";
import { DraftCard, FileCard } from "../src/features/chat/file-card";
import { useObjectUrl } from "../src/hooks/use-object-url";
import { digest } from "../src/lib/files";
import { createQueryClient } from "../src/lib/query";
import { ApiError } from "../src/lib/api";
import { json } from "./fixtures";
describe("accessible primitives and resources", () => {
  it("associates fields and renders semantic polymorphic controls", () => {
    render(
      <FormField label="Name" description="Required">
        {(id) => <Input id={id} />}
      </FormField>,
    );
    expect(screen.getByLabelText("Name")).toBeVisible();
    render(
      <Button asChild>
        <a href="/">Home</a>
      </Button>,
    );
    expect(screen.getByRole("link", { name: "Home" })).toHaveAttribute(
      "href",
      "/",
    );
  });
  it("opens and dismisses a focus-managed dialog with keyboard", async () => {
    const user = userEvent.setup();
    render(
      <Dialog>
        <DialogTrigger asChild>
          <Button>Open</Button>
        </DialogTrigger>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Example</DialogTitle>
            <DialogDescription>Description</DialogDescription>
          </DialogHeader>
          <DialogFooter showCloseButton>
            <DialogClose asChild>
              <Button>Done</Button>
            </DialogClose>
          </DialogFooter>
        </DialogContent>
      </Dialog>,
    );
    await user.click(screen.getByRole("button", { name: "Open" }));
    expect(screen.getByRole("dialog")).toHaveAccessibleName("Example");
    await user.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Open" })).toHaveFocus();
  });
  it("uses a real dialog for mobile navigation", async () => {
    vi.mocked(window.matchMedia).mockReturnValue({
      matches: true,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    } as unknown as MediaQueryList);
    const user = userEvent.setup();
    render(
      <Sidebar>
        <Button data-navigate>Chats</Button>
        <span>Label</span>
      </Sidebar>,
    );
    await user.click(screen.getByRole("button", { name: "Open navigation" }));
    expect(screen.getByRole("dialog")).toBeVisible();
    await user.click(screen.getByText("Label"));
    expect(screen.getByRole("dialog")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Chats" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Open navigation" }),
    ).toHaveFocus();
  });
  it("renders safely on a server without browser URL allocation", () => {
    function Preview() {
      const url = useObjectUrl(undefined);
      return <img src={url} alt="Preview" />;
    }
    expect(renderToString(<Preview />)).toContain("Preview");
    expect(renderToString(<Sidebar>Navigation</Sidebar>)).toContain(
      "Navigation",
    );
  });
  it("contains rendering failures and keeps a recovery action available", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    function Broken(): never {
      throw new Error("broken");
    }
    render(
      <ErrorBoundary>
        <Broken />
      </ErrorBoundary>,
    );
    expect(
      screen.getByRole("heading", { name: "Unable to display this page" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Reload" })).toBeVisible();
  });
  it("verifies image previews and releases draft URLs", async () => {
    const file = new File(["image"], "preview.png", { type: "image/png" });
    const unmount = render(
      <DraftCard
        draft={{ id: "draft", file }}
        disabled={false}
        onRemove={vi.fn()}
      />,
    ).unmount;
    expect(screen.getByAltText("")).toHaveAttribute("src", "blob:preview");
    unmount();
    expect(URL.revokeObjectURL).toHaveBeenCalled();
    const attachment = {
      id: "file",
      workspace_id: "workspace",
      name: "preview.png",
      media_type: "image/png",
      size: 5,
      sha256: await digest(await file.arrayBuffer()),
    };
    vi.spyOn(globalThis, "fetch").mockImplementation(async () =>
      json({ attachment, index: 0, data: btoa("image") }),
    );
    const cache = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={cache}>
        <FileCard file={attachment} />
      </QueryClientProvider>,
    );
    expect(await screen.findByAltText("preview.png")).toBeVisible();
    const user = userEvent.setup();
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    await user.click(
      screen.getByRole("button", { name: "Download preview.png" }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Download preview.png" }),
      ).toBeEnabled(),
    );
  });
  it("never retries unauthorized requests, but retries transient reads", async () => {
    const cache = createQueryClient(),
      unauthorized = vi
        .fn()
        .mockRejectedValue(new ApiError("unauthorized", 401));
    await expect(
      cache.fetchQuery({
        queryKey: ["auth"],
        queryFn: unauthorized,
        retryDelay: 0,
      }),
    ).rejects.toThrow("unauthorized");
    expect(unauthorized).toHaveBeenCalledOnce();
    const transient = vi
      .fn()
      .mockRejectedValueOnce(new Error("network"))
      .mockRejectedValueOnce(new Error("network"))
      .mockResolvedValue("ok");
    expect(
      await cache.fetchQuery({
        queryKey: ["read"],
        queryFn: transient,
        retryDelay: 0,
      }),
    ).toBe("ok");
    expect(transient).toHaveBeenCalledTimes(3);
  });
});
