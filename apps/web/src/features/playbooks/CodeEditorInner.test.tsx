import { render, screen } from "@testing-library/react";
import { EditorView } from "@codemirror/view";
import { describe, expect, it, vi } from "vitest";
import CodeEditorInner from "@/features/playbooks/CodeEditorInner";

describe("CodeEditorInner", () => {
  it("uses the application surface and body size without forcing dark mode", () => {
    render(<CodeEditorInner value={"---\nname: Example"} onChange={vi.fn()} aria-label="Playbook YAML" />);
    const host = screen.getByLabelText("Playbook YAML");
    const editor = host.querySelector<HTMLElement>(".cm-editor")!;
    const view = EditorView.findFromDOM(editor)!;

    expect(host).toHaveClass("bg-background", "border-border");
    expect(view.state.facet(EditorView.darkTheme)).toBe(false);
    expect(getComputedStyle(editor).fontSize).toBe("14px");
    expect(view.state.doc.toString()).toBe("---\nname: Example");
  });

  it("preserves read-only mode and reconciles an external document update", () => {
    const onChange = vi.fn();
    const { rerender } = render(<CodeEditorInner value="name: First" onChange={onChange} readOnly aria-label="Playbook YAML" />);
    rerender(<CodeEditorInner value="name: Second" onChange={onChange} readOnly aria-label="Playbook YAML" />);
    const editor = screen.getByLabelText("Playbook YAML").querySelector<HTMLElement>(".cm-editor")!;
    const view = EditorView.findFromDOM(editor)!;

    expect(view.state.readOnly).toBe(true);
    expect(view.state.doc.toString()).toBe("name: Second");
    expect(onChange).not.toHaveBeenCalled();
  });
});
