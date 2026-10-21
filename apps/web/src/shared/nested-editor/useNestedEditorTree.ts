import { useCallback, useEffect, useMemo, useRef, useState } from "react";

/**
 * Headless core for the unified nested-CRUD editor.
 *
 * Owns the entire edit tree for one root entity and orchestrates a partial-
 * failure-aware save across an arbitrary number of master-detail levels
 * (Device → Interfaces → IP addresses, VM → VM Interface → IP, …). It is pure
 * state + orchestration: it imports NO feature code and talks to the backend
 * only through the {@link RelationDescriptor} CRUD callbacks the host wires in.
 *
 * Save contract (the spine):
 *  - creates + updates run TOP-DOWN: a parent resolves its real id before any
 *    child create injects that id as the FK.
 *  - deletes run BOTTOM-UP: a grandchild is removed before its parent so the
 *    backend's orphan check never trips (there is no cascade delete).
 *  - temp ids resolve depth-first: after a parent create succeeds the new real
 *    id is written back into the node, then each child's `toPayload(draft,
 *    parentRealId)` injects the real FK right before that child's POST.
 *  - partial failure keeps committed nodes real + clean, marks the failed node
 *    `failed` with the server message, and marks its not-yet-created descendants
 *    `blocked`. Re-running save is skip-clean idempotent: only non-`clean` nodes
 *    re-issue calls.
 */

/** A client-generated id, stable for a node's whole lifetime (survives the
 *  temp→real swap; React keys and parent links use this, never the real id). */
export type LocalId = string;

export type NodeStatus =
  | "new"
  | "clean"
  | "dirty"
  | "deleted"
  | "saving"
  | "failed"
  | "blocked";

/** One editable record in the tree. `draft` is the working copy; `baseline` is
 *  the last-persisted snapshot used to diff dirty state. A draft is an opaque
 *  string-map (the descriptor's row form owns its shape). */
export interface NestedNode {
  localId: LocalId;
  /** Server id once persisted; absent for unsaved `new` rows. */
  realId?: string;
  /** Which relation level this node belongs to (descriptor key). */
  specKey: string;
  /** The owning parent node, or undefined at the root relation level. */
  parentLocalId?: LocalId;
  draft: Record<string, string>;
  /** Last-persisted snapshot (empty for a `new` row). */
  baseline: Record<string, string>;
  status: NodeStatus;
  /** Inline server message for a `failed` node. */
  error?: string;
  /** True once this node's child levels have been lazily fetched. */
  childrenLoaded: boolean;
  /** True when the server reported more rows than were loaded at this node's
   *  own level (its siblings were capped) — blocks deletes on that level. */
  truncated: boolean;
  /** Server-reported total at this node's level when truncated, so the UI can
   *  render "showing N of M". 0 when the level is fully loaded. */
  levelTotal: number;
}

/** Minimal CRUD + form contract the core depends on. The concrete descriptor is
 *  compiled from inventory specs by `descriptorFromSpec.ts`; the core never sees
 *  feature types. */
export interface RelationDescriptor<TChild extends { id: string } = { id: string }> {
  /** Stable relation key (also the node `specKey`). */
  key: string;
  /** i18n key for the section title. */
  title: string;
  /** FK field name the child create needs (`device_id` / `interface_id` / …),
   *  carried for documentation; injection itself is centralized in `toPayload`. */
  parentFkField: string;
  /** Lazily fetch this relation's rows for one parent real id. */
  listByParent: (
    parentRealId: string,
    page: { limit: number; offset: number },
  ) => Promise<{ rows: TChild[]; total: number; loaded: number }>;
  /** Create one row; returns the new real id plus the raw persisted record. */
  create: (payload: Record<string, unknown>) => Promise<{ id: string; raw: TChild }>;
  /** Update one row; returns the raw persisted record. */
  update: (realId: string, patch: Record<string, unknown>) => Promise<TChild>;
  /** Delete one row. */
  remove: (realId: string) => Promise<void>;
  /** Optional bulk-create; the core uses sequential `create` for exact per-row
   *  attribution, so this stays unused by default. */
  bulkCreate?: (payloads: Record<string, unknown>[]) => Promise<{ id: string; raw: TChild }[]>;
  /** Build a fresh empty draft for a new row at this level. */
  buildDraft: (row?: TChild) => Record<string, string>;
  /** Project a raw fetched row into the editor draft shape. */
  toDraft: (row: TChild) => Record<string, string>;
  /** Centralized temp-id→real-FK injection: produce the create/update payload,
   *  stamping the parent's real id onto the FK field. `parentRealId` is omitted
   *  at the root level. */
  toPayload: (draft: Record<string, string>, parentRealId?: string) => Record<string, unknown>;
  /** Validate one draft; returns a field→message map (empty = valid). */
  validate: (draft: Record<string, string>) => Record<string, string>;
  /** Page size for `listByParent` (clamped to the server cap by the host). */
  pageLimit: number;
  /** The single nested child relation, if this level has one. */
  childRelation?: AnyRelationDescriptor;
}

