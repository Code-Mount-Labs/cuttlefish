import { readSession } from "../lib/query";
import { ChangePasswordForm } from "../features/auth/change-password-form";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "../lib/schema";
import { BookOpen, Folder, LogOut, UserRound } from "lucide-react";
import { errorMessage, request } from "../lib/api";
import { Button } from "../components/ui/button";
import { Sidebar } from "../components/sidebar";
import { AuthForm } from "../features/auth/auth-form";
import { Groups } from "../features/platform/groups";
export function PlatformApp() {
  const cache = useQueryClient(),
    [register, setRegister] = useState(false),
    [page, setPage] = useState<"groups" | "guide" | "account">("groups"),
    [error, setError] = useState(""),
    [notice, setNotice] = useState("");
  const session = useQuery({
    queryKey: ["session"],
    queryFn: ({ signal }) =>
      readSession(async () => {
        await request("/api/v1/auth/session", z.unknown(), { signal });
        return true;
      }),
    refetchInterval: (query) => (query.state.data === null ? false : 15000),
    refetchOnWindowFocus: (query) => query.state.data !== null,
    refetchOnReconnect: (query) => query.state.data !== null,
  });
  async function login(body: { username: string; password: string }) {
    await request(
      `/api/v1/auth/${register ? "register" : "login"}`,
      z.unknown(),
      { body },
    );
    setNotice("");
    setRegister(false);
    cache.removeQueries({ predicate: (q) => q.queryKey[0] !== "session" });
    await cache.invalidateQueries({ queryKey: ["session"] });
  }
  async function clearSession() {
    await cache.cancelQueries();
    cache.clear();
    setError("");
    await session.refetch();
  }
  async function logout() {
    try {
      await request("/api/v1/auth/logout", z.unknown(), { body: {} });
      await clearSession();
    } catch (e) {
      setError(errorMessage(e));
    }
  }
  if (session.data === null)
    return (
      <AuthForm
        notice={notice}
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
  if (session.error && session.data === undefined)
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
            className="justify-start"
            data-navigate
            aria-current={page === "groups" ? "page" : undefined}
            onClick={() => setPage("groups")}
          >
            <Folder />
            Groups
          </Button>
          <Button
            variant="ghost"
            className="justify-start"
            data-navigate
            aria-current={page === "guide" ? "page" : undefined}
            onClick={() => setPage("guide")}
          >
            <BookOpen />
            Installation
          </Button>
        </nav>
        <div className="sidebar-bottom">
          <Button
            variant="ghost"
            className="justify-start"
            data-navigate
            aria-current={page === "account" ? "page" : undefined}
            onClick={() => setPage("account")}
          >
            <UserRound />
            Account
          </Button>
          <Button
            variant="ghost"
            className="justify-start font-normal text-muted-foreground"
            onClick={() => void logout()}
          >
            <LogOut />
            Sign out
          </Button>
        </div>
      </Sidebar>
      <div className="platform-content">
        {(error || session.error) && (
          <p role="alert" className="error">
            {error || errorMessage(session.error)}
          </p>
        )}
        {page === "groups" ? (
          <Groups />
        ) : page === "account" ? (
          <main className="platform-page">
            <h1>Account</h1>
            <section className="settings-section">
              <h2>Password</h2>
              <ChangePasswordForm
                scope="platform"
                onChanged={() => {
                  setNotice(
                    "Password changed. Sign in with your new password.",
                  );
                  void clearSession();
                }}
              />
            </section>
          </main>
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
