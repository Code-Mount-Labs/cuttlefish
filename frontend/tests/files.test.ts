import { describe, it, expect, vi } from "vitest";
import {
  bytesLabel,
  digest,
  encode64,
  fileBlob,
  uploadFiles,
  MAX_FILE_SIZE,
  CHUNK_SIZE,
  downloadFile,
  isPreviewable,
} from "../src/lib/files";
import { json, workspace } from "./fixtures";
describe("file transport", () => {
  it("formats file sizes and restricts image previews", () => {
    expect(bytesLabel(50)).toBe("50 B");
    expect(bytesLabel(2048)).toBe("2.0 KB");
    expect(bytesLabel(2 * 1024 * 1024)).toBe("2.0 MB");
    expect(isPreviewable("image/svg+xml")).toBe(false);
    expect(isPreviewable("image/png")).toBe(true);
    expect(encode64(new Uint8Array([65, 66]))).toBe("QUI=");
  });
  it("uploads exact bytes in bounded chunks including empty files", async () => {
    const fetch = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(async () => json({ ready: true }));
    const progress = vi.fn();
    const data = new Uint8Array(CHUNK_SIZE + 4).fill(65),
      file = new File([data], "sample.bin");
    const [attachment] = await uploadFiles(
      [{ id: "file", file }],
      workspace.workspace_id,
      progress,
    );
    expect(attachment).toMatchObject({
      id: "file",
      size: data.length,
      media_type: "application/octet-stream",
    });
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(progress).toHaveBeenCalledTimes(2);
    const body = JSON.parse(String(fetch.mock.calls[1][1]?.body));
    expect(body.index).toBe(1);
    expect(body.data).toBe("QUFBQQ==");
    await uploadFiles(
      [
        {
          id: "empty",
          file: new File([], "empty.txt", { type: "text/plain" }),
        },
      ],
      workspace.workspace_id,
      progress,
    );
    expect(fetch).toHaveBeenCalledTimes(3);
  });
  it("rejects oversized files and incomplete final acknowledgements", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async () =>
      json({ ready: false }),
    );
    await expect(
      uploadFiles(
        [{ id: "one", file: new File(["data"], "a.txt") }],
        "workspace",
        vi.fn(),
      ),
    ).rejects.toThrow("did not finish");
    await expect(
      uploadFiles(
        [
          {
            id: "large",
            file: new File([new Uint8Array(MAX_FILE_SIZE + 1)], "large.bin"),
          },
        ],
        "workspace",
        vi.fn(),
      ),
    ).rejects.toThrow("8 MiB");
  });
  it("checks identity, index, length and SHA-256 before exposing downloads", async () => {
    const data = new TextEncoder().encode("test"),
      file = {
        id: "file",
        workspace_id: "workspace",
        name: "a.txt",
        media_type: "text/plain",
        size: 4,
        sha256: await digest(data.buffer),
      };
    const fetch = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(async () =>
        json({ attachment: file, index: 0, data: encode64(data) }),
      );
    expect(await (await fileBlob(file)).text()).toBe("test");
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => {});
    vi.useFakeTimers();
    await downloadFile(file);
    expect(click).toHaveBeenCalled();
    vi.runAllTimers();
    vi.useRealTimers();
    fetch.mockImplementation(async () =>
      json({
        attachment: { ...file, id: "wrong" },
        index: 0,
        data: encode64(data),
      }),
    );
    await expect(fileBlob(file)).rejects.toThrow("identity");
    fetch.mockImplementation(async () =>
      json({ attachment: file, index: 1, data: encode64(data) }),
    );
    await expect(fileBlob(file)).rejects.toThrow("identity");
    fetch.mockImplementation(async () =>
      json({ attachment: file, index: 0, data: "YQ==" }),
    );
    await expect(fileBlob(file)).rejects.toThrow("Incomplete");
    fetch.mockImplementation(async () =>
      json({ attachment: file, index: 0, data: "Zm9vZA==" }),
    );
    await expect(fileBlob(file)).rejects.toThrow("checksum");
    await expect(fileBlob({ ...file, size: -1 })).rejects.toThrow();
  });
});
