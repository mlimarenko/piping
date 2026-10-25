import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import TemplateSortableList, { type TemplateSortableListLabels, type TemplateSortableListProps } from "./TemplateSortableList";

const labels: TemplateSortableListLabels = {
  handle: (name) => `Перетащить ${name}`,
  moveUp: (name) => `Выше: ${name}`,
  moveDown: (name) => `Ниже: ${name}`,
  position: (position, total) => `Позиция ${position} из ${total}`,
  instructions: "Пробел — взять, стрелки — переместить, Escape — отменить.",
  roleDescription: "Перемещаемый элемент",
  pickedUp: (name, position, total) => `Взят ${name}: ${position} из ${total}`,
  moved: (name, position, total) => `Перемещён ${name}: ${position} из ${total}`,
  dropped: (name, position, total) => `Размещён ${name}: ${position} из ${total}`,
  cancelled: (name) => `Отменено: ${name}`,
};
const items = Object.freeze([
  Object.freeze({ id: "stable-a", label: "Сайт" }),
  Object.freeze({ id: "stable-b", label: "Устройство" }),
  Object.freeze({ id: "stable-c", label: "Адрес" }),
]);

function list(onMove = vi.fn(), disabled = false, rows: TemplateSortableListProps["items"] = items) {
  return (
    <TemplateSortableList items={rows} disabled={disabled} onMove={onMove} ariaLabel="Шаги" labels={labels}>
      {(index, { dragHandle, moveControls }) => (
        <div>
          {dragHandle}
          <button type="button">{rows[index]?.label}</button>
          {moveControls}
          <input aria-label={`Значение ${rows[index]?.id}`} defaultValue={rows[index]?.label} />
        </div>
      )}
    </TemplateSortableList>
  );
}

describe("TemplateSortableList", () => {
  it("names the list and exposes localized handle instructions and position", () => {
    render(list());
    const rows = within(screen.getByRole("list", { name: "Шаги" })).getAllByRole("listitem");
    expect(rows).toHaveLength(3);
    expect(rows[1]).toHaveAttribute("aria-posinset", "2");
    expect(rows[1]).toHaveAttribute("aria-setsize", "3");
    const handle = screen.getByRole("button", { name: "Перетащить Устройство" });
    expect(handle).toHaveAccessibleDescription(`Позиция 2 из 3 ${labels.instructions}`);
    expect(handle).toHaveAttribute("aria-roledescription", labels.roleDescription);
    expect(handle.querySelector("button")).toBeNull();
    expect(screen.getByRole("button", { name: "Устройство" })).toBeEnabled();
  });

  it("requests zero-based adjacent moves and announces the target without mutating items", () => {
    const onMove = vi.fn(() => true);
    render(list(onMove));
    fireEvent.click(screen.getByRole("button", { name: "Выше: Устройство" }));
    expect(onMove).toHaveBeenLastCalledWith(1, 0);
    expect(screen.getByRole("status", { name: "Шаги" })).toHaveTextContent("Перемещён Устройство: 1 из 3");
    fireEvent.click(screen.getByRole("button", { name: "Ниже: Устройство" }));
    expect(onMove).toHaveBeenLastCalledWith(1, 2);
    expect(onMove).toHaveBeenCalledTimes(2);
    expect(screen.getAllByRole("textbox").map((input) => (input as HTMLInputElement).value))
      .toEqual(["Сайт", "Устройство", "Адрес"]);
  });

  it("announces cancellation rather than success when the parent rejects a move", () => {
    const onMove = vi.fn(() => false);
    render(list(onMove));
    fireEvent.click(screen.getByRole("button", { name: "Выше: Устройство" }));
    expect(onMove).toHaveBeenCalledExactlyOnceWith(1, 0);
    expect(screen.getByRole("status", { name: "Шаги" })).toHaveTextContent("Отменено: Устройство");
    expect(screen.queryByText(/^Перемещён/)).not.toBeInTheDocument();
    expect(screen.queryByText(/^Размещён/)).not.toBeInTheDocument();
    expect(screen.getAllByRole("textbox").map((input) => (input as HTMLInputElement).value))
      .toEqual(["Сайт", "Устройство", "Адрес"]);
  });

  it("disables only the impossible boundary moves", () => {
    const onMove = vi.fn();
    render(list(onMove));
    const firstUp = screen.getByRole("button", { name: "Выше: Сайт" });
    const lastDown = screen.getByRole("button", { name: "Ниже: Адрес" });
    expect(firstUp).toBeDisabled();
    expect(lastDown).toBeDisabled();
    fireEvent.click(firstUp);
    fireEvent.click(lastDown);
    expect(onMove).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Ниже: Сайт" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Выше: Адрес" })).toBeEnabled();
  });

  it("disables every sorting control for read-only or saving state", () => {
    const onMove = vi.fn();
    render(list(onMove, true));
    for (const button of screen.getAllByRole("button", { name: /^(Перетащить|Выше:|Ниже:)/ })) {
      expect(button).toBeDisabled();
      fireEvent.click(button);
      fireEvent.keyDown(button, { key: " ", code: "Space" });
    }
    expect(onMove).not.toHaveBeenCalled();
  });

  it("preserves row DOM and field state by stable ID when reordered or renamed", () => {
    const { rerender } = render(list());
    const field = screen.getByRole("textbox", { name: "Значение stable-b" });
    fireEvent.change(field, { target: { value: "Несохранённое значение" } });
    const reordered = [items[2]!, { id: "stable-b", label: "Переименовано" }, items[0]!];
    rerender(list(vi.fn(), false, reordered));
    expect(screen.getByRole("textbox", { name: "Значение stable-b" })).toBe(field);
    expect(field).toHaveValue("Несохранённое значение");
    expect(screen.getByRole("button", { name: "Перетащить Переименовано" })).toBeEnabled();
  });

  it("handles empty and single-item lists without possible moves", () => {
    const { rerender } = render(list(vi.fn(), false, []));
    expect(screen.queryByRole("listitem")).not.toBeInTheDocument();
    rerender(list(vi.fn(), false, [items[0]!]));
    expect(screen.getByRole("button", { name: "Выше: Сайт" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Ниже: Сайт" })).toBeDisabled();
  });
});
