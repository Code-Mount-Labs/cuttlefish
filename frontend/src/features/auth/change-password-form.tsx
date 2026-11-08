import { useRef, useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { FieldGroup } from "@/components/ui/field";
import { FormField } from "@/components/form-field";
import { errorMessage, local, request } from "@/lib/api";
import { z } from "@/lib/schema";

export function ChangePasswordForm({
  scope,
  onChanged,
}: {
  scope: "local" | "platform";
  onChanged: () => void;
}) {
  const form = useRef<HTMLFormElement>(null);
  const submitting = useRef(false);
  const [validation, setValidation] = useState("");
  const mutation = useMutation({
    mutationFn: (body: { current_password: string; new_password: string }) =>
      scope === "local"
        ? local("password", z.object({ ok: z.boolean() }), { body })
        : request("/api/v1/auth/password", z.object({ ok: z.boolean() }), {
            body,
          }),
    onSettled: () => {
      submitting.current = false;
    },
    onSuccess: () => {
      form.current?.reset();
      mutation.reset();
      onChanged();
    },
  });
  return (
    <form
      ref={form}
      aria-label="Change password"
      onChange={() => {
        setValidation("");
        mutation.reset();
      }}
      onSubmit={(event) => {
        event.preventDefault();
        if (submitting.current) return;
        const values = new FormData(event.currentTarget);
        const current_password = String(values.get("current_password"));
        const new_password = String(values.get("new_password"));
        if (new_password !== values.get("confirm_password")) {
          setValidation("The new passwords do not match.");
          return;
        }
        const size = new TextEncoder().encode(new_password).length;
        if (size < 12 || size > 1024) {
          setValidation("Use a password between 12 and 1024 bytes.");
          return;
        }
        submitting.current = true;
        mutation.mutate({ current_password, new_password });
      }}
    >
      <fieldset disabled={mutation.isPending}>
        <FieldGroup>
          <FormField label="Current password">
            {(id) => (
              <Input
                id={id}
                name="current_password"
                type="password"
                autoComplete="current-password"
                required
                maxLength={1024}
              />
            )}
          </FormField>
          <FormField label="New password">
            {(id) => (
              <Input
                id={id}
                name="new_password"
                type="password"
                autoComplete="new-password"
                required
                maxLength={1024}
              />
            )}
          </FormField>
          <FormField label="Confirm new password">
            {(id) => (
              <Input
                id={id}
                name="confirm_password"
                type="password"
                autoComplete="new-password"
                required
                maxLength={1024}
              />
            )}
          </FormField>
          <p className="text-sm text-muted-foreground">
            Changing your password signs you out on all devices.
          </p>
          <Button type="submit" disabled={mutation.isPending}>
            {mutation.isPending ? "Changing password…" : "Change password"}
          </Button>
          {(validation || mutation.error) && (
            <p role="alert" className="error">
              {validation || errorMessage(mutation.error)}
            </p>
          )}
        </FieldGroup>
      </fieldset>
    </form>
  );
}
