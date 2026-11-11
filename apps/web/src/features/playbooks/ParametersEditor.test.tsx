import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import "@/shared/i18n";
import ParametersEditor from "./ParametersEditor";
import { parseParameterValues } from "./varsSchema";

const schema = { type: "object", properties: {
  name: { type: "string", title: "Service name" },
  count: { type: "integer", title: "Retries", default: 3 },
  enabled: { type: "boolean", title: "Enabled" },
  hosts: { type: "array", title: "Hosts" },
  settings: { type: "object", title: "Settings" },
}, required: ["name"] };

function Harness() {
  const [value, setValue] = useState('{"name":"first","enabled":false,"hosts":[],"settings":{}}');
  const [valid, setValid] = useState(false);
  return <><ParametersEditor schema={schema} value={value} onChange={setValue} onValidityChange={setValid} /><button disabled={!valid}>Save parameters</button><output data-testid="values">{value}</output></>;
}

describe("parameter editor", () => {
  it("edits typed values, YAML and JSON through the same object", () => {
    render(<Harness />);
    fireEvent.change(screen.getByLabelText("Service name *"), { target: { value: "" } });
    fireEvent.change(screen.getByLabelText("Retries"), { target: { value: "0" } });
    fireEvent.click(screen.getByRole("button", { name: "YAML" }));
    expect(parseParameterValues((screen.getByRole("textbox", { name: "Parameters YAML" }) as HTMLTextAreaElement).value, "yaml")).toEqual({ name: "", enabled: false, hosts: [], settings: {}, count: 0 });
    fireEvent.change(screen.getByRole("textbox", { name: "Parameters YAML" }), { target: { value: 'name: changed\ncount: 0\nenabled: false\nhosts: [one, two]\nsettings: {port: 22}' } });
    fireEvent.click(screen.getByRole("button", { name: "Fields" }));
    expect(screen.getByLabelText("Service name *")).toHaveValue("changed");
    expect(screen.getByLabelText("Retries")).toHaveValue(0);
    fireEvent.click(screen.getByRole("button", { name: "JSON" }));
    expect(JSON.parse((screen.getByRole("textbox", { name: "Parameters JSON" }) as HTMLTextAreaElement).value)).toEqual({ name: "changed", count: 0, enabled: false, hosts: ["one", "two"], settings: { port: 22 } });
  });

  it("blocks save while a raw or nested draft is incomplete, without discarding it", () => {
    render(<Harness />);
    fireEvent.change(screen.getByLabelText("Hosts"), { target: { value: '["one",' } });
    expect(screen.getByRole("button", { name: "Save parameters" })).toBeDisabled();
    expect(screen.getByLabelText("Hosts")).toHaveValue('["one",');
    fireEvent.change(screen.getByLabelText("Hosts"), { target: { value: '["one"]' } });
    expect(screen.getByRole("button", { name: "Save parameters" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "YAML" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Parameters YAML" }), { target: { value: 'name: a\nname: b' } });
    expect(screen.getByRole("button", { name: "Save parameters" })).toBeDisabled();
    expect(JSON.parse(screen.getByTestId("values").textContent!)).toHaveProperty("name", "first");
    expect(screen.getByRole("button", { name: "Fields" })).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox", { name: "Parameters YAML" }), { target: { value: 'name: b' } });
    expect(screen.getByRole("button", { name: "Save parameters" })).toBeEnabled();
  });
});