/**
 * The core treats descriptors opaquely — it only ever feeds a child relation its
 * OWN rows — so the `TChild` generic carries no cross-level constraint here. This
 * alias erases it at the seam, letting a concrete `RelationDescriptor<Resource>`
 * be stored/passed without tripping function-parameter contravariance.
 */
export type AnyRelationDescriptor = RelationDescriptor<{ id: string }>;

/** Per-node outcome surfaced from {@link useNestedEditorTree.save}. */
export interface SaveResult {
  ok: boolean;
  /** Local ids of nodes that ended this run persisted + clean. */
  persisted: LocalId[];
  /** Nodes whose call failed this run. */
  failed: { relKey: string; nodeId: LocalId; message: string }[];
  /** Nodes that could not even be attempted because an ancestor failed. */
  blocked: LocalId[];
}

/** Public shape returned by the hook. */
export interface NestedEditorTree {
  /** Every node, keyed by localId. */
  nodes: Record<LocalId, NestedNode>;
  /** Direct child node ids of a parent (or root ids when `parentLocalId` is
   *  undefined), for a given relation key, excluding hard-removed-new nodes. */
  rowsByRelation: (relKey: string, parentLocalId?: LocalId) => LocalId[];
  /** Which node ids are currently expanded (children visible). */
  expanded: Set<LocalId>;
  isExpanded: (localId: LocalId) => boolean;
  /** Toggle a node open/closed; opening lazily fetches its child relation. */
  toggleExpand: (localId: LocalId) => void;
  /** Append a new empty row at a relation level under a parent (or at root). */
  addRow: (relKey: string, parentLocalId?: LocalId) => LocalId;
  /** Patch one draft field and re-diff dirty state. */
  patchDraft: (localId: LocalId, field: string, value: string) => void;
  /** Mark a node (and its descendants) deleted; a never-persisted `new` node is
   *  dropped outright. */
  markDeleted: (localId: LocalId) => void;
  /** Re-arm a `failed`/`blocked` node so the next save retries it: a never-saved
   *  node returns to `new`, a previously-persisted one to `dirty`. */
  retryNode: (localId: LocalId) => void;
  /** Whole-tree dirty flag (any new/dirty/deleted/failed node). */
  isDirty: boolean;
  /** Field-level validation errors for a node, computed on demand. */
  validationErrors: (localId: LocalId) => Record<string, string>;
  /** True when any node has unresolved validation errors. */
  hasInvalid: boolean;
  /** Run a partial-failure-aware save and return the per-node outcome. */
  save: () => Promise<SaveResult>;
  /** Lazily fetch one top-level relation's rows (idempotent per relation key).
   *  The host calls this once per root relation when the panel mounts. */
  loadRootRelation: (relKey: string) => void;
}

interface DescriptorIndex {
  byKey: Record<string, AnyRelationDescriptor>;
  /** Relation keys whose parent is the root entity (top-level levels). */
  rootKeys: string[];
}

/** Flatten a descriptor tree into a key lookup. The root entity is implicit:
 *  the descriptors passed at the top are the root's relation levels. */
