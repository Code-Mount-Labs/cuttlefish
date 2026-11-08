import { UpdatesPanel } from "../updates/updates-panel";
import { ChangePasswordForm } from "../auth/change-password-form";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldLabel, FieldGroup } from "@/components/ui/field";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select";
import { useRef, useState, type ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "../../lib/schema";
import { ArrowLeft, LogOut } from "lucide-react";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { FormField } from "../../components/form-field";
import { local, errorMessage, safeWebUrl } from "../../lib/api";
import {
  workspaceSchema,
  providerSchema,
  type Config,
  type Workspace,
} from "../../lib/contracts";
function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="settings-section">
      <h2>{title}</h2>
      {children}
    </section>
  );
}
function useSetting(path: string) {
  const cache = useQueryClient();
  return useMutation({
    mutationFn: (body: unknown) => local(path, z.unknown(), { body }),
    onSuccess: async () => {
      await cache.invalidateQueries({ queryKey: ["config"] });
    },
  });
}
function Result({
  pending,
  error,
  success,
}: {
  pending: boolean;
  error: unknown;
  success: boolean;
}) {
  return (
    <p role="status" className={error ? "error" : "field-help"}>
      {pending
        ? "Saving…"
        : error
          ? errorMessage(error)
          : success
            ? "Saved"
            : ""}
    </p>
  );
}
function Remote({ remote }: { remote: Config["remote"] }) {
  const [mode, setMode] = useState(remote.choice.mode),
    mutation = useSetting("remote");
  const url = safeWebUrl(remote.public_url, true);
  return (
    <Section title="Remote access">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          const data = new FormData(e.currentTarget);
          mutation.mutate(
            mode === "existing"
              ? { mode, origin: String(data.get("origin")) }
              : { mode },
          );
        }}
      >
        <FieldGroup>
          <FormField label="Access">
            {(id) => (
              <NativeSelect
                id={id}
                value={mode}
                onChange={(e) => setMode(e.target.value as typeof mode)}
              >
                <NativeSelectOption value="local">
                  Local only
                </NativeSelectOption>
                <NativeSelectOption value="managed">
                  Cuttlefish managed relay
                </NativeSelectOption>
                <NativeSelectOption value="existing">
                  Existing HTTPS address
                </NativeSelectOption>
              </NativeSelect>
            )}
          </FormField>
          {mode === "existing" && (
            <FormField label="HTTPS address">
              {(id) => (
                <Input
                  id={id}
                  name="origin"
                  type="url"
                  defaultValue={remote.choice.origin}
                  required
                />
              )}
            </FormField>
          )}
          <div className="form-actions">
            <Button type="submit" disabled={mutation.isPending}>
              Save access
            </Button>
            <span>{remote.state.replaceAll("_", " ")}</span>
          </div>
          <Result
            pending={mutation.isPending}
            error={mutation.error}
            success={mutation.isSuccess}
          />
          {url && remote.state === "ready" && (
            <a
              href={url.replace(/\/$/, "") + "/"}
              target="_blank"
              rel="noopener noreferrer"
            >
              Open remote dashboard
            </a>
          )}
        </FieldGroup>
      </form>
    </Section>
  );
}
function Workspaces() {
  const cache = useQueryClient(),
    pending = useRef<Workspace | null>(null),
    create = useSetting("workspaces");
  const rows = useQuery({
    queryKey: ["local-workspaces"],
    queryFn: ({ signal }) =>
      local(
        "workspaces",
        z.array(
          z.object({ workspace: workspaceSchema, directory: z.string() }),
        ),
        { signal },
      ),
  });
  const providers = useMutation({
    mutationFn: () =>
      local(
        "providers",
        z.array(
          z.object({
            provider: providerSchema,
            installed: z.boolean(),
            signed_in: z.boolean(),
          }),
        ),
      ),
  });
  return (
    <Section title="Providers and workspaces">
      <Button
        variant="outline"
        onClick={() => providers.mutate()}
        disabled={providers.isPending}
      >
        Check local sign-ins
      </Button>
      {providers.data?.map((p) => (
        <p key={p.provider}>
          {p.provider === "claude" ? "Claude Code" : "Codex"}:{" "}
          {!p.installed
            ? "Install the CLI"
            : p.signed_in
              ? "Signed in"
              : "Sign in on this machine"}
        </p>
      ))}
      {providers.error && (
        <p role="alert" className="error">
          {errorMessage(providers.error)}
        </p>
      )}
      <p className="field-help">
        Sign in locally using <code>codex login</code> or{" "}
        <code>claude auth login</code>.
      </p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          const data = new FormData(e.currentTarget);
          pending.current ??= {
            workspace_id: crypto.randomUUID(),
            name: String(data.get("name")),
            provider: data.get("provider") === "claude" ? "claude" : "codex",
            allow_edits: data.has("edits"),
          };
          create.mutate(pending.current, {
            onSuccess: () => {
              pending.current = null;
              void cache.invalidateQueries({ queryKey: ["local-workspaces"] });
              void cache.invalidateQueries({ queryKey: ["workspaces"] });
            },
          });
        }}
      >
        <FieldGroup>
          <FormField label="Workspace name">
            {(id) => (
              <Input
                id={id}
                name="name"
                required
                maxLength={128}
                disabled={create.isPending}
              />
            )}
          </FormField>
          <FormField label="Provider">
            {(id) => (
              <NativeSelect id={id} name="provider">
                <NativeSelectOption value="codex">Codex</NativeSelectOption>
                <NativeSelectOption value="claude">
                  Claude Code
                </NativeSelectOption>
              </NativeSelect>
            )}
          </FormField>
          <Field orientation="horizontal">
            <Checkbox id="workspace-edits" name="edits" />
            <FieldLabel htmlFor="workspace-edits">Allow file edits</FieldLabel>
          </Field>
          <p className="field-help">
            Creates a separate local folder. Permissions are fixed for this
            workspace.
          </p>
          <Button type="submit" disabled={create.isPending}>
            Create workspace
          </Button>
          <Result
            pending={create.isPending}
            error={create.error}
            success={create.isSuccess}
          />
        </FieldGroup>
      </form>
      <ul className="workspace-rows">
        {rows.data?.map(({ workspace: w, directory }) => (
          <li key={w.workspace_id}>
            <strong>{w.name}</strong>
            <span>
              {w.provider === "claude" ? "Claude Code" : "Codex"} /{" "}
              {w.allow_edits ? "File edits allowed" : "Read only"}
            </span>
            <code>{directory}</code>
          </li>
        ))}
      </ul>
      {rows.error && (
        <p className="error" role="alert">
          {errorMessage(rows.error)}
        </p>
      )}
    </Section>
  );
}
function Database({ database }: { database: Config["database"] }) {
  const [mode, setMode] = useState(
      database.mode === "existing" ? "existing" : "managed",
    ),
    mutation = useSetting("database"),
    retry = useSetting("database/retry");
  return (
    <Section title="Execution database">
      <p role="status">{database.state.replaceAll("_", " ")}</p>
      {database.error_code && (
        <p role="alert" className="error">
          {database.error_code}
        </p>
      )}
      {database.error_code?.startsWith("docker_") && (
        <p>
          Run <code>cuttlefish install-docker</code> on this machine, then
          retry.
        </p>
      )}
      {(database.state === "unconfigured" || database.state === "failed") && (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            const data = new FormData(e.currentTarget);
            mutation.mutate(
              mode === "managed"
                ? { mode, port: Number(data.get("port")) }
                : mode === "existing"
                  ? { mode, url: String(data.get("url")) }
                  : { mode },
            );
          }}
        >
          <FieldGroup>
            <FormField label="Database setup">
              {(id) => (
                <NativeSelect
                  id={id}
                  value={mode}
                  onChange={(e) => setMode(e.target.value)}
                >
                  <NativeSelectOption value="managed">
                    Create PostgreSQL on this machine
                  </NativeSelectOption>
                  <NativeSelectOption value="existing">
                    Use existing PostgreSQL
                  </NativeSelectOption>
                  <NativeSelectOption value="later">
                    Set up later
                  </NativeSelectOption>
                </NativeSelect>
              )}
            </FormField>
            {mode === "managed" && (
              <FormField label="Local port">
                {(id) => (
                  <Input
                    id={id}
                    name="port"
                    type="number"
                    required
                    min={1024}
                    max={65535}
                    defaultValue={54329}
                  />
                )}
              </FormField>
            )}
            {mode === "existing" && (
              <FormField label="PostgreSQL connection URL">
                {(id) => (
                  <Input
                    id={id}
                    name="url"
                    type="password"
                    autoComplete="off"
                    required
                  />
                )}
              </FormField>
            )}
            {mode === "managed" && (
              <p className="field-help">
                Installs Docker if needed. Database files stay on this machine;
                only the router connects to PostgreSQL.
              </p>
            )}
            <Button type="submit" disabled={mutation.isPending}>
              Save database
            </Button>
            <Result
              pending={mutation.isPending}
              error={mutation.error}
              success={mutation.isSuccess}
            />
          </FieldGroup>
        </form>
      )}
      {database.state === "failed" && (
        <>
          <Button
            variant="outline"
            disabled={retry.isPending}
            onClick={() => retry.mutate({})}
          >
            Retry saved setup
          </Button>
          <Result
            pending={retry.isPending}
            error={retry.error}
            success={retry.isSuccess}
          />
        </>
      )}
    </Section>
  );
}
function Model({ model }: { model: Config["model"] }) {
  const [secret, setSecret] = useState("");
  const mutation = useSetting("model");
  return (
    <Section title="Routing model">
      <p className="field-help">
        {model.configured
          ? `Configured: ${model.model}`
          : "A single eligible agent is selected without a routing model."}
      </p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          const form = e.currentTarget,
            data = new FormData(form);
          mutation.mutate(
            {
              endpoint: String(data.get("endpoint")),
              model: String(data.get("model")),
              api_key: String(data.get("key")),
            },
            {
              onSuccess: () => {
                setSecret("");
              },
            },
          );
        }}
      >
        <FieldGroup>
          <FormField label="Provider endpoint">
            {(id) => (
              <Input
                id={id}
                name="endpoint"
                type="url"
                required
                defaultValue="https://api.openai.com/v1/responses"
              />
            )}
          </FormField>
          <FormField label="Model">
            {(id) => <Input id={id} name="model" required maxLength={128} />}
          </FormField>
          <FormField label="API key">
            {(id) => (
              <Input
                id={id}
                name="key"
                value={secret}
                onChange={(event) => setSecret(event.target.value)}
                type="password"
                autoComplete="off"
                required
              />
            )}
          </FormField>
          <Button type="submit" disabled={mutation.isPending}>
            Save model
          </Button>
          <Result
            pending={mutation.isPending}
            error={mutation.error}
            success={mutation.isSuccess}
          />
        </FieldGroup>
      </form>
    </Section>
  );
}
export function SettingsPage({
  config,
  onBack,
  onLogout,
  onPasswordChanged,
}: {
  config: Config;
  onBack?: () => void;
  onLogout: () => void;
  onPasswordChanged?: () => void;
}) {
  return (
    <main id="settings" className="settings-page">
      <header className="settings-header">
        {onBack && (
          <Button variant="ghost" onClick={onBack}>
            <ArrowLeft />
            Chats
          </Button>
        )}
        <Button variant="ghost" onClick={onLogout}>
          <LogOut />
          Sign out
        </Button>
      </header>
      <h1>Settings</h1>
      <dl className="machine-identity">
        <dt>Machine</dt>
        <dd>{config.name}</dd>
        <dt>Role</dt>
        <dd>{config.role.replaceAll("_", " + ")}</dd>
        <dt>Group</dt>
        <dd>{config.group_id}</dd>
      </dl>
      {onPasswordChanged && (
        <Section title="Password">
          <ChangePasswordForm scope="local" onChanged={onPasswordChanged} />
        </Section>
      )}
      <UpdatesPanel enabled={config.features?.updates === true} />
      <Remote remote={config.remote} />
      {config.role !== "router" && config.features?.chat && <Workspaces />}
      {config.role !== "agent" && (
        <>
          <Database database={config.database} />
          <Model model={config.model} />
        </>
      )}
    </main>
  );
}
