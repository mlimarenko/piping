import { useId, useState, type ReactNode } from "react";
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  pointerWithin,
  useSensor,
  useSensors,
  type Announcements,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { ArrowDown, ArrowUp, GripVertical } from "lucide-react";

import { Button } from "@/shared/ui-primitives/button";

export type TemplateSortableListLabels = {
  handle: (label: string) => string;
  moveUp: (label: string) => string;
  moveDown: (label: string) => string;
  position: (position: number, total: number) => string;
  instructions: string;
  roleDescription: string;
  pickedUp: (label: string, position: number, total: number) => string;
  moved: (label: string, position: number, total: number) => string;
  dropped: (label: string, position: number, total: number) => string;
  cancelled: (label: string) => string;
};

type SortableItem = { id: string; label: string };
type SortableControls = { dragHandle: ReactNode; moveControls: ReactNode };

export type TemplateSortableListProps = {
  items: readonly SortableItem[];
  disabled: boolean;
  onMove: (from: number, to: number) => boolean;
  ariaLabel: string;
  labels: TemplateSortableListLabels;
  children: (index: number, controls: SortableControls) => ReactNode;
};

function SortableRow({
  item, index, total, disabled, labels, move, children,
}: {
  item: SortableItem;
  index: number;
  total: number;
  disabled: boolean;
  labels: TemplateSortableListLabels;
  move: (from: number, to: number) => void;
  children: TemplateSortableListProps["children"];
}) {
  const positionId = useId();
  const { attributes, listeners, setNodeRef, setActivatorNodeRef, transform, transition, isDragging } =
    useSortable({ id: item.id, disabled });

  return (
    <div
      ref={setNodeRef}
      role="listitem"
      aria-posinset={index + 1}
      aria-setsize={total}
      className="relative min-w-0"
      style={{ transform: CSS.Transform.toString(transform), transition, zIndex: isDragging ? 1 : undefined }}
    >
      <span id={positionId} className="sr-only">{labels.position(index + 1, total)}</span>
      {children(index, {
        dragHandle: (
          <Button
            {...attributes}
            {...listeners}
            ref={setActivatorNodeRef}
            type="button"
            variant="ghost"
            size="icon-sm"
            disabled={disabled}
            aria-label={labels.handle(item.label)}
            aria-roledescription={labels.roleDescription}
            aria-describedby={`${positionId} ${attributes["aria-describedby"]}`}
            className="touch-none cursor-grab text-muted-foreground active:cursor-grabbing"
          >
            <GripVertical className="size-4" aria-hidden="true" />
          </Button>
        ),
        moveControls: (
          <span className="inline-flex items-center gap-1">
            <Button type="button" variant="ghost" size="icon-sm" disabled={disabled || index === 0}
              aria-label={labels.moveUp(item.label)} aria-describedby={positionId}
              onClick={() => move(index, index - 1)}>
              <ArrowUp className="size-4" aria-hidden="true" />
            </Button>
            <Button type="button" variant="ghost" size="icon-sm" disabled={disabled || index === total - 1}
              aria-label={labels.moveDown(item.label)} aria-describedby={positionId}
              onClick={() => move(index, index + 1)}>
              <ArrowDown className="size-4" aria-hidden="true" />
            </Button>
          </span>
        ),
      })}
    </div>
  );
}

export default function TemplateSortableList({ items, disabled, onMove, ariaLabel, labels, children }: TemplateSortableListProps) {
  const [announcement, setAnnouncement] = useState("");
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  const move = (from: number, to: number, describeMovement = labels.moved) => {
    const item = items[from];
    if (!item) return;
    if (disabled || to < 0 || to >= items.length || from === to) {
      setAnnouncement(labels.cancelled(item.label));
      return;
    }
    const accepted = onMove(from, to);
    setAnnouncement(accepted
      ? describeMovement(item.label, to + 1, items.length)
      : labels.cancelled(item.label));
  };
  const describe = (id: string | number, describeItem: (label: string, position: number, total: number) => string) => {
    const index = items.findIndex((item) => item.id === id);
    const item = items[index];
    return item ? describeItem(item.label, index + 1, items.length) : undefined;
  };
  const announcements: Announcements = {
    onDragStart: ({ active }) => describe(active.id, labels.pickedUp),
    onDragOver: ({ active, over }) => {
      const item = items.find((item) => item.id === active.id);
      return item && over ? describe(over.id, (_, position, total) => labels.moved(item.label, position, total)) : undefined;
    },
    // Completion is announced by move after the parent accepts or rejects it.
    onDragEnd: () => undefined,
    onDragCancel: ({ active }) => describe(active.id, (label) => labels.cancelled(label)),
  };
  const handleDragEnd = ({ active, over }: DragEndEvent) => {
    const from = items.findIndex((item) => item.id === active.id);
    const to = over ? items.findIndex((item) => item.id === over.id) : -1;
    move(from, to, labels.dropped);
  };

  return (
    <DndContext sensors={sensors}
      collisionDetection={(args) => args.pointerCoordinates ? pointerWithin(args) : closestCenter(args)}
      accessibility={{ screenReaderInstructions: { draggable: labels.instructions }, announcements }}
      onDragEnd={handleDragEnd}>
      <SortableContext items={items.map((item) => item.id)} strategy={verticalListSortingStrategy}>
        <div role="list" aria-label={ariaLabel} className="flex min-w-0 flex-col gap-2">
          {items.map((item, index) => (
            <SortableRow key={item.id} item={item} index={index} total={items.length}
              disabled={disabled} labels={labels} move={move}>
              {children}
            </SortableRow>
          ))}
        </div>
      </SortableContext>
      <span role="status" aria-label={ariaLabel} aria-live="polite" aria-atomic="true" className="sr-only">{announcement}</span>
    </DndContext>
  );
}
