import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "../lib/schema";
import { BookOpen, Folder, LogOut } from "lucide-react";
import { ApiError, errorMessage, request } from "../lib/api";
import { Button } from "../components/ui/button";
import { Sidebar } from "../components/sidebar";
import { AuthForm } from "../features/auth/auth-form";
import { Groups } from "../features/platform/groups";
export function PlatformApp() {
  const cache = useQueryClient(),
    [register, setRegister] = useState(false),
    [page, setPage] = useState<"groups" | "guide">("groups"),
    [error, setError] = useState("");
  const session = useQuery({
    queryKey: ["session"],
    queryFn: ({ signal }) =>
      request("/api/v1/auth/session", z.unknown(), { signal }),
    refetchInterval: 15000,
  });
  async function login(body: { username: string; password: string }) {
    await request(
      `/api/v1/auth/${register ? "register" : "login"}`,
      z.unknown(),
      { body },
    );
    cache.removeQueries({ predicate: (q) => q.queryKey[0] !== "session" });
    await cache.invalidateQueries({ queryKey: ["session"] });
  }
  async function logout() {
    try {
      await request("/api/v1/auth/logout", z.unknown(), { body: {} });
      await cache.cancelQueries();
      cache.clear();
      setError("");
      await session.refetch();
    } catch (e) {
      setError(errorMessage(e));
    }
  }
  if (session.error instanceof ApiError && session.error.status === 401)
    return (
      <AuthForm
        register={register}
        onSubmit={login}
        onToggle={() => setRegister(!register)}
      />
    );
  if (session.isPending)
    return (
      <main className="auth-page">
        <p role="status">Opening workspace…</p>
      </main>
    );
  if (session.error)
    return (
      <main className="auth-page">
        <p role="alert">{errorMessage(session.error)}</p>
        <Button onClick={() => void session.refetch()}>Retry</Button>
      </main>
    );
  return (
    <div className="app-layout">
      <Sidebar>
        <div className="brand">Cuttlefish</div>
        <nav className="platform-nav" aria-label="Workspace">
          <Button
            variant="ghost"
            data-navigate
            aria-current={page === "groups" ? "page" : undefined}
            onClick={() => setPage("groups")}
          >
            <Folder />
            Groups
          </Button>
          <Button
            variant="ghost"
            data-navigate
            aria-current={page === "guide" ? "page" : undefined}
            onClick={() => setPage("guide")}
          >
            <BookOpen />
            Installation
          </Button>
        </nav>
        <div className="sidebar-bottom">
          <Button variant="ghost" onClick={() => void logout()}>
            <LogOut />
            Sign out
          </Button>
        </div>
      </Sidebar>
      <div className="platform-content">
        {error && (
          <p role="alert" className="error">
            {error}
          </p>
        )}
        {page === "groups" ? (
          <Groups />
        ) : (
          <main className="platform-page">
            <h1>Installation</h1>
            <section className="settings-section">
              <h2>Router + agent</h2>
              <p>
                Run the router, PostgreSQL, and first agent on one machine.
                Choose this for a single-machine setup.
              </p>
            </section>
            <section className="settings-section">
              <h2>Router only</h2>
              <p>
                Keep routing on an always-on server. Connect agents on separate
                machines.
              </p>
            </section>
            <section className="settings-section">
              <h2>Agent only</h2>
              <p>
                Add a laptop or server to a group that already has a router.
              </p>
            </section>
            <p>
              Create a group, choose Connect machine, and run the installer.
              Agents connect outbound. They do not need a public port or
              database password.
            </p>
          </main>
        )}
      </div>
    </div>
  );
}