function indexDescriptors(roots: AnyRelationDescriptor[]): DescriptorIndex {
  const byKey: Record<string, AnyRelationDescriptor> = {};
  const walk = (d: AnyRelationDescriptor) => {
    byKey[d.key] = d;
    if (d.childRelation) walk(d.childRelation);
  };
  roots.forEach(walk);
  return { byKey, rootKeys: roots.map((d) => d.key) };
}

let localIdCounter = 0;
/** Monotonic client id. Not crypto: a tree lives in one component instance, and
 *  ids only need to be unique within it (and never collide with a server uuid,
 *  which the `local:` prefix guarantees). */
function nextLocalId(): LocalId {
  localIdCounter += 1;
  return `local:${localIdCounter}:${Math.random().toString(36).slice(2, 8)}`;
}

/** Shallow draft equality over the union of keys (drafts are flat string maps). */
function draftsEqual(a: Record<string, string>, b: Record<string, string>): boolean {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const k of keys) {
    if ((a[k] ?? "") !== (b[k] ?? "")) return false;
  }
  return true;
}

export interface UseNestedEditorTreeOptions {
  /** The root entity's real id (a Device id, VM id, …). Required to fetch and
   *  create the top-level relation rows. */
  rootRealId: string;
  /** The root's relation levels (each may carry a single nested childRelation). */
  relations: AnyRelationDescriptor[];
  /** Hard cap on a single child page (server MAX). Clamps `pageLimit`. */
  maxPageSize?: number;
}

/**
 * The headless nested-editor state machine. See the module doc for the save
 * contract. All returned callbacks are stable across renders.
 */
