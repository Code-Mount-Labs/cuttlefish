import { useRef } from "react";
import { ArrowUp, Paperclip, RotateCcw } from "lucide-react";
import { Button } from "../../components/ui/button";
import { Textarea } from "../../components/ui/textarea";
import type { DraftFile } from "../../lib/files";
import { DraftCard } from "./file-card";
export function Composer({
  text,
  onText,
  files,
  onFiles,
  onRemove,
  onSend,
  locked,
  disabled,
  pending,
  status,
  attachments,
}: {
  text: string;
  onText: (value: string) => void;
  files: DraftFile[];
  onFiles: (files: File[]) => void;
  onRemove: (id: string) => void;
  onSend: () => void;
  locked: boolean;
  disabled: boolean;
  pending: boolean;
  status: string;
  attachments: boolean;
}) {
  const picker = useRef<HTMLInputElement>(null);
  return (
    <div className="composer-dock">
      <p id="chat-status" role="status">
        {status}
      </p>
      <form
        id="chat-form"
        className="composer"
        onSubmit={(e) => {
          e.preventDefault();
          if (!disabled) onSend();
        }}
        onDragOver={(e) => {
          if (attachments && e.dataTransfer.types.includes("Files"))
            e.preventDefault();
        }}
        onDrop={(e) => {
          e.preventDefault();
          if (attachments && !locked) onFiles(Array.from(e.dataTransfer.files));
        }}
      >
        <div id="draft-attachments">
          {files.map((draft) => (
            <DraftCard
              key={draft.id}
              draft={draft}
              disabled={locked}
              onRemove={() => onRemove(draft.id)}
            />
          ))}
        </div>
        <label className="sr-only" htmlFor="chat-message">
          Message Cuttlefish
        </label>
        <Textarea
          id="chat-message"
          value={text}
          onChange={(e) => onText(e.target.value)}
          placeholder="Message Cuttlefish"
          maxLength={4000}
          required={!pending && !files.length}
          readOnly={locked}
          rows={2}
          onPaste={(e) => {
            if (attachments && !locked && e.clipboardData.files.length) {
              e.preventDefault();
              onFiles(Array.from(e.clipboardData.files));
            }
          }}
          onKeyDown={(e) => {
            if (
              e.key === "Enter" &&
              !e.shiftKey &&
              !e.nativeEvent.isComposing
            ) {
              e.preventDefault();
              e.currentTarget.form?.requestSubmit();
            }
          }}
        />
        <div className="composer-actions">
          <Button
            id="attach-files"
            type="button"
            variant="ghost"
            size="icon"
            disabled={locked || !attachments}
            aria-label="Attach files"
            title="Attach files (up to 8 MiB each)"
            onClick={() => picker.current?.click()}
          >
            <Paperclip />
          </Button>
          <input
            ref={picker}
            id="file-picker"
            type="file"
            multiple
            hidden
            onChange={(e) => {
              onFiles(Array.from(e.target.files ?? []));
              e.target.value = "";
            }}
          />
          <Button
            id="chat-send"
            type="submit"
            size="icon"
            disabled={disabled}
            aria-label={pending ? "Retry same request" : "Send message"}
          >
            {pending ? <RotateCcw /> : <ArrowUp />}
          </Button>
        </div>
      </form>
    </div>
  );
}
