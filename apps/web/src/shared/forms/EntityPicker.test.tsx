import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import EntityPicker from "@/shared/forms/EntityPicker";
import en from "@/shared/i18n/locales/en";

const options = Array.from({ length: 12 }, (_, index) => ({
  id: `iface-${index + 1}`,
  name: "mgmt0",
  subtitle: `virtual · #${String(index + 1).padStart(6, "0")}`,
  searchText: `mgmt0 ${index + 1}`,
}));

describe("EntityPicker", () => {
  it("explains the entity in the opened picker and describes its search input", () => {
    render(<EntityPicker entity="Interfaces" value="" onChange={() => {}} options={options} label="Interface" />);
    const text = en.inventory.resources.hints.entities.Interfaces;
    expect(screen.queryByText(text)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("combobox", { name: "Interface" }));
    const description = screen.getByText(text);
    expect(screen.getByPlaceholderText((text) => text.includes("Interface"))).toHaveAttribute("aria-describedby", description.id);
  });

  it("renders its searchable list inline so dialog wheel scrolling can reach it", () => {
    render(
      <div data-testid="dialog-body">
        <EntityPicker
          entity="Interfaces"
          value=""
          onChange={() => {}}
          options={options}
          label="Interface"
          onRequestCreate={() => {}}
        />
      </div>,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Interface" }));

    const panel = document.querySelector("[data-picker-panel]");
    const list = document.querySelector("[cmdk-list]");
    expect(panel).toBeInTheDocument();
    expect(panel?.closest("[data-picker-root]")).toBeInTheDocument();
    expect(panel?.closest("[data-entity-picker-root]")).toBeInTheDocument();
    expect(panel?.closest("[data-testid='dialog-body']")).toBeInTheDocument();
    expect(list).toHaveClass("overflow-y-auto", "overscroll-contain");
    expect(screen.getByPlaceholderText((text) => text.includes("Interface"))).toHaveClass("px-1.5");
  });

  it("submits option ids and closes after selection", () => {
    const onChange = vi.fn();
    render(
      <EntityPicker
        entity="Interfaces"
        value=""
        onChange={onChange}
        options={options.slice(0, 2)}
        label="Interface"
        onRequestCreate={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Interface" }));
    fireEvent.click(screen.getAllByRole("option")[0]!);

    expect(onChange).toHaveBeenCalledWith("iface-1");
    expect(document.querySelector("[data-picker-panel]")).not.toBeInTheDocument();
  });

  it("closes when the user clicks outside", () => {
    render(
      <>
        <EntityPicker
          entity="Interfaces"
          value=""
          onChange={() => {}}
          options={options}
          label="Interface"
          onRequestCreate={() => {}}
        />
        <button type="button">outside</button>
      </>,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Interface" }));
    expect(document.querySelector("[data-picker-panel]")).toBeInTheDocument();

    fireEvent.pointerDown(screen.getByRole("button", { name: "outside" }));
    expect(document.querySelector("[data-picker-panel]")).not.toBeInTheDocument();
  });

  it("captures Escape while open so parent dialogs do not handle it", () => {
    const parentEscape = vi.fn();
    window.addEventListener("keydown", parentEscape);
    render(
      <EntityPicker
        entity="Interfaces"
        value=""
        onChange={() => {}}
        options={options}
        label="Interface"
        onRequestCreate={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Interface" }));
    expect(document.querySelector("[data-picker-panel]")).toBeInTheDocument();

    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    });
    act(() => {
      window.dispatchEvent(event);
    });

    expect(event.defaultPrevented).toBe(true);
    expect(parentEscape).not.toHaveBeenCalled();
    expect(document.querySelector("[data-picker-panel]")).not.toBeInTheDocument();
    window.removeEventListener("keydown", parentEscape);
  });
});
