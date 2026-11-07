import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "../lib/schema";
import { ApiError, errorMessage, local } from "../lib/api";
import { configSchema } from "../lib/contracts";
import { savePending } from "../lib/pending";
import { AuthForm } from "../features/auth/auth-form";
import { ChatPage } from "../features/chat/chat-page";
import { SettingsPage } from "../features/settings/settings-page";
import { Button } from "../components/ui/button";
export function LocalApp() {
  const cache = useQueryClient(),
    [settings, setSettings] = useState(false),
    [error, setError] = useState("");
  const status = useQuery({
    queryKey: ["status"],
    queryFn: ({ signal }) =>
      local("status", z.object({ initialized: z.boolean() }), { signal }),
  });
  const config = useQuery({
    queryKey: ["config"],
    queryFn: ({ signal }) => local("config", configSchema, { signal }),
    enabled: !!status.data,
    refetchInterval: 5000,
  });
  async function login(body: {
    username: string;
    password: string;
    token?: string;
  }) {
    await local(status.data?.initialized ? "login" : "setup", z.unknown(), {
      body,
    });
    cache.removeQueries({ predicate: (q) => q.queryKey[0] !== "status" });
    await cache.invalidateQueries({ queryKey: ["status"] });
    await config.refetch();
  }
  async function logout() {
    try {
      await local("logout", z.unknown(), { body: {} });
      savePending(null);
      history.replaceState(null, "", location.pathname);
      await cache.cancelQueries();
      cache.clear();
      await status.refetch();
      await config.refetch();
      setSettings(false);
      setError("");
    } catch (e) {
      setError(errorMessage(e));
    }
  }
  if (config.error instanceof ApiError && config.error.status === 401)
    return <AuthForm setup={!status.data?.initialized} onSubmit={login} />;
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
  const canChat =
    config.data.role !== "agent" &&
    config.data.features?.chat === true &&
    config.data.database.state === "ready";
  return (
    <>
      {error && (
        <p role="alert" className="global-error">
          {error}
        </p>
      )}
      {canChat && (
        <ChatPage
          config={config.data}
          active={!settings}
          onSettings={() => setSettings(true)}
          onLogout={() => void logout()}
        />
      )}
      {(settings || !canChat) && (
        <SettingsPage
          config={config.data}
          onBack={canChat ? () => setSettings(false) : undefined}
          onLogout={() => void logout()}
        />
      )}
    </>
  );
}
