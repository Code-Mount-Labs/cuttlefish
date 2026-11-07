import { useId, type ReactNode } from "react";
import { Label } from "./ui/label";
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
    <div className="form-field">
      <Label htmlFor={id}>{label}</Label>
      {children(id)}
      {description && <p className="field-help">{description}</p>}
    </div>
  );
}