export function useNestedEditorTree(
  options: UseNestedEditorTreeOptions,
): NestedEditorTree {
  const { rootRealId, relations, maxPageSize = 1000 } = options;
  const index = useMemo(() => indexDescriptors(relations), [relations]);
  // The descriptor index / root id / page cap are kept in refs so the stable
  // callbacks below always read the current values without being recreated on
  // every render. Refs are synced in an effect (never during render); every
  // consumer runs in an event handler or effect, after the sync has landed.
  const indexRef = useRef(index);
  const rootIdRef = useRef(rootRealId);
  const maxPageRef = useRef(maxPageSize);
  useEffect(() => {
    indexRef.current = index;
    rootIdRef.current = rootRealId;
    maxPageRef.current = maxPageSize;
  }, [index, rootRealId, maxPageSize]);

  const [nodes, setNodes] = useState<Record<LocalId, NestedNode>>({});
  const [expanded, setExpanded] = useState<Set<LocalId>>(new Set());
  /** Parents whose child level is mid-fetch — guards against double loads. */
  const loadingParents = useRef<Set<string>>(new Set());

  /** Merge a fetched page of rows into the tree under one parent (or at root). */
  const ingestRows = useCallback(
    (
      relKey: string,
      parentLocalId: LocalId | undefined,
      page: { rows: { id: string }[]; total: number; loaded: number },
    ) => {
      const desc = indexRef.current.byKey[relKey];
      if (!desc) return;
      const truncated = page.loaded < page.total;
      setNodes((prev) => {
        const next = { ...prev };
        for (const row of page.rows) {
          // Skip rows already represented (e.g. a re-expand): keep local edits.
          const exists = Object.values(next).some(
            (n) => n.specKey === relKey && n.realId === row.id && n.parentLocalId === parentLocalId,
          );
          if (exists) continue;
          const localId = nextLocalId();
          const draft = desc.toDraft(row);
          next[localId] = {
            localId,
            realId: row.id,
            specKey: relKey,
            parentLocalId,
            draft,
            baseline: { ...draft },
            status: "clean",
            childrenLoaded: false,
            truncated,
            levelTotal: truncated ? page.total : 0,
          };
        }
        // Stamp truncation + the level total onto every sibling at this level so
        // a delete guard / "N of M" label on any one of them reflects the
        // level-wide partial load.
        if (truncated) {
          for (const id of Object.keys(next)) {
            const n = next[id]!;
            if (n.specKey === relKey && n.parentLocalId === parentLocalId) {
              next[id] = { ...n, truncated: true, levelTotal: page.total };
            }
          }
        }
        return next;
      });
    },
    [],
  );

  /** Lazily fetch a node's child relation rows the first time it expands. */
  const ensureChildrenLoaded = useCallback((parent: NestedNode) => {
    const desc = indexRef.current.byKey[parent.specKey];
    const childDesc = desc?.childRelation;
    if (!childDesc || !parent.realId) return;
    if (parent.childrenLoaded) return;
    const guardKey = `${parent.localId}:${childDesc.key}`;
    if (loadingParents.current.has(guardKey)) return;
    loadingParents.current.add(guardKey);
    const limit = Math.min(childDesc.pageLimit, maxPageRef.current);
    void childDesc
      .listByParent(parent.realId, { limit, offset: 0 })
      .then((page) => {
        ingestRows(childDesc.key, parent.localId, page);
        setNodes((prev) => {
          const cur = prev[parent.localId];
          if (!cur) return prev;
          return { ...prev, [parent.localId]: { ...cur, childrenLoaded: true } };
        });
      })
      .finally(() => {
        loadingParents.current.delete(guardKey);
      });
  }, [ingestRows]);

  /** Load the top-level relation rows for one root relation key (lazy, on first
   *  expand of that section by the host, or eagerly via the section component). */
  const ensureRootLoaded = useRef<Set<string>>(new Set());
  const loadRootRelation = useCallback((relKey: string) => {
    if (ensureRootLoaded.current.has(relKey)) return;
    const desc = indexRef.current.byKey[relKey];
    if (!desc) return;
    ensureRootLoaded.current.add(relKey);
    const limit = Math.min(desc.pageLimit, maxPageRef.current);
    void desc
      .listByParent(rootIdRef.current, { limit, offset: 0 })
      .then((page) => ingestRows(relKey, undefined, page))
      .catch(() => {
        // Allow a retry on transient failure.
        ensureRootLoaded.current.delete(relKey);
      });
  }, [ingestRows]);

  const rowsByRelation = useCallback(
    (relKey: string, parentLocalId?: LocalId): LocalId[] =>
      Object.values(nodes)
        .filter((n) => n.specKey === relKey && n.parentLocalId === parentLocalId)
        .map((n) => n.localId),
    [nodes],
  );

  const isExpanded = useCallback((localId: LocalId) => expanded.has(localId), [expanded]);

  const toggleExpand = useCallback(
    (localId: LocalId) => {
      setExpanded((prev) => {
        const next = new Set(prev);
        if (next.has(localId)) {
          next.delete(localId);
        } else {
          next.add(localId);
        }
        return next;
      });
      setNodes((prev) => {
        const node = prev[localId];
        if (node) ensureChildrenLoaded(node);
        return prev;
      });
    },
    [ensureChildrenLoaded],
  );

  const addRow = useCallback((relKey: string, parentLocalId?: LocalId): LocalId => {
    const desc = indexRef.current.byKey[relKey];
    const localId = nextLocalId();
    const draft = desc ? desc.buildDraft() : {};
    setNodes((prev) => ({
      ...prev,
      [localId]: {
        localId,
        specKey: relKey,
        parentLocalId,
        draft,
        baseline: {},
        status: "new",
        childrenLoaded: true, // a brand-new row has no server children to fetch
        truncated: false,
        levelTotal: 0,
      },
    }));
    // A new row under a parent is most useful expanded so its own children (if
    // any) can be added immediately.
    if (parentLocalId) setExpanded((prev) => new Set(prev).add(localId));
    return localId;
  }, []);

  const patchDraft = useCallback((localId: LocalId, field: string, value: string) => {
    setNodes((prev) => {
      const node = prev[localId];
      if (!node) return prev;
      const draft = { ...node.draft, [field]: value };
      // A `new` node stays `new`; a previously-persisted node toggles
      // clean↔dirty against its baseline. A `failed` node returns to dirty so a
      // re-save retries it.
      let status: NodeStatus = node.status;
      if (node.status !== "new" && node.status !== "deleted") {
        status = draftsEqual(draft, node.baseline) ? "clean" : "dirty";
      }
      return { ...prev, [localId]: { ...node, draft, status, error: undefined } };
    });
  }, []);

  const markDeleted = useCallback((localId: LocalId) => {
    setNodes((prev) => {
      const node = prev[localId];
      if (!node) return prev;
      const next = { ...prev };
      // Collect the node + all descendants (by parent chain).
      const toRemove = new Set<LocalId>([localId]);
      let grew = true;
      while (grew) {
        grew = false;
        for (const n of Object.values(next)) {
          if (n.parentLocalId && toRemove.has(n.parentLocalId) && !toRemove.has(n.localId)) {
            toRemove.add(n.localId);
            grew = true;
          }
        }
      }
      for (const id of toRemove) {
        const n = next[id]!;
        if (!n.realId) {
          // Never persisted → drop outright (nothing to delete server-side).
          delete next[id];
        } else {
          next[id] = { ...n, status: "deleted", error: undefined };
        }
      }
      return next;
    });
  }, []);

  const retryNode = useCallback((localId: LocalId) => {
    setNodes((prev) => {
      const node = prev[localId];
      if (!node) return prev;
      if (node.status !== "failed" && node.status !== "blocked") return prev;
      const status: NodeStatus = node.realId ? "dirty" : "new";
      return { ...prev, [localId]: { ...node, status, error: undefined } };
    });
  }, []);

  const isDirty = useMemo(
    () =>
      Object.values(nodes).some(
        (n) =>
          n.status === "new" ||
          n.status === "dirty" ||
          n.status === "deleted" ||
          n.status === "failed" ||
          n.status === "blocked",
      ),
    [nodes],
  );

  // `validationErrors` / `hasInvalid` run during render, so they read the
  // memoized `index` value directly (not the ref) to stay render-safe.
  const validationErrors = useCallback(
    (localId: LocalId): Record<string, string> => {
      const node = nodes[localId];
      if (!node || node.status === "deleted") return {};
      const desc = index.byKey[node.specKey];
      return desc ? desc.validate(node.draft) : {};
    },
    [nodes, index],
  );

  const hasInvalid = useMemo(
    () =>
      Object.values(nodes).some((n) => {
        if (n.status === "deleted" || n.status === "clean") return false;
        const desc = index.byKey[n.specKey];
        if (!desc) return false;
        return Object.keys(desc.validate(n.draft)).length > 0;
      }),
    [nodes, index],
  );

  /** Live mutable working copy used during a save run so the depth-first walk
   *  reads freshly-resolved real ids without waiting for React state. */
  const saveInFlight = useRef(false);

  const save = useCallback(async (): Promise<SaveResult> => {
    if (saveInFlight.current) {
      return { ok: false, persisted: [], failed: [], blocked: [] };
    }
    saveInFlight.current = true;
    // Snapshot the tree into a local working map we mutate as ids resolve, then
    // flush it back to React state once at the end. `nodes` is the current
    // committed state (the hook re-creates `save` whenever `nodes` changes).
    const working: Record<LocalId, NestedNode> = Object.fromEntries(
      Object.entries(nodes).map(([id, n]) => [id, { ...n }]),
    );

    const persisted: LocalId[] = [];
    const failed: { relKey: string; nodeId: LocalId; message: string }[] = [];
    const blocked: LocalId[] = [];

    const setStatus = (id: LocalId, patch: Partial<NestedNode>) => {
      const cur = working[id];
      if (cur) working[id] = { ...cur, ...patch };
    };

    const childrenOf = (parentLocalId: LocalId | undefined, relKey: string): NestedNode[] =>
      Object.values(working).filter(
        (n) => n.parentLocalId === parentLocalId && n.specKey === relKey,
      );

    const errorMessage = (e: unknown): string =>
      e instanceof Error ? e.message : typeof e === "string" ? e : "Save failed";

    /** Recursively upsert (create/update) a node top-down, then its children. A
     *  blocked ancestor short-circuits: descendants are marked blocked, never
     *  attempted. */
    const upsertSubtree = async (
      node: NestedNode,
      parentRealId: string | undefined,
      ancestorBlocked: boolean,
    ): Promise<void> => {
      const desc = indexRef.current.byKey[node.specKey];
      if (!desc) return;

      let realId = node.realId;
      let blockedHere = ancestorBlocked;

      if (ancestorBlocked) {
        if (node.status === "new") {
          setStatus(node.localId, { status: "blocked" });
          blocked.push(node.localId);
          blockedHere = true;
        }
        // A previously-persisted child whose ancestor failed THIS run keeps its
        // own state (it already has a real FK and is independently committable),
        // so it is not force-blocked.
      }

      if (!blockedHere) {
        // Act on any node that isn't already persisted-clean or deleted. The
        // create-vs-update choice keys on whether a REAL id exists — NOT on the
        // status label — so a node whose CREATE failed (status 'failed', no real
        // id) is correctly re-created on the next save, and an inline edit of
        // such a node (which flips it to 'dirty') still re-creates instead of
        // wrongly attempting an update with a missing id.
        const shouldUpsert =
          node.status === "new" || node.status === "dirty" || node.status === "failed";
        if (shouldUpsert) {
          try {
            setStatus(node.localId, { status: "saving", error: undefined });
            const payload = desc.toPayload(node.draft, parentRealId);
            if (!realId) {
              const created = await desc.create(payload);
              realId = created.id;
              setStatus(node.localId, {
                status: "clean",
                realId: created.id,
                baseline: { ...node.draft },
                error: undefined,
              });
            } else {
              await desc.update(realId, payload);
              setStatus(node.localId, {
                status: "clean",
                baseline: { ...node.draft },
                error: undefined,
              });
            }
            persisted.push(node.localId);
          } catch (e) {
            const message = errorMessage(e);
            setStatus(node.localId, { status: "failed", error: message });
            failed.push({ relKey: node.specKey, nodeId: node.localId, message });
            blockedHere = true; // its uncreated descendants can't get a real FK
          }
        }
        // `clean`/`deleted` nodes are skipped here (deletes handled separately).
      }

      // Recurse into the single child relation (creates/updates only here).
      const childDesc = desc.childRelation;
      if (childDesc) {
        const children = childrenOf(node.localId, childDesc.key).filter(
          (c) => c.status !== "deleted",
        );
        for (const child of children) {
          await upsertSubtree(child, realId, blockedHere || !realId);
        }
      }
    };

    /** Bottom-up delete: remove a node's descendants before the node itself. */
    const deleteSubtreeDeletes = async (node: NestedNode): Promise<void> => {
      const desc = indexRef.current.byKey[node.specKey];
      const childDesc = desc?.childRelation;
      if (childDesc) {
        const children = childrenOf(node.localId, childDesc.key);
        for (const child of children) {
          await deleteSubtreeDeletes(child);
        }
      }
      if (node.status !== "deleted") return;
      if (!node.realId || !desc) {
        delete working[node.localId];
        return;
      }
      try {
        setStatus(node.localId, { status: "saving", error: undefined });
        await desc.remove(node.realId);
        delete working[node.localId];
      } catch (e) {
        const message = errorMessage(e);
        setStatus(node.localId, { status: "failed", error: message });
        failed.push({ relKey: node.specKey, nodeId: node.localId, message });
      }
    };

    try {
      // Phase 1: creates + updates, top-down from each root relation level.
      for (const relKey of indexRef.current.rootKeys) {
        const roots = childrenOf(undefined, relKey).filter((n) => n.status !== "deleted");
        for (const node of roots) {
          await upsertSubtree(node, rootIdRef.current, false);
        }
      }
      // Phase 2: deletes, bottom-up. Walk every root subtree; the recursion
      // visits the deepest deleted descendants first.
      for (const relKey of indexRef.current.rootKeys) {
        const roots = childrenOf(undefined, relKey);
        for (const node of roots) {
          await deleteSubtreeDeletes(node);
        }
      }
      // Also handle deleted roots whose subtree was entirely deleted.
    } finally {
      setNodes(working);
      saveInFlight.current = false;
    }

    return { ok: failed.length === 0, persisted, failed, blocked };
  }, [nodes]);

  const tree: NestedEditorTree = {
    nodes,
    rowsByRelation,
    expanded,
    isExpanded,
    toggleExpand,
    addRow,
    patchDraft,
    markDeleted,
    retryNode,
    isDirty,
    validationErrors,
    hasInvalid,
    save,
    loadRootRelation,
  };
  return tree;
}
