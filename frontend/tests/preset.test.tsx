import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSeparator,
  FieldSet,
  FieldTitle,
} from "../src/components/ui/field";
import {
  NativeSelect,
  NativeSelectOptGroup,
  NativeSelectOption,
} from "../src/components/ui/native-select";
import { Separator } from "../src/components/ui/separator";

describe("Mira form semantics", () => {
  it("groups related controls with a legend and supports grouped keyboard selection", async () => {
    render(
      <FieldSet>
        <FieldLegend>Schedule</FieldLegend>
        <FieldGroup>
          <Field orientation="horizontal">
            <FieldLabel htmlFor="frequency">Repeat</FieldLabel>
            <FieldContent>
              <NativeSelect id="frequency" aria-describedby="repeat-help">
                <NativeSelectOptGroup label="Regular">
                  <NativeSelectOption value="hour">Hourly</NativeSelectOption>
                  <NativeSelectOption value="day">Daily</NativeSelectOption>
                </NativeSelectOptGroup>
              </NativeSelect>
              <FieldDescription id="repeat-help">
                Elapsed time between runs
              </FieldDescription>
            </FieldContent>
          </Field>
          <FieldSeparator>Advanced</FieldSeparator>
          <Field orientation="responsive">
            <FieldTitle>Run options</FieldTitle>
            <FieldLegend variant="label">Optional</FieldLegend>
          </Field>
          <FieldSeparator />
        </FieldGroup>
      </FieldSet>,
    );
    expect(screen.getByRole("group", { name: "Schedule" })).toBeVisible();
    expect(
      screen.getByRole("combobox", { name: "Repeat" }),
    ).toHaveAccessibleDescription("Elapsed time between runs");
    await userEvent
      .setup()
      .selectOptions(screen.getByLabelText("Repeat"), "day");
    expect(screen.getByLabelText("Repeat")).toHaveValue("day");
    expect(screen.getByText("Advanced")).toBeVisible();
    expect(screen.getByText("Run options")).toBeVisible();
    render(<Separator orientation="vertical" decorative={false} />);
    expect(screen.getByRole("separator")).toHaveAttribute(
      "aria-orientation",
      "vertical",
    );
  });
  it("prefers a custom error and deduplicates repeated validation messages", () => {
    const { rerender } = render(
      <FieldError errors={[{ message: "Ignored" }]}>
        Use a registered workspace
      </FieldError>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Use a registered workspace",
    );
    rerender(
      <FieldError
        errors={[{ message: "Required" }, { message: "Required" }]}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("Required");
    expect(screen.queryByRole("list")).toBeNull();
    rerender(
      <FieldError
        errors={[
          { message: "Required" },
          undefined,
          { message: "Invalid time" },
          { message: "Required" },
        ]}
      />,
    );
    expect(
      screen.getAllByRole("listitem").map((item) => item.textContent),
    ).toEqual(["Required", "Invalid time"]);
    rerender(<FieldError errors={[{}, { message: "Invalid time" }]} />);
    expect(screen.getAllByRole("listitem")).toHaveLength(1);
  });
  it.each([undefined, [], [undefined], [{}]])(
    "omits empty validation alerts (%j)",
    (errors) => {
      render(<FieldError errors={errors} />);
      expect(screen.queryByRole("alert")).toBeNull();
    },
  );
});
