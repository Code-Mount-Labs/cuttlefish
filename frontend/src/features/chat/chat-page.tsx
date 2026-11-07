import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select";
import { useEffect, useRef, useState } from "react";
import { LogOut, Plus, Search, Settings } from "lucide-react";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { Sidebar } from "../../components/sidebar";
import type { Config } from "../../lib/contracts";
import { activityState, providerLabel } from "../../lib/activity";
import { useChat } from "./use-chat";
import { Message } from "./message";
import { TaskActivity } from "./activity";
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
  const chat = useChat(active, config.features?.activity === true),
    [search, setSearch] = useState("");
  const scroll = useRef<HTMLDivElement>(null),
    atBottom = useRef(true);
  const turns = chat.turns.data ?? [],
    events = chat.activity.data?.events ?? [],
    lastSeen = chat.activity.data?.last_seen_ms ?? null;
  const latest = turns.at(-1),
    state = latest
      ? activityState(
          latest,
          events.filter((e) => e.command_id === latest.request.command_id),
          lastSeen,
        )
      : { title: "Waiting for an update…" };
  useEffect(() => {
    if (scroll.current && atBottom.current)
      scroll.current.scrollTop = scroll.current.scrollHeight;
  }, [chat.turns.data, chat.activity.data]);
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
    <div className="app-layout" hidden={!active} id="chat-panel">
      <Sidebar>
        <div className="brand">Cuttlefish</div>
        <Button
          id="new-chat"
          variant="ghost"
          className="nav-action"
          disabled={locked}
          data-navigate
          onClick={() => chat.navigate(null)}
        >
          <Plus />
          New chat
        </Button>
        <div className="chat-search">
          <Search size={15} />
          <Input
            id="chat-search"
            aria-label="Search chats"
            placeholder="Search"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
        <nav id="chat-list" className="chat-list" aria-label="Chats">
          {chat.conversations.data
            ?.filter((c) =>
              (c.title ?? "Chat").toLowerCase().includes(search.toLowerCase()),
            )
            .map((c) => (
              <Button
                key={c.thread_id}
                variant="ghost"
                disabled={locked}
                data-navigate
                aria-current={c.thread_id === chat.thread ? "page" : undefined}
                title={c.title ?? "Chat"}
                onClick={() => chat.navigate(c.thread_id)}
              >
                {c.title ?? "Chat"}
              </Button>
            ))}
        </nav>
        <div className="sidebar-bottom">
          <Button variant="ghost" data-navigate onClick={onSettings}>
            <Settings />
            Settings
          </Button>
          <Button id="logout" variant="ghost" onClick={onLogout}>
            <LogOut />
            Sign out
          </Button>
        </div>
      </Sidebar>
      <main className="conversation">
        <header className="conversation-header">
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
              <NativeSelectOption value="">Select workspace</NativeSelectOption>
            )}
            {chat.workspaces.data?.map(({ workspace: w }) => (
              <NativeSelectOption key={w.workspace_id} value={w.workspace_id}>
                {w.name} · {providerLabel(w.provider)}
              </NativeSelectOption>
            ))}
          </NativeSelect>
          {chat.thread && !chat.busy && !chat.ended && !locked && (
            <Button id="end-chat" variant="ghost" size="sm" onClick={chat.end}>
              End session
            </Button>
          )}
        </header>
        <div
          id="conversation-scroll"
          className="conversation-scroll"
          ref={scroll}
          onScroll={(event) => {
            const el = event.currentTarget;
            atBottom.current =
              el.scrollHeight - el.scrollTop - el.clientHeight < 100;
          }}
        >
          <div className="transcript">
            {!chat.thread && <h1 id="chat-welcome">New chat</h1>}
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
              chat.busy ||
              chat.ended ||
              !chat.workspace ||
              !!chat.queryError
            }
            pending={!!chat.pending}
            status={status}
            attachments={config.features?.attachments === true}
          />
        </div>
      </main>
    </div>
  );
}
