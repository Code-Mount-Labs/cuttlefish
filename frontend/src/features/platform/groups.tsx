import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "../../lib/schema";
import { Plus, RefreshCw } from "lucide-react";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { Textarea } from "../../components/ui/textarea";
import {
  Dialog,
  DialogTrigger,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from "../../components/ui/dialog";
import { FormField } from "../../components/form-field";
import { errorMessage, request, safeWebUrl } from "../../lib/api";
import {
  allPages,
  groupSchema,
  machineSchema,
  installCommand,
  type Group,
} from "./api";
function Copy({ value, label }: { value: string; label: string }) {
  const [message, setMessage] = useState("");
  return (
    <>
      <Button
        variant="outline"
        onClick={() =>
          void navigator.clipboard.writeText(value).then(
            () => setMessage("Copied"),
            () => setMessage("Select and copy the value manually."),
          )
        }
      >
        {label}
      </Button>
      <span role="status">{message}</span>
    </>
  );
}
function ConnectMachine({ group }: { group: Group }) {
  const [open, setOpen] = useState(false);
  const invitation = useMutation({
    mutationFn: () =>
      request(
        `/api/v1/groups/${group.group_id}/invitations`,
        z.object({ invitation: z.string() }),
        { body: {} },
      ),
    onSuccess: () => setOpen(true),
  });
  return (
    <Dialog
      open={open}
      onOpenChange={() => {
        setOpen(false);
        invitation.reset();
      }}
    >
      <DialogTrigger asChild>
        <Button
          onClick={(event) => {
            event.preventDefault();
            invitation.mutate();
          }}
          disabled={invitation.isPending}
        >
          <Plus />
          Connect machine
        </Button>
      </DialogTrigger>
      {invitation.error && (
        <p role="alert" className="error">
          {errorMessage(invitation.error)}
        </p>
      )}

      <DialogContent>
        <DialogTitle>Connect machine</DialogTitle>
        <DialogDescription>
          Run the installer on Linux or macOS, then enter the group ID and
          enrollment code.
        </DialogDescription>
        <FormField label="Install command">
          {(id) => (
            <Textarea
              id={id}
              readOnly
              rows={4}
              value={installCommand(location.origin)}
            />
          )}
        </FormField>
        <Copy
          value={installCommand(location.origin)}
          label="Copy install command"
        />
        <FormField label="Platform address">
          {(id) => <Input id={id} readOnly value={location.origin} />}
        </FormField>
        <FormField label="Group ID">
          {(id) => <Input id={id} readOnly value={group.group_id} />}
        </FormField>
        <FormField label="Enrollment code">
          {(id) => (
            <Input id={id} readOnly value={invitation.data?.invitation ?? ""} />
          )}
        </FormField>
        <Copy
          value={invitation.data?.invitation ?? ""}
          label="Copy enrollment code"
        />
        <p className="field-help">
          Private code. Valid for one machine and 30 minutes. Keep the terminal
          open during this alpha release.
        </p>
      </DialogContent>
    </Dialog>
  );
}
function Machines({ group }: { group: Group }) {
  const machines = useQuery({
    queryKey: ["machines", group.group_id],
    queryFn: ({ signal }) =>
      allPages(
        `/api/v1/groups/${group.group_id}/installations`,
        machineSchema,
        (m) => m.installation_id,
        signal,
      ),
    refetchInterval: 15000,
  });
  return (
    <section className="group-detail">
      <div className="section-heading">
        <h2>{group.name}</h2>
        <ConnectMachine group={group} />
      </div>
      <div className="group-identifier">
        <code>{group.group_id}</code>
        <Copy value={group.group_id} label="Copy group ID" />
      </div>
      {machines.isPending && <p role="status">Loading machines…</p>}
      {machines.error && (
        <p role="alert" className="error">
          {errorMessage(machines.error)}
        </p>
      )}
      {machines.data?.length === 0 && <p>No machines connected.</p>}
      <ul className="machine-list">
        {machines.data?.map((machine) => (
          <li key={machine.installation_id}>
            <div>
              <strong>{machine.name}</strong>
              <span>
                {
                  {
                    router: "Router",
                    agent: "Agent",
                    router_agent: "Router + agent",
                  }[machine.role]
                }
              </span>
            </div>
            <span className={machine.online ? "online" : "muted"}>
              {machine.online ? "Ready" : "Offline / starting"}
            </span>
            <div className="machine-links">
              {(
                [
                  ["Router dashboard", machine.router_dashboard_url],
                  ["Agent dashboard", machine.agent_dashboard_url],
                ] as const
              ).map(
                ([label, url]) =>
                  safeWebUrl(url) && (
                    <a
                      key={label}
                      href={safeWebUrl(url)}
                      target="_blank"
                      rel="noopener noreferrer"
                    >
                      {label}
                    </a>
                  ),
              )}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
export function Groups() {
  const cache = useQueryClient(),
    [selected, setSelected] = useState<string | null>(null),
    [open, setOpen] = useState(false);
  const groups = useQuery({
    queryKey: ["groups"],
    queryFn: ({ signal }) =>
      allPages("/api/v1/groups", groupSchema, (g) => g.group_id, signal),
    refetchInterval: 15000,
  });
  const create = useMutation({
    mutationFn: (name: string) =>
      request("/api/v1/groups", z.object({ group_id: z.string() }), {
        body: { name },
      }),
    onSuccess: async (data) => {
      setSelected(data.group_id);
      setOpen(false);
      await cache.invalidateQueries({ queryKey: ["groups"] });
    },
  });
  const group = groups.data?.find((g) => g.group_id === selected);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <main className="platform-page">
        <div className="section-heading">
          <h1>Groups</h1>
          <div className="row-actions">
            <Button
              variant="ghost"
              size="icon"
              aria-label="Refresh groups"
              onClick={() => void groups.refetch()}
            >
              <RefreshCw />
            </Button>
            <DialogTrigger asChild>
              <Button>
                <Plus />
                Create group
              </Button>
            </DialogTrigger>
          </div>
        </div>
        {groups.isPending && <p role="status">Loading groups…</p>}
        {groups.error && (
          <p role="alert" className="error">
            {errorMessage(groups.error)}
          </p>
        )}
        {groups.data?.length === 0 && (
          <p>No groups yet. Create a group to connect your first machine.</p>
        )}
        <ul className="group-list">
          {groups.data?.map((g) => (
            <li key={g.group_id}>
              <button
                className="group-row"
                aria-label={`${g.name}, ${g.agents} agents, ${g.online} online`}
                aria-pressed={selected === g.group_id}
                onClick={() => setSelected(g.group_id)}
              >
                <strong>{g.name}</strong>
                <span>{g.agents} agents</span>
                <span>{g.online} online</span>
              </button>
            </li>
          ))}
        </ul>
        {group && <Machines key={group.group_id} group={group} />}
        <DialogContent>
          <DialogTitle>Create group</DialogTitle>
          <DialogDescription>
            Connect related agents to this group.
          </DialogDescription>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              create.mutate(
                String(new FormData(e.currentTarget).get("name")).trim(),
              );
            }}
          >
            <FormField label="Group name">
              {(id) => <Input id={id} name="name" required maxLength={128} />}
            </FormField>
            <Button type="submit" disabled={create.isPending}>
              Create group
            </Button>
            {create.error && (
              <p role="alert" className="error">
                {errorMessage(create.error)}
              </p>
            )}
          </form>
        </DialogContent>
      </main>
    </Dialog>
  );
}
