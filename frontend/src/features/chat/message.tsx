import { useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, Copy } from "lucide-react";
import { Button } from "../../components/ui/button";
import type { Attachment } from "../../lib/contracts";
import { downloadFile } from "../../lib/files";
import { errorMessage, safeWebUrl } from "../../lib/api";
import { FileCard } from "./file-card";
export function Message({
  speaker,
  text,
  files = [],
}: {
  speaker: "user" | "agent";
  text: string;
  files?: Attachment[];
}) {
  const [copied, setCopied] = useState(false),
    [error, setError] = useState("");
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      setError("Copy is unavailable in this browser.");
    }
  }
  return (
    <article
      className={`message ${speaker}`}
      aria-label={speaker === "user" ? "You" : "Cuttlefish"}
    >
      <div className="message-body">
        {speaker === "user" ? (
          text
        ) : (
          <Markdown
            remarkPlugins={[remarkGfm]}
            skipHtml
            components={{
              a: ({ href, children }) => {
                const file = files.find(
                  (f) =>
                    href &&
                    (href.startsWith("/") || href.startsWith("artifacts/")) &&
                    href.endsWith("/" + f.name),
                );
                return file ? (
                  <button
                    type="button"
                    className="inline-download"
                    onClick={() =>
                      void downloadFile(file).catch((e) =>
                        setError(errorMessage(e)),
                      )
                    }
                  >
                    {children}
                  </button>
                ) : safeWebUrl(href) ? (
                  <a href={href} target="_blank" rel="noopener noreferrer">
                    {children}
                  </a>
                ) : (
                  <span>{children}</span>
                );
              },
              img: ({ alt }) => <span>{alt}</span>,
              table: ({ children }) => (
                <div className="table-scroll">
                  <table>{children}</table>
                </div>
              ),
            }}
          >
            {text}
          </Markdown>
        )}
      </div>
      {files.map((file) => (
        <FileCard key={file.id} file={file} />
      ))}
      {speaker === "agent" && (
        <div className="message-actions">
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={() => void copy()}
            aria-label={copied ? "Copied" : "Copy response"}
            title={copied ? "Copied" : "Copy response"}
          >
            {copied ? <Check /> : <Copy />}
          </Button>
        </div>
      )}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </article>
  );
}
