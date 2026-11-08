import { readSession } from "../lib/query";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "../lib/schema";
import { errorMessage, local } from "../lib/api";
import { configSchema, type Config } from "../lib/contracts";
import { savePending } from "../lib/pending";
import { AuthForm } from "../features/auth/auth-form";
import { ChatPage } from "../features/chat/chat-page";
import { SettingsPage } from "../features/settings/settings-page";
import { Button } from "../components/ui/button";
export function LocalApp() {
  const cache = useQueryClient(),
    [error, setError] = useState(""),
    [notice, setNotice] = useState("");
  const status = useQuery({
    queryKey: ["status"],
    queryFn: ({ signal }) =>
      local("status", z.object({ initialized: z.boolean() }), { signal }),
  });
  const config = useQuery({
    queryKey: ["config"],
    queryFn: ({ signal }) =>
      readSession(() => local("config", configSchema, { signal })),
    enabled: !!status.data,
    refetchInterval: (query) => (query.state.data === null ? false : 5000),
    refetchOnWindowFocus: (query) => query.state.data !== null,
    refetchOnReconnect: (query) => query.state.data !== null,
  });
  async function login(body: {
    username: string;
    password: string;
    token?: string;
  }) {
    await local(status.data?.initialized ? "login" : "setup", z.unknown(), {
      body,
    });
    setNotice("");
    cache.removeQueries({ predicate: (q) => q.queryKey[0] !== "status" });
    await cache.invalidateQueries({ queryKey: ["status"] });
    await config.refetch();
  }
  async function clearSession() {
    savePending(null);
    history.replaceState(null, "", location.pathname);
    await cache.cancelQueries();
    cache.clear();
    await status.refetch();
    await config.refetch();
    setError("");
  }
  async function logout() {
    try {
      await local("logout", z.unknown(), { body: {} });
      await clearSession();
    } catch (e) {
      setError(errorMessage(e));
    }
  }
  if (config.data === null)
    return (
      <AuthForm
        setup={!status.data?.initialized}
        onSubmit={login}
        notice={notice}
      />
    );
  if (!config.data)
    return (
      <main className="auth-page">
        <h1>Cuttlefish</h1>
        {status.error || config.error ? (
          <>
            <p role="alert">{errorMessage(status.error ?? config.error)}</p>
            <Button
              onClick={() => {
                void status.refetch();
                void config.refetch();
              }}
            >
              Retry
            </Button>
          </>
        ) : (
          <p role="status">Opening workspace…</p>
        )}
      </main>
    );
  return (
    <LocalWorkspace
      config={config.data}
      error={error}
      onLogout={() => void logout()}
      onPasswordChanged={() => {
        setNotice("Password changed. Sign in with your new password.");
        void clearSession();
      }}
    />
  );
}

function LocalWorkspace({
  config,
  error,
  onLogout,
  onPasswordChanged,
}: {
  config: Config;
  error: string;
  onLogout: () => void;
  onPasswordChanged: () => void;
}) {
  const hasChat = config.role !== "agent" && config.features?.chat === true;
  const canChat = hasChat && config.database.state === "ready";
  // Opening setup stays on setup when provisioning finishes. Background state
  // updates must not navigate away from an unfinished settings form.
  const [settings, setSettings] = useState(!canChat);
  return (
    <>
      {error && (
        <p role="alert" className="global-error">
          {error}
        </p>
      )}
      {hasChat && (
        <ChatPage
          config={config}
          active={!settings && canChat}
          onSettings={() => setSettings(true)}
          onLogout={onLogout}
        />
      )}
      {(settings || !canChat) && (
        <SettingsPage
          config={config}
          onBack={canChat ? () => setSettings(false) : undefined}
          onLogout={onLogout}
          onPasswordChanged={onPasswordChanged}
        />
      )}
    </>
  );
}
