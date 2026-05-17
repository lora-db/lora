/**
 * Adapter: converts a raw `QueryResult` from `@loradb/lora-wasm` into the
 * playground's `AdaptedResult` shape (table rows + a `GraphData` for the
 * canvas + per-column inferred types + counts).
 *
 * Pure functions only — no `"use client"`, no side effects.
 */

import type {
  LoraNode,
  LoraPath,
  LoraRelationship,
  LoraValue,
  QueryResult,
} from "@loradb/lora-wasm";
import type {
  GraphData,
  LinkObject,
  NodeObject,
} from "@loradb/lora-graph-canvas";
import type { AdaptedResult, CellType, QueryRow } from "./types";

// ---------------------------------------------------------------------------
// Type guards (mirrors lora-wasm's exported guards but stays local so we
// don't add a runtime import dependency).
// ---------------------------------------------------------------------------

function isObjectLike(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function isNode(v: unknown): v is LoraNode {
  return isObjectLike(v) && (v as { kind?: unknown }).kind === "node";
}

function isRelationship(v: unknown): v is LoraRelationship {
  return isObjectLike(v) && (v as { kind?: unknown }).kind === "relationship";
}

function isPath(v: unknown): v is LoraPath {
  return isObjectLike(v) && (v as { kind?: unknown }).kind === "path";
}

// ---------------------------------------------------------------------------
// Cell-type inference
// ---------------------------------------------------------------------------

export function inferType(v: LoraValue | unknown): CellType {
  if (v === null || v === undefined) return "null";
  if (isNode(v)) return "node";
  if (isRelationship(v)) return "relationship";
  if (isPath(v)) return "path";
  if (Array.isArray(v)) return "array";
  switch (typeof v) {
    case "string":
      return "string";
    case "number":
      return "number";
    case "boolean":
      return "boolean";
    case "object":
      return "object";
    default:
      return "object";
  }
}

// ---------------------------------------------------------------------------
// Label helpers
// ---------------------------------------------------------------------------

/**
 * Pick a human-friendly display label for a node. Preference order:
 *   1. `properties.name` if string
 *   2. The first string-valued property
 *   3. The first graph label (`labels[0]`)
 *   4. The node id stringified
 */
export function pickLabel(n: LoraNode): string {
  const props = n.properties;
  const named = props?.["name"];
  if (typeof named === "string" && named.length > 0) return named;
  if (props) {
    for (const key of Object.keys(props)) {
      const value = props[key];
      if (typeof value === "string" && value.length > 0) return value;
    }
  }
  if (n.labels && n.labels.length > 0 && typeof n.labels[0] === "string") {
    return n.labels[0]!;
  }
  return String(n.id);
}

/** The primary graph label (first element of `labels[]`), if any. */
export function primaryLabel(n: LoraNode): string | undefined {
  return n.labels && n.labels.length > 0 ? n.labels[0] : undefined;
}

// ---------------------------------------------------------------------------
// Graph + row assembly
// ---------------------------------------------------------------------------

/**
 * Walk a `LoraValue` and dedupe any nodes/relationships into the provided
 * maps. Paths are flattened into their ID references; we record them so
 * that if the same node/rel appears inline elsewhere it dedupes naturally
 * by id. Path entries that reference IDs we never see inline produce
 * "stub" nodes/links so the graph still renders something.
 */
function collectGraph(
  v: LoraValue,
  nodes: Map<number, LoraNode>,
  rels: Map<number, LoraRelationship>,
  pathNodeIds: Set<number>,
  pathRelIds: Set<number>,
): void {
  if (v === null || v === undefined) return;
  if (isNode(v)) {
    nodes.set(v.id, v);
    return;
  }
  if (isRelationship(v)) {
    rels.set(v.id, v);
    return;
  }
  if (isPath(v)) {
    for (const id of v.nodes) pathNodeIds.add(id);
    for (const id of v.rels) pathRelIds.add(id);
    return;
  }
  if (Array.isArray(v)) {
    for (const item of v) collectGraph(item, nodes, rels, pathNodeIds, pathRelIds);
    return;
  }
  if (isObjectLike(v)) {
    // Generic record (not a tagged structural value) — descend into its
    // properties in case it contains nested structural values.
    for (const key of Object.keys(v)) {
      collectGraph(
        (v as Record<string, LoraValue>)[key] as LoraValue,
        nodes,
        rels,
        pathNodeIds,
        pathRelIds,
      );
    }
  }
}

function toNodeObject(n: LoraNode): NodeObject {
  const node: NodeObject = {
    id: n.id,
    label: pickLabel(n),
    _kind: "node",
    _labels: n.labels,
    _properties: n.properties,
  };
  const group = primaryLabel(n);
  if (group !== undefined) node.group = group;
  return node;
}

function toLinkObject(r: LoraRelationship): LinkObject {
  return {
    id: r.id,
    source: r.startId,
    target: r.endId,
    label: r.type,
    _kind: "relationship",
    _type: r.type,
    _properties: r.properties,
  };
}

function stubNode(id: number): NodeObject {
  return {
    id,
    label: String(id),
    _kind: "node",
    _stub: true,
  };
}

function stubLink(id: number): LinkObject {
  // We don't know the endpoints of a path-only relationship, so emit a
  // self-loop placeholder. force-graph filters out links whose endpoints
  // are missing, so the placeholder needs a valid id reference.
  return {
    id,
    source: id,
    target: id,
    label: "",
    _kind: "relationship",
    _stub: true,
  };
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

export function adapt(raw: QueryResult): AdaptedResult {
  const columns = raw.columns;
  const rawRows = raw.rows;

  const nodeMap = new Map<number, LoraNode>();
  const relMap = new Map<number, LoraRelationship>();
  const pathNodeIds = new Set<number>();
  const pathRelIds = new Set<number>();

  // Build index-aligned `values[]` arrays per row while collecting nodes/rels.
  const rows: QueryRow[] = new Array(rawRows.length);
  // Track per-column observed cell types to compute a dominant type per column.
  const perColumnTypes: Array<Set<CellType>> = columns.map(() => new Set<CellType>());

  for (let r = 0; r < rawRows.length; r++) {
    const rowObj = rawRows[r] as Record<string, LoraValue> | undefined;
    const values: unknown[] = new Array(columns.length);
    for (let c = 0; c < columns.length; c++) {
      const col = columns[c]!;
      const cell = rowObj ? rowObj[col] : null;
      values[c] = cell;
      collectGraph(cell as LoraValue, nodeMap, relMap, pathNodeIds, pathRelIds);
      perColumnTypes[c]!.add(inferType(cell));
    }
    rows[r] = { values };
  }

  // Dominant column type: if a column saw exactly one non-"null" type, use it;
  // if it saw multiple non-null types fall back to "object". Pure-null
  // columns stay "null".
  const cellTypes: CellType[] = perColumnTypes.map((set) => {
    const nonNull = [...set].filter((t) => t !== "null");
    if (nonNull.length === 0) return "null";
    if (nonNull.length === 1) return nonNull[0]!;
    return "object";
  });

  // Add stub entries for path-only references we never saw inline.
  for (const id of pathNodeIds) {
    if (!nodeMap.has(id)) {
      // Use a sentinel LoraNode so toNodeObject still works generically.
      nodeMap.set(id, {
        kind: "node",
        id,
        labels: [],
        properties: {},
      });
    }
  }

  // Stub endpoint nodes for any inline relationship whose start/end node
  // wasn't itself returned (e.g. `RETURN r` on its own). Without these
  // stubs the canvas throws "node not found" when force-graph resolves
  // the link's source/target ids.
  for (const r of relMap.values()) {
    if (!nodeMap.has(r.startId)) {
      nodeMap.set(r.startId, {
        kind: "node",
        id: r.startId,
        labels: [],
        properties: {},
      });
    }
    if (!nodeMap.has(r.endId)) {
      nodeMap.set(r.endId, {
        kind: "node",
        id: r.endId,
        labels: [],
        properties: {},
      });
    }
  }

  const graphNodes: NodeObject[] = [];
  for (const n of nodeMap.values()) graphNodes.push(toNodeObject(n));

  const graphLinks: LinkObject[] = [];
  for (const r of relMap.values()) graphLinks.push(toLinkObject(r));
  // Path-only relationships we never saw inline — emit stub links so the
  // count is honest. They point at themselves because we don't know the
  // endpoints; the canvas can render them as orphan markers.
  for (const id of pathRelIds) {
    if (!relMap.has(id)) graphLinks.push(stubLink(id));
  }

  // If any stub nodes were materialized but never made it into graphNodes
  // (shouldn't happen given the loop above, but defensive), re-add them.
  for (const id of pathNodeIds) {
    if (!graphNodes.some((n) => n.id === id)) graphNodes.push(stubNode(id));
  }

  const hasGraph = graphNodes.length > 0 || graphLinks.length > 0;
  const graph: GraphData | null = hasGraph
    ? { nodes: graphNodes, links: graphLinks }
    : null;

  return {
    columns,
    cellTypes,
    rows,
    graph,
    stats: {
      nodeCount: graphNodes.length,
      relCount: graphLinks.length,
      rowCount: rows.length,
    },
  };
}
