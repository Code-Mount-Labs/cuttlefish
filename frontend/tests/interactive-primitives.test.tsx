import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Button } from "../src/components/ui/button";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  InputGroupText,
  InputGroupTextarea,
} from "../src/components/ui/input-group";
import {
  DropdownMenu,
  DropdownMenuPortal,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuItem,
  DropdownMenuCheckboxItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubTrigger,
  DropdownMenuSubContent,
} from "../src/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "../src/components/ui/tooltip";

describe("interactive shadcn primitives", () => {
  it("preserves link semantics when a button styles navigation", async () => {
    render(
      <Button asChild variant="outline" size="sm">
        <a href="#details">Details</a>
      </Button>,
    );
    const link = screen.getByRole("link", { name: "Details" });
    expect(link).toHaveAttribute("href", "#details");
    expect(link).toHaveAttribute("data-variant", "outline");
    await userEvent.setup().tab();
    expect(link).toHaveFocus();
    expect(screen.queryByRole("button")).toBeNull();
  });
  it("keeps addons out of the tab order and preserves native form control semantics", async () => {
    const submit = vi.fn(),
      action = vi.fn(),
      user = userEvent.setup();
    render(
      <form
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <InputGroup aria-label="Search group">
          <InputGroupAddon>
            <InputGroupText>Search</InputGroupText>
          </InputGroupAddon>
          <InputGroupInput aria-label="Search chats" />
          <InputGroupAddon align="inline-end">
            <InputGroupButton onClick={action}>Clear</InputGroupButton>
          </InputGroupAddon>
        </InputGroup>
        <InputGroup aria-label="Message group">
          <InputGroupAddon align="block-start">
            <InputGroupText>Message</InputGroupText>
          </InputGroupAddon>
          <InputGroupTextarea aria-label="Message" />
          <InputGroupAddon align="block-end">
            <InputGroupButton size="sm" type="submit" variant="default">
              Send
            </InputGroupButton>
            <InputGroupButton size="icon-xs" aria-label="Attach">
              +
            </InputGroupButton>
            <InputGroupButton size="icon-sm" disabled aria-label="Unavailable">
              ×
            </InputGroupButton>
          </InputGroupAddon>
        </InputGroup>
      </form>,
    );
    await user.tab();
    expect(screen.getByRole("textbox", { name: "Search chats" })).toHaveFocus();
    await user.keyboard("Planning");
    expect(screen.getByRole("textbox", { name: "Search chats" })).toHaveValue(
      "Planning",
    );
    await user.tab();
    expect(screen.getByRole("button", { name: "Clear" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(action).toHaveBeenCalledOnce();
    expect(submit).not.toHaveBeenCalled();
    await user.tab();
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus();
    await user.keyboard("Read the report{Enter}Then summarize it");
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue(
      "Read the report\nThen summarize it",
    );
    await user.tab();
    await user.keyboard("{Enter}");
    expect(submit).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Unavailable" })).toBeDisabled();
  });
  it("supports keyboard menu selection, checkbox settings and radio choices", async () => {
    const choose = vi.fn();
    function Menu() {
      const [checked, setChecked] = useState(false),
        [value, setValue] = useState("compact");
      return (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button>Options</Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            <DropdownMenuGroup>
              <DropdownMenuLabel inset>Chat</DropdownMenuLabel>
              <DropdownMenuItem inset onSelect={choose}>
                Copy<DropdownMenuShortcut>⌘C</DropdownMenuShortcut>
              </DropdownMenuItem>
              <DropdownMenuItem disabled>Unavailable</DropdownMenuItem>
              <DropdownMenuItem variant="destructive">
                End session
              </DropdownMenuItem>
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
            <DropdownMenuCheckboxItem
              checked={checked}
              onCheckedChange={setChecked}
              inset
            >
              Show activity
            </DropdownMenuCheckboxItem>
            <DropdownMenuRadioGroup value={value} onValueChange={setValue}>
              <DropdownMenuRadioItem value="compact">
                Compact
              </DropdownMenuRadioItem>
              <DropdownMenuRadioItem value="comfortable" inset>
                Comfortable
              </DropdownMenuRadioItem>
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      );
    }
    const user = userEvent.setup();
    render(<Menu />);
    await user.tab();
    await user.keyboard("{Enter}");
    expect(screen.getByRole("menu")).toBeVisible();
    expect(screen.getByRole("menuitem", { name: /Copy/ })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(choose).toHaveBeenCalledOnce();
    expect(screen.queryByRole("menu")).toBeNull();
    expect(screen.getByRole("button", { name: "Options" })).toHaveFocus();
    await user.keyboard("{Enter}");
    await user.click(screen.getByRole("menuitemcheckbox"));
    await user.click(screen.getByRole("button", { name: "Options" }));
    expect(screen.getByRole("menuitemcheckbox")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(
      screen.getByRole("menuitem", { name: "Unavailable" }),
    ).toHaveAttribute("aria-disabled", "true");
    await user.click(
      screen.getByRole("menuitemradio", { name: "Comfortable" }),
    );
    await user.click(screen.getByRole("button", { name: "Options" }));
    expect(
      screen.getByRole("menuitemradio", { name: "Comfortable" }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("menuitemradio", { name: "Compact" }),
    ).toHaveAttribute("aria-checked", "false");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
  });
  it("opens a portaled submenu with the keyboard and invokes its action", async () => {
    const exportChat = vi.fn(),
      user = userEvent.setup();
    render(
      <DropdownMenu>
        <DropdownMenuTrigger>More</DropdownMenuTrigger>
        <DropdownMenuContent align="end" sideOffset={8}>
          <DropdownMenuSub>
            <DropdownMenuSubTrigger inset>Export</DropdownMenuSubTrigger>
            <DropdownMenuPortal>
              <DropdownMenuSubContent>
                <DropdownMenuLabel>Format</DropdownMenuLabel>
                <DropdownMenuItem onSelect={exportChat}>
                  Markdown
                </DropdownMenuItem>
              </DropdownMenuSubContent>
            </DropdownMenuPortal>
          </DropdownMenuSub>
        </DropdownMenuContent>
      </DropdownMenu>,
    );
    await user.tab();
    await user.keyboard("{Enter}");
    await user.keyboard("{ArrowRight}");
    expect(
      await screen.findByRole("menuitem", { name: "Markdown" }),
    ).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(exportChat).toHaveBeenCalledOnce();
    expect(screen.queryByRole("menu")).toBeNull();
  });
  it("provides an accessible tooltip description on keyboard focus", async () => {
    const user = userEvent.setup();
    render(
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button aria-label="New chat">+</Button>
          </TooltipTrigger>
          <TooltipContent>Start a new conversation</TooltipContent>
        </Tooltip>
      </TooltipProvider>,
    );
    await user.tab();
    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "Start a new conversation",
    );
    expect(
      screen.getByRole("button", { name: "New chat" }),
    ).toHaveAccessibleDescription("Start a new conversation");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("tooltip")).toBeNull();
  });
  it("forwards a custom tooltip offset and provider delay", () => {
    render(
      <TooltipProvider delayDuration={100}>
        <Tooltip open>
          <TooltipTrigger>Info</TooltipTrigger>
          <TooltipContent sideOffset={6} className="custom-tooltip">
            Details
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>,
    );
    expect(screen.getByRole("tooltip")).toHaveTextContent("Details");
    expect(document.querySelector('[data-slot="tooltip-content"]')).toHaveClass(
      "custom-tooltip",
    );
  });
});
