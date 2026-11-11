import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { EditorView } from "@codemirror/view";
import { undo } from "@codemirror/commands";
import { forceLinting, forEachDiagnostic } from "@codemirror/lint";
import { replaceAll, SearchQuery, setSearchQuery } from "@codemirror/search";
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

  it("keeps undo and selection isolated per document", () => {
    const onChange = vi.fn();
    const { rerender } = render(<CodeEditorInner value="one" documentId="a" onChange={onChange} />);
    const view = EditorView.findFromDOM(document.querySelector(".cm-editor")!)!;
    act(() => view.dispatch({ changes: { from: 3, insert: "!" }, selection: { anchor: 2 } }));
    rerender(<CodeEditorInner value="two" documentId="b" onChange={onChange} />);
    act(() => view.dispatch({ changes: { from: 3, insert: "?" } }));
    rerender(<CodeEditorInner value="one!" documentId="a" onChange={onChange} />);
    expect(view.state.selection.main.anchor).toBe(2);
    act(() => { expect(undo(view)).toBe(true); });
    expect(view.state.doc.toString()).toBe("one");
    rerender(<CodeEditorInner value="two?" documentId="b" onChange={onChange} />);
    act(() => { expect(undo(view)).toBe(true); });
    expect(view.state.doc.toString()).toBe("two");
  });

  it("reconfigures canvas height without replacing the editor or losing document history", () => {
    const onChange = vi.fn();
    const { rerender } = render(<CodeEditorInner value="one" documentId="a" height="320px" minHeight="0" onChange={onChange} aria-label="Project source" />);
    const view = EditorView.findFromDOM(screen.getByLabelText("Project source").querySelector(".cm-editor")!)!;
    expect(getComputedStyle(view.dom).height).toBe("320px");
    act(() => view.dispatch({ changes: { from: 3, insert: "!" }, selection: { anchor: 2 } }));
    rerender(<CodeEditorInner value="one!" documentId="a" height="640px" minHeight="0" onChange={onChange} aria-label="Project source" />);
    expect(EditorView.findFromDOM(screen.getByLabelText("Project source").querySelector(".cm-editor")!)).toBe(view);
    expect(getComputedStyle(view.dom).height).toBe("640px");
    expect(view.state.selection.main.anchor).toBe(2);
    act(() => { expect(undo(view)).toBe(true); });
    expect(view.state.doc.toString()).toBe("one");
    rerender(<CodeEditorInner value="one" documentId="a" onChange={onChange} aria-label="Project source" />);
    expect(getComputedStyle(view.dom).height).toBe("auto");
    expect(view.state.doc.toString()).toBe("one");
  });

  it("reports YAML syntax errors but not custom tags or arbitrary text", async () => {
    const { rerender } = render(<CodeEditorInner value="key: [broken" fileName="a.yml" onChange={vi.fn()} />);
    const view = EditorView.findFromDOM(document.querySelector(".cm-editor")!)!;
    const diagnostics = () => {
      forceLinting(view);
      const messages: string[] = [];
      forEachDiagnostic(view.state, (diagnostic) => messages.push(diagnostic.message));
      return messages;
    };
    await waitFor(() => expect(diagnostics().length).toBeGreaterThan(0));
    rerender(<CodeEditorInner value={'secret: !vault |\n  encrypted\nother: !custom value'} fileName="a.yml" onChange={vi.fn()} />);
    await waitFor(() => expect(diagnostics()).toEqual([]));
    rerender(<CodeEditorInner value="key: [broken" fileName="notes.txt" onChange={vi.fn()} />);
    expect(diagnostics()).toEqual([]);
  });

  it("provides search and optional save shortcuts and disables editing in read-only mode", () => {
    const onSave = vi.fn();
    const { rerender } = render(<CodeEditorInner value="alpha beta" onChange={vi.fn()} onSave={onSave} />);
    const view = EditorView.findFromDOM(document.querySelector(".cm-editor")!)!;
    fireEvent.keyDown(view.contentDOM, { key: "s", ctrlKey: true });
    expect(onSave).toHaveBeenCalledOnce();
    fireEvent.keyDown(view.contentDOM, { key: "f", ctrlKey: true });
    expect(document.querySelector(".cm-search")).not.toBeNull();
    act(() => {
      view.dispatch({ effects: setSearchQuery.of(new SearchQuery({ search: "alpha", replace: "gamma" })) });
      expect(replaceAll(view)).toBe(true);
    });
    expect(view.state.doc.toString()).toBe("gamma beta");
    rerender(<CodeEditorInner value="alpha beta" onChange={vi.fn()} readOnly />);
    expect(view.state.readOnly).toBe(true);
    expect(view.contentDOM.getAttribute("contenteditable")).toBe("false");
    act(() => { expect(replaceAll(view)).toBe(false); });
    expect(view.state.doc.toString()).toBe("alpha beta");
  });
});
