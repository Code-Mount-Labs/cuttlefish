import { useState } from "react";
import {
  LogOut,
  MessageSquare,
  Puzzle,
  Repeat2,
  Search,
  Settings,
  SquarePen,
  Workflow,
} from "lucide-react";
import { Sidebar } from "@/components/sidebar";
import { Button } from "@/components/ui/button";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
} from "@/components/ui/input-group";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

export type ChatSection = "chats" | "routines" | "workflows" | "plugins";
const sections = [
  { id: "chats", label: "Chats", icon: MessageSquare },
  { id: "routines", label: "Routines", icon: Repeat2 },
  { id: "workflows", label: "Workflows", icon: Workflow },
  { id: "plugins", label: "Plugins", icon: Puzzle },
] as const;

export function ChatNavigation({
  section,
  thread,
  conversations,
  locked,
  onSection,
  onNew,
  onConversation,
  onSettings,
  onLogout,
}: {
  section: ChatSection;
  thread: string | null;
  conversations: readonly { thread_id: string; title?: string | null }[];
  locked: boolean;
  onSection: (section: ChatSection) => void;
  onNew: () => void;
  onConversation: (id: string) => void;
  onSettings: () => void;
  onLogout: () => void;
}) {
  const [search, setSearch] = useState("");
  const filtered = conversations.filter((c) =>
    (c.title ?? "Chat").toLowerCase().includes(search.toLowerCase()),
  );
  return (
    <Sidebar>
      <div className="workspace-brand">
        <span>Cuttlefish</span>
        <TooltipProvider delayDuration={450}>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                id="new-chat"
                variant="ghost"
                size="icon"
                aria-label="New chat"
                disabled={locked}
                data-navigate
                onClick={onNew}
              >
                <SquarePen />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right">New chat</TooltipContent>
          </Tooltip>
        </TooltipProvider>
      </div>
      <nav className="workspace-nav" aria-label="Primary navigation">
        {sections.map((item) => (
          <Button
            key={item.id}
            aria-label={item.label}
            variant="ghost"
            className="w-full justify-start"
            aria-current={section === item.id ? "page" : undefined}
            data-navigate
            onClick={() => onSection(item.id)}
          >
            <item.icon />
            {item.label}
          </Button>
        ))}
      </nav>
      <div className="history-heading">Recent chats</div>
      <InputGroup className="history-search">
        <InputGroupAddon>
          <Search />
        </InputGroupAddon>
        <InputGroupInput
          id="chat-search"
          aria-label="Search chats"
          placeholder="Search chats"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
        />
      </InputGroup>
      <nav id="chat-list" className="chat-list" aria-label="Chat history">
        {filtered.map((chat) => (
          <Button
            key={chat.thread_id}
            className="chat-history-item w-full justify-start font-normal"
            variant="ghost"
            disabled={locked}
            data-navigate
            aria-current={
              section === "chats" && chat.thread_id === thread
                ? "page"
                : undefined
            }
            onClick={() => onConversation(chat.thread_id)}
          >
            <span className="truncate">{chat.title ?? "Chat"}</span>
          </Button>
        ))}
        {!filtered.length && (
          <p className="history-empty">
            {search ? "No matching chats" : "Your chats will appear here"}
          </p>
        )}
      </nav>
      <div className="sidebar-bottom workspace-account">
        <Button
          variant="ghost"
          className="justify-start"
          data-navigate
          onClick={onSettings}
        >
          <Settings />
          Settings
        </Button>
        <Button
          id="logout"
          variant="ghost"
          className="justify-start text-muted-foreground"
          onClick={onLogout}
        >
          <LogOut />
          Sign out
        </Button>
      </div>
    </Sidebar>
  );
}
