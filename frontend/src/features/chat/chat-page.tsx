import { AutomationPage } from "@/features/automations/automation-page";
import { ArrowDown, MoreHorizontal, Square } from "lucide-react";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
import { useEffect, useRef, useState } from "react";
import { Button } from "../../components/ui/button";
import { ChatNavigation, type ChatSection } from "./chat-navigation";
import type { Config } from "../../lib/contracts";
import { activityState, providerLabel } from "../../lib/activity";
import { useChat } from "./use-chat";
import { Message } from "./message";
import { TaskRecovery } from "./recovery";
import { TaskActivity } from "./activity";
import { DelegatedTasks } from "./delegated-tasks";
import { Composer } from "./composer";
export function ChatPage({
  config,
  active,
  onSettings,
  onLogout,
}: {
  config: Config;
  active: boolean;
  onSettings: () => void;
  onLogout: () => void;
}) {
  const [section, setSection] = useState<ChatSection>("chats");
  const chat = useChat(
    active,
    config.features?.activity === true,
    config.features?.collaboration === true,
  );
  const [away, setAway] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  function navigateSection(next: ChatSection) {
    setSection(next);
    requestAnimationFrame(() => {
      const target =
        next === "chats"
          ? document.getElementById("chat-message")
          : panel.current?.querySelector<HTMLElement>("main:not([hidden]) h1");
      target?.focus();
    });
  }
  function openChat(id: string | null) {
    setSection("chats");
    chat.navigate(id);
    setAway(false);
    requestAnimationFrame(() =>
      document.getElementById("chat-message")?.focus(),
    );
  }
  const scroll = useRef<HTMLDivElement>(null);
  const turns = chat.turns.data ?? [],
    events = chat.activity.data?.events ?? [],
    lastSeen = chat.activity.data?.last_seen_ms ?? null;
  const latest =
      turns.find((turn) => turn.state !== "completed") ?? turns.at(-1),
    state = latest
      ? activityState(
          latest,
          events.filter((e) => e.command_id === latest.request.command_id),
          lastSeen,
        )
      : { title: "Waiting for an update…" };
  useEffect(() => {
    if (scroll.current && !away)
      scroll.current.scrollTop = scroll.current.scrollHeight;
  }, [chat.turns.data, chat.activity.data, away]);
  const locked = chat.sending || !!chat.pending;
  const status =
    chat.progress ||
    (chat.queryError
      ? "Connection interrupted. Reconnecting… Your task may still be running."
      : chat.busy
        ? state.title
        : chat.ended
          ? "Session ended. Start a new chat to continue."
          : !chat.workspaces.data?.length
            ? "Create a workspace in Settings to start chatting."
            : "");
  return (
    <div className="app-layout" hidden={!active} id="chat-panel" ref={panel}>
      <ChatNavigation
        section={section}
        thread={chat.thread}
        conversations={chat.conversations.data ?? []}
        locked={locked}
        onSection={navigateSection}
        onNew={() => openChat(null)}
        onConversation={openChat}
        onSettings={onSettings}
        onLogout={onLogout}
      />
      <main
        className="conversation"
        hidden={section !== "chats"}
        data-empty={!chat.thread}
      >
        <header className="conversation-header">
          <div className="conversation-heading">
            <h1 id="conversation-title" tabIndex={-1}>
              {chat.thread
                ? (chat.conversations.data?.find(
                    (c) => c.thread_id === chat.thread,
                  )?.title ?? "Chat")
                : "Cuttlefish"}
            </h1>
          </div>
          <div className="conversation-tools">
            <label className="sr-only" htmlFor="chat-workspace">
              Workspace and provider
            </label>
            <NativeSelect
              id="chat-workspace"
              aria-label="Workspace and provider"
              value={chat.workspace}
              disabled={locked || !!chat.thread}
              onChange={(e) => chat.setWorkspace(e.target.value)}
            >
              {!chat.workspaces.data?.length && (
                <NativeSelectOption value="">
                  Select workspace
                </NativeSelectOption>
              )}
              {chat.workspaces.data?.map(({ workspace: w }) => (
                <NativeSelectOption key={w.workspace_id} value={w.workspace_id}>
                  {w.name} · {providerLabel(w.provider)}
                </NativeSelectOption>
              ))}
            </NativeSelect>
            {chat.thread && !chat.busy && !chat.ended && !locked && (
              <DropdownMenu modal={false}>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label="Conversation options"
                  >
                    <MoreHorizontal />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem id="end-chat" onSelect={chat.end}>
                    <Square />
                    End session
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </div>
        </header>
        <div
          id="conversation-scroll"
          className="conversation-scroll"
          ref={scroll}
          onScroll={(event) => {
            const el = event.currentTarget;
            setAway(el.scrollHeight - el.scrollTop - el.clientHeight >= 100);
          }}
        >
          <div className="transcript">
            {!chat.thread && (
              <h2 id="chat-welcome">What would you like to do?</h2>
            )}
            <DelegatedTasks
              tasks={chat.children.data ?? []}
              error={!!chat.children.error}
              onOpen={openChat}
            />
            <div
              id="messages"
              role="log"
              aria-label="Conversation"
              aria-live="polite"
              aria-relevant="additions text"
            >
              {turns.map((turn) => (
                <div className="turn" key={turn.request.command_id}>
                  {"message" in turn.request.action && (
                    <Message
                      speaker="user"
                      text={turn.request.action.message.text}
                      files={turn.request.action.message.attachments}
                    />
                  )}
                  <TaskActivity
                    turn={turn}
                    events={events.filter(
                      (e) => e.command_id === turn.request.command_id,
                    )}
                    lastSeen={lastSeen}
                  />
                  {config.features?.recovery &&
                    chat.thread &&
                    turn.state !== "completed" &&
                    events
                      .filter((e) => e.command_id === turn.request.command_id)
                      .at(-1)?.phase === "needs_attention" && (
                      <TaskRecovery
                        thread={chat.thread}
                        command={turn.request.command_id}
                      />
                    )}
                  {turn.outcome?.type === "completed" &&
                    turn.outcome.event.kind.type === "replied" && (
                      <Message
                        speaker="agent"
                        text={turn.outcome.event.kind.text ?? ""}
                        files={turn.outcome.event.artifacts}
                      />
                    )}
                  {turn.outcome?.type === "rejected" && (
                    <Message speaker="agent" text={turn.outcome.reason} />
                  )}
                </div>
              ))}
            </div>
          </div>
        </div>
        <div className="composer-container">
          {away && (
            <Button
              className="jump-to-latest"
              variant="secondary"
              size="sm"
              onClick={() => {
                setAway(false);
              }}
            >
              <ArrowDown />
              Jump to latest
            </Button>
          )}

          <p id="notice" role="alert" className="error">
            {chat.error}
          </p>
          <Composer
            text={chat.text}
            onText={chat.setText}
            files={chat.files}
            onFiles={chat.addFiles}
            onRemove={chat.removeFile}
            onSend={chat.send}
            locked={locked}
            disabled={
              chat.sending ||
              (chat.busy && !config.features?.followups) ||
              chat.ended ||
              !chat.workspace ||
              !!chat.queryError
            }
            pending={!!chat.pending}
            status={status}
            attachments={config.features?.attachments === true}
            working={chat.busy}
          />
        </div>
      </main>
      {(section === "routines" || section === "workflows") && (
        <AutomationPage key={section} section={section} onOpenChat={openChat} />
      )}
      {section === "plugins" && (
        <main className="automation-page">
          <h1 tabIndex={-1}>Plugins</h1>
          <p className="empty-section">No plugins installed.</p>
        </main>
      )}
    </div>
  );
}
