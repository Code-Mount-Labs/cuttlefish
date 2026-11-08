import { FieldGroup } from "@/components/ui/field";
import { useMutation } from "@tanstack/react-query";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { FormField } from "../../components/form-field";
import { errorMessage } from "../../lib/api";
export function AuthForm({
  setup = false,
  register = false,
  onSubmit,
  onToggle,
  notice,
}: {
  setup?: boolean;
  register?: boolean;
  onSubmit: (input: {
    username: string;
    password: string;
    token?: string;
  }) => Promise<void>;
  onToggle?: () => void;
  notice?: string;
}) {
  const mutation = useMutation({ mutationFn: onSubmit });
  return (
    <main className="auth-page" id="auth">
      <div className="auth-brand">Cuttlefish</div>
      <h1>
        {setup
          ? "Set up this machine"
          : register
            ? "Create account"
            : "Sign in"}
      </h1>
      {notice && <p role="status">{notice}</p>}
      <form
        id="auth-form"
        onSubmit={(e) => {
          e.preventDefault();
          const data = new FormData(e.currentTarget);
          mutation.mutate({
            username: String(data.get("username")),
            password: String(data.get("password")),
            ...(setup ? { token: String(data.get("token")) } : {}),
          });
        }}
      >
        <FieldGroup>
          {setup && (
            <FormField label="Setup code">
              {(id) => (
                <Input
                  id={id}
                  name="token"
                  type="password"
                  autoComplete="off"
                  required
                />
              )}
            </FormField>
          )}
          <FormField label="Username">
            {(id) => (
              <Input
                id={id}
                name="username"
                autoComplete="username"
                required
                minLength={3}
                maxLength={64}
              />
            )}
          </FormField>
          <FormField label="Password">
            {(id) => (
              <Input
                id={id}
                name="password"
                type="password"
                autoComplete={
                  setup || register ? "new-password" : "current-password"
                }
                required
                minLength={12}
                maxLength={1024}
              />
            )}
          </FormField>
          <Button type="submit" disabled={mutation.isPending}>
            {mutation.isPending
              ? "Signing in…"
              : setup
                ? "Create administrator"
                : register
                  ? "Create account"
                  : "Sign in"}
          </Button>
          {mutation.error && (
            <p className="error" role="alert">
              {errorMessage(mutation.error)}
            </p>
          )}
        </FieldGroup>
      </form>
      {onToggle && (
        <Button variant="link" onClick={onToggle}>
          {register ? "Sign in instead" : "Create account"}
        </Button>
      )}
    </main>
  );
}
