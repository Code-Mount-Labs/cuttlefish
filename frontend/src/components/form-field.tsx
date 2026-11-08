import { useId, type ReactNode } from "react";
import { Field, FieldLabel, FieldDescription } from "./ui/field";
export function FormField({
  label,
  children,
  description,
}: {
  label: string;
  children: (id: string) => ReactNode;
  description?: string;
}) {
  const id = useId();
  return (
    <Field>
      <FieldLabel htmlFor={id}>{label}</FieldLabel>
      {children(id)}
      {description && <FieldDescription>{description}</FieldDescription>}
    </Field>
  );
}
