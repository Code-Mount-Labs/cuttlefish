import { it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Message } from "../src/features/chat/message";
import { FileCard } from "../src/features/chat/file-card";
import { Composer } from "../src/features/chat/composer";
import { TaskActivity } from "../src/features/chat/activity";
import { ErrorBoundary } from "../src/components/error-boundary";
import { json, turn } from "./fixtures";
const file = {
  id: "file",
  workspace_id: "workspace",
  name: "report.txt",
  size: 4,
  media_type: "text/plain",
  sha256: "a".repeat(64),
};
it("reports copy and artifact download failures without trusting external images", async () => {
  const user = userEvent.setup();
  vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(
    new Error("No permission"),
  );
  vi.spyOn(globalThis, "fetch").mockImplementation(async () =>
    json({ error: "File unavailable" }, 503),
  );
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Message
        speaker="agent"
        files={[file]}
        text={
          "[Report](artifacts/report.txt)\n\n[Absolute](/results/report.txt)\n\n[Other](/not-matched)\n\n![External image](https://example.org/track.png)"
        }
      />
    </QueryClientProvider>,
  );
  expect(screen.queryByRole("img")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Copy response" }));
  expect(
    await screen.findByText("Copy is unavailable in this browser."),
  ).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Report" }));
  expect(await screen.findByText("File unavailable")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Download report.txt" }));
  await waitFor(() =>
    expect(screen.getAllByText("File unavailable")).toHaveLength(2),
  );
});
it("rejects a corrupt image preview visibly", async () => {
  vi.spyOn(globalThis, "fetch").mockImplementation(async () =>
    json({ error: "Corrupt image" }, 503),
  );
  render(
    <QueryClientProvider client={new QueryClient()}>
      <FileCard file={{ ...file, media_type: "image/png" }} />
    </QueryClientProvider>,
  );
  expect(await screen.findByText("Corrupt image")).toBeVisible();
  expect(screen.queryByRole("img")).toBeNull();
});
it("composer handles browser file and keyboard events according to locked state", async () => {
  const files = vi.fn(),
    send = vi.fn(),
    text = vi.fn(),
    remove = vi.fn(),
    props = {
      text: "hi",
      onText: text,
      files: [],
      onFiles: files,
      onRemove: remove,
      onSend: send,
      locked: false,
      disabled: false,
      pending: false,
      status: "Working",
      attachments: true,
    };
  const { rerender } = render(<Composer {...props} />);
  const form = document.querySelector("form")!,
    box = screen.getByRole("textbox");
  fireEvent.dragOver(form, { dataTransfer: { types: ["Files"] } });
  fireEvent.dragOver(form, { dataTransfer: { types: ["text/plain"] } });
  fireEvent.drop(form, {
    dataTransfer: { files: [new File(["x"], "dropped.txt")] },
  });
  expect(files).toHaveBeenCalledTimes(1);
  const picker = document.getElementById("file-picker")!;
  const click = vi.spyOn(picker, "click");
  fireEvent.click(screen.getByRole("button", { name: "Attach files" }));
  expect(click).toHaveBeenCalled();
  fireEvent.change(picker, {
    target: { files: [new File(["x"], "chosen.txt")] },
  });
  expect(files).toHaveBeenCalledTimes(2);
  fireEvent.change(picker, { target: { files: null } });
  fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
  fireEvent.keyDown(box, { key: "Enter", isComposing: true });
  fireEvent.keyDown(box, { key: "a" });
  expect(send).not.toHaveBeenCalled();
  rerender(<Composer {...props} locked disabled attachments={false} />);
  fireEvent.drop(form, { dataTransfer: { files: [] } });
  fireEvent.paste(box, {
    clipboardData: { files: [new File(["x"], "no.txt")] },
  });
  fireEvent.submit(form);
  expect(send).not.toHaveBeenCalled();
});
it("activity distinguishes terminal and uncertain states", () => {
  const { rerender } = render(
    <TaskActivity
      turn={{ ...turn, state: "completed" }}
      events={[]}
      lastSeen={null}
    />,
  );
  expect(screen.getByText("Completed")).toBeVisible();
  rerender(
    <TaskActivity
      turn={turn}
      events={[
        {
          command_id: "c",
          sequence: 1,
          phase: "needs_attention",
          provider: "codex",
          at_ms: 1,
        },
      ]}
      lastSeen={null}
    />,
  );
  expect(screen.getAllByText("This task needs attention")[0]).toBeVisible();
  rerender(
    <TaskActivity
      turn={turn}
      events={[
        {
          command_id: "c",
          sequence: 1,
          phase: "submitted",
          provider: "codex",
          at_ms: 1,
        },
        {
          command_id: "c",
          sequence: 2,
          phase: "working",
          provider: "codex",
          at_ms: 2,
        },
      ]}
      lastSeen={Date.now()}
    />,
  );
  expect(screen.getByText("2 updates")).toBeVisible();
});
it("the render error recovery action reloads the page", () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  function Broken(): never {
    throw new Error("bad");
  }
  render(
    <ErrorBoundary>
      <Broken />
    </ErrorBoundary>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Reload" }));
});
