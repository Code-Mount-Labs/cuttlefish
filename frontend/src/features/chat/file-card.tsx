import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Download, FileText, X } from "lucide-react";
import { Button } from "../../components/ui/button";
import type { Attachment } from "../../lib/contracts";
import {
  bytesLabel,
  downloadFile,
  fileBlob,
  isPreviewable,
  type DraftFile,
} from "../../lib/files";
import { errorMessage } from "../../lib/api";
import { useObjectUrl } from "../../hooks/use-object-url";
export function DraftCard({
  draft,
  disabled,
  onRemove,
}: {
  draft: DraftFile;
  disabled: boolean;
  onRemove: () => void;
}) {
  const url = useObjectUrl(
    isPreviewable(draft.file.type) ? draft.file : undefined,
  );
  return (
    <div className="file-card">
      {url ? <img src={url} alt="" /> : <FileText size={18} />}
      <span>
        {draft.file.name}
        <small>{bytesLabel(draft.file.size)}</small>
      </span>
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        disabled={disabled}
        aria-label={`Remove ${draft.file.name}`}
        onClick={onRemove}
      >
        <X />
      </Button>
    </div>
  );
}
export function FileCard({ file }: { file: Attachment }) {
  const [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const preview = useQuery({
    queryKey: ["file", file.id, file.sha256],
    queryFn: ({ signal }) => fileBlob(file, signal),
    enabled: isPreviewable(file.media_type),
    staleTime: Infinity,
    retry: false,
  });
  const url = useObjectUrl(preview.data);
  async function download() {
    setBusy(true);
    setError("");
    try {
      await downloadFile(file);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="attachment-result">
      <div className="file-card">
        <FileText size={18} />
        <span>
          {file.name}
          <small>{bytesLabel(file.size)}</small>
        </span>
        <Button
          variant="ghost"
          size="icon-sm"
          disabled={busy}
          aria-label={`Download ${file.name}`}
          onClick={() => void download()}
        >
          <Download />
        </Button>
      </div>
      {url && (
        <img
          className="artifact-preview"
          src={url}
          alt={file.name}
          loading="lazy"
        />
      )}
      {(error || preview.error) && (
        <p role="alert" className="error">
          {error || errorMessage(preview.error)}
        </p>
      )}
    </div>
  );
}
