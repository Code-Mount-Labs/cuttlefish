import { useRef, useState } from "react";
import { ArrowUp, Paperclip, RotateCcw } from "lucide-react";
import { Button } from "../../components/ui/button";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupTextarea,
} from "../../components/ui/input-group";
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
  working = false,
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
  working?: boolean;
}) {
  const picker = useRef<HTMLInputElement>(null);
  const [dragging, setDragging] = useState(false);
  const canSend = !disabled && (pending || !!text.trim() || files.length > 0);
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
          if (canSend) onSend();
        }}
        onDragOver={(e) => {
          if (
            attachments &&
            !locked &&
            e.dataTransfer.types.includes("Files")
          ) {
            e.preventDefault();
            setDragging(true);
          }
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null))
            setDragging(false);
        }}
        onDrop={(e) => {
          e.preventDefault();
          setDragging(false);
          if (attachments && !locked) onFiles(Array.from(e.dataTransfer.files));
        }}
      >
        <InputGroup
          data-dragging={dragging}
          className="composer-input-group data-[dragging=true]:ring-2 data-[dragging=true]:ring-ring"
        >
          {dragging && (
            <InputGroupAddon align="block-start">
              <span role="status">Drop files to attach</span>
            </InputGroupAddon>
          )}
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
          <InputGroupTextarea
            aria-describedby="chat-status notice"
            className="min-h-20 max-h-56 px-4 py-3 text-base md:text-base"
            id="chat-message"
            value={text}
            onChange={(e) => onText(e.target.value)}
            placeholder={working ? "Add a follow-up…" : "Message Cuttlefish"}
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
          {working && (
            <p className="composer-queue-hint">
              Follow-ups are saved and run after the current turn.
            </p>
          )}
          <InputGroupAddon
            align="block-end"
            className="composer-actions justify-between px-3 pt-1.5 pb-2.5"
          >
            <Button
              id="attach-files"
              type="button"
              variant="ghost"
              size="icon"
              disabled={locked || !attachments}
              aria-label="Attach files"
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
              disabled={!canSend}
              aria-label={
                pending
                  ? "Retry same request"
                  : working
                    ? "Queue follow-up"
                    : "Send message"
              }
            >
              {pending ? <RotateCcw /> : <ArrowUp />}
            </Button>
          </InputGroupAddon>
        </InputGroup>
      </form>
    </div>
  );
}
