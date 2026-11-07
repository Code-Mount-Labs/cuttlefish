import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select";
import { FieldGroup } from "@/components/ui/field";
import { FormField } from "@/components/form-field";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { providerLabel } from "@/lib/activity";
import type { Workspace } from "@/lib/contracts";
import {
  firstRunTime,
  intervalLabel,
  localTime,
  schedulePreview,
  type Editor,
} from "./model";

export function AutomationEditor({
  editor,
  workspaces,
  pending,
  error,
  onClose,
  onSave,
}: {
  editor: Editor;
  workspaces: { workspace: Workspace }[];
  pending: boolean;
  error: string;
  onClose: () => void;
  onSave: (body: unknown) => void;
}) {
  const [validation, setValidation] = useState("");
  const [previewTime, setPreviewTime] = useState(Date.now);
  const routine = editor.kind === "routine";
  const [firstInput, setFirstInput] = useState(
    routine ? localTime(editor.value.first_run_ms) : "",
  );
  const [interval, setInterval] = useState(
    routine ? editor.value.interval_minutes : 0,
  );
  function submit(form: HTMLFormElement) {
    const fields = new FormData(form);
    const name = String(fields.get("name")).trim();
    const instructions = String(fields.get("instructions")).trim();
    const first = routine
      ? firstRunTime(editor.value.first_run_ms, String(fields.get("first")))
      : 0;
    if (!name || !instructions || (routine && !Number.isFinite(first))) {
      setValidation(
        routine
          ? "Enter a name, a prompt, and a valid schedule."
          : "Enter a name and a prompt.",
      );
      return;
    }
    setValidation("");
    const value = {
      id: editor.value.id,
      revision: editor.value.revision,
      name,
      instructions,
      workspace_id: String(fields.get("workspace")),
    };
    onSave(
      routine
        ? {
            type: "save_routine",
            routine: {
              ...value,
              enabled: editor.value.enabled,
              first_run_ms: first,
              interval_minutes: Number(fields.get("interval")),
            },
          }
        : { type: "save_workflow", workflow: value },
    );
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !pending) onClose();
      }}
    >
      <DialogContent
        className="sm:max-w-xl"
        aria-describedby="automation-description"
        showCloseButton={!pending}
      >
        <DialogTitle>
          {editor.value.revision ? "Edit" : "New"} {editor.kind}
        </DialogTitle>
        <DialogDescription id="automation-description" className="sr-only">
          {routine
            ? "Schedule a prompt for an agent to run automatically."
            : "Save a prompt to run whenever you need it."}
        </DialogDescription>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            submit(event.currentTarget);
          }}
          aria-busy={pending}
        >
          <fieldset disabled={pending} className="automation-fields">
            <FieldGroup>
              <FormField label="Prompt">
                {(id) => (
                  <Textarea
                    id={id}
                    name="instructions"
                    defaultValue={editor.value.instructions}
                    rows={5}
                    className="min-h-28"
                    maxLength={4000}
                    required
                    placeholder="What should the agent do?"
                  />
                )}
              </FormField>
              <FormField label="Agent workspace">
                {(id) => (
                  <NativeSelect
                    id={id}
                    name="workspace"
                    defaultValue={editor.value.workspace_id}
                    required
                  >
                    <NativeSelectOption value="" disabled>
                      Select a workspace
                    </NativeSelectOption>
                    {workspaces.map(({ workspace }) => (
                      <NativeSelectOption
                        key={workspace.workspace_id}
                        value={workspace.workspace_id}
                      >
                        {workspace.name} · {providerLabel(workspace.provider)}
                      </NativeSelectOption>
                    ))}
                  </NativeSelect>
                )}
              </FormField>
              {routine && (
                <fieldset className="automation-schedule">
                  <legend className="sr-only">Schedule</legend>
                  <div className="automation-field-row">
                    <FormField label="First run">
                      {(id) => (
                        <Input
                          id={id}
                          name="first"
                          type="datetime-local"
                          value={firstInput}
                          onChange={(event) => {
                            setFirstInput(event.target.value);
                            setPreviewTime(Date.now());
                          }}
                          required
                        />
                      )}
                    </FormField>
                    <FormField label="Repeat">
                      {(id) => (
                        <NativeSelect
                          id={id}
                          name="interval"
                          value={interval}
                          onChange={(event) => {
                            setInterval(Number(event.target.value));
                            setPreviewTime(Date.now());
                          }}
                        >
                          {[
                            ...new Set([
                              15,
                              60,
                              1440,
                              10080,
                              editor.value.interval_minutes,
                            ]),
                          ].map((minutes) => (
                            <NativeSelectOption key={minutes} value={minutes}>
                              {intervalLabel(minutes)}
                            </NativeSelectOption>
                          ))}
                        </NativeSelect>
                      )}
                    </FormField>
                  </div>
                  <p className="automation-timezone" role="status">
                    {schedulePreview(
                      editor.value,
                      firstInput,
                      interval,
                      previewTime,
                    )}
                  </p>
                  <p className="automation-timezone">
                    {Intl.DateTimeFormat().resolvedOptions().timeZone} · Repeats
                    use elapsed time.
                  </p>
                </fieldset>
              )}
              <FormField label="Name">
                {(id) => (
                  <Input
                    id={id}
                    name="name"
                    defaultValue={editor.value.name}
                    maxLength={128}
                    required
                    placeholder={routine ? "Morning briefing" : "Weekly review"}
                  />
                )}
              </FormField>
              {(validation || error) && (
                <p role="alert" className="error">
                  {validation || error}
                </p>
              )}
              <div className="automation-editor-actions">
                <Button
                  type="button"
                  variant="outline"
                  disabled={pending}
                  onClick={onClose}
                >
                  Cancel
                </Button>
                <Button type="submit" disabled={pending}>
                  {pending ? "Saving…" : "Save"}
                </Button>
              </div>
            </FieldGroup>
          </fieldset>
        </form>
      </DialogContent>
    </Dialog>
  );
}
