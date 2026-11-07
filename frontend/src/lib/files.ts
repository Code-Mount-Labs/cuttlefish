import { z } from "./schema";
import { local } from "./api";
import { attachmentSchema, type Attachment } from "./contracts";
export const MAX_FILE_SIZE = 8 * 1024 * 1024,
  CHUNK_SIZE = 192 * 1024,
  MAX_FILES = 8;
export const isPreviewable = (type: string) =>
  ["image/png", "image/jpeg", "image/webp", "image/gif"].includes(type);
export const bytesLabel = (size: number) =>
  size < 1024
    ? `${size} B`
    : size < 1024 * 1024
      ? `${(size / 1024).toFixed(1)} KB`
      : `${(size / 1024 / 1024).toFixed(1)} MB`;
export async function digest(data: ArrayBuffer) {
  return Array.from(
    new Uint8Array(await crypto.subtle.digest("SHA-256", data)),
    (b) => b.toString(16).padStart(2, "0"),
  ).join("");
}
export function encode64(bytes: Uint8Array) {
  let value = "";
  for (let i = 0; i < bytes.length; i += 8192)
    value += String.fromCharCode(...bytes.subarray(i, i + 8192));
  return btoa(value);
}
export type DraftFile = { id: string; file: File };
export async function uploadFiles(
  files: DraftFile[],
  workspace: string,
  progress: (text: string) => void,
): Promise<Attachment[]> {
  const results: Attachment[] = [];
  for (const { id, file } of files) {
    if (file.size > MAX_FILE_SIZE)
      throw new Error(`${file.name} exceeds the 8 MiB file limit.`);
    const buffer = await file.arrayBuffer(),
      data = new Uint8Array(buffer);
    const attachment = {
      id,
      workspace_id: workspace,
      name: file.name,
      media_type: file.type || "application/octet-stream",
      size: file.size,
      sha256: await digest(buffer),
    };
    const count = Math.max(1, Math.ceil(file.size / CHUNK_SIZE));
    for (let index = 0; index < count; index++) {
      progress(
        `Uploading ${file.name} · ${Math.round((index / count) * 100)}%`,
      );
      const result = await local(
        "attachments",
        z.object({ ready: z.boolean() }),
        {
          body: {
            attachment,
            index,
            data: encode64(
              data.subarray(index * CHUNK_SIZE, (index + 1) * CHUNK_SIZE),
            ),
          },
        },
      );
      if (index === count - 1 && !result.ready)
        throw new Error("Upload did not finish. Retry to resume.");
    }
    results.push(attachment);
  }
  return results;
}
export async function fileBlob(file: Attachment, signal?: AbortSignal) {
  attachmentSchema.parse(file);
  const parts: Uint8Array<ArrayBuffer>[] = [];
  for (
    let index = 0;
    index < Math.max(1, Math.ceil(file.size / CHUNK_SIZE));
    index++
  ) {
    const chunk = await local(
      `attachments/${encodeURIComponent(file.id)}/chunks/${index}`,
      z.object({
        attachment: attachmentSchema,
        index: z.number(),
        data: z.string(),
      }),
      { signal },
    );
    if (
      chunk.attachment.id !== file.id ||
      chunk.attachment.workspace_id !== file.workspace_id ||
      chunk.attachment.sha256 !== file.sha256 ||
      chunk.index !== index
    )
      throw new Error("File identity changed.");
    parts.push(Uint8Array.from(atob(chunk.data), (c) => c.charCodeAt(0)));
  }
  const blob = new Blob(parts, { type: file.media_type });
  if (blob.size !== file.size) throw new Error("Incomplete file download.");
  if ((await digest(await blob.arrayBuffer())) !== file.sha256)
    throw new Error("File checksum mismatch.");
  return blob;
}
export async function downloadFile(file: Attachment) {
  const url = URL.createObjectURL(await fileBlob(file));
  const link = document.createElement("a");
  link.href = url;
  link.download = file.name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
