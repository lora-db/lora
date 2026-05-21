/**
 * Client-side validation for wizard drafts.
 *
 * Returns a list of issues per draft so the wizard can surface them
 * inline (and disable the "Create" button) BEFORE the user pays the
 * round-trip cost of an engine rejection. Mirrors the engine-side
 * checks at `crates/lora-parser/src/parser/schema.rs:528-644`.
 */

import type {
  ConstraintDef,
  ConstraintDraft,
  ConstraintKind,
  IndexDef,
  IndexDraft,
} from "./types";

export interface ValidationIssue {
  field:
    | "name"
    | "label"
    | "properties"
    | "propertyType"
    | "kind"
    | "vectorOptions";
  message: string;
  /** When true the wizard step that owns this field is blocked. */
  blocking: boolean;
}

const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

function checkName(
  name: string,
  existing: ReadonlyArray<{ name: string }>,
  selfName?: string,
): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  if (name.length === 0) {
    issues.push({ field: "name", message: "Pick a name.", blocking: true });
    return issues;
  }
  if (!NAME_RE.test(name)) {
    issues.push({
      field: "name",
      message:
        "Names must start with a letter or underscore and contain only letters, digits, or underscores.",
      blocking: true,
    });
  }
  const collision = existing.find(
    (d) => d.name === name && d.name !== selfName,
  );
  if (collision) {
    issues.push({
      field: "name",
      message: `“${name}” is already in use by another index or constraint.`,
      blocking: true,
    });
  }
  return issues;
}

// ---------------------------------------------------------------------------
// Index drafts
// ---------------------------------------------------------------------------

export function validateIndexDraft(
  draft: IndexDraft,
  existing: {
    indexes: ReadonlyArray<IndexDef>;
    constraints: ReadonlyArray<ConstraintDef>;
  },
  opts?: { selfName?: string },
): ValidationIssue[] {
  const issues: ValidationIssue[] = [];

  // Name: shared namespace across indexes and constraints in LoraDB.
  const others = [...existing.indexes, ...existing.constraints];
  issues.push(...checkName(draft.name, others, opts?.selfName));

  // LOOKUP indexes don't carry a label or property list.
  if (draft.kind !== "LOOKUP") {
    if (draft.label.trim().length === 0) {
      issues.push({
        field: "label",
        message:
          draft.entity === "NODE"
            ? "Pick a label (e.g. Person)."
            : "Pick a relationship type (e.g. KNOWS).",
        blocking: true,
      });
    }
    if (draft.properties.length === 0) {
      issues.push({
        field: "properties",
        message: "Pick at least one property to index.",
        blocking: true,
      });
    }
    const seen = new Set<string>();
    for (const p of draft.properties) {
      if (seen.has(p)) {
        issues.push({
          field: "properties",
          message: `Property “${p}” appears more than once.`,
          blocking: true,
        });
        break;
      }
      seen.add(p);
    }
  }

  // VECTOR indexes carry a single property (engine enforces this with
  // a clear error too — surface it client-side so the user can fix
  // before they see a rejection) and a tuning block.
  if (draft.kind === "VECTOR") {
    if (draft.properties.length > 1) {
      issues.push({
        field: "properties",
        message:
          "Vector indexes apply to a single embedding property — pick one.",
        blocking: true,
      });
    }
    const v = draft.vectorOptions;
    if (v) {
      if (
        !Number.isFinite(v.dimensions) ||
        v.dimensions < 1 ||
        v.dimensions > 4096
      ) {
        issues.push({
          field: "vectorOptions",
          message: "Dimensions must be between 1 and 4096.",
          blocking: true,
        });
      }
      if (v.provider === "hnsw") {
        if (v.hnswM < 4 || v.hnswM > 128) {
          issues.push({
            field: "vectorOptions",
            message: "HNSW M must be between 4 and 128.",
            blocking: true,
          });
        }
        if (v.hnswEfConstruction < 16 || v.hnswEfConstruction > 2000) {
          issues.push({
            field: "vectorOptions",
            message:
              "HNSW efConstruction must be between 16 and 2000.",
            blocking: true,
          });
        }
        if (v.hnswEfSearch < 16 || v.hnswEfSearch > 2000) {
          issues.push({
            field: "vectorOptions",
            message: "HNSW efSearch must be between 16 and 2000.",
            blocking: true,
          });
        }
      }
      if (v.quantization === "int8" && v.similarity !== "cosine") {
        issues.push({
          field: "vectorOptions",
          message:
            "int8 quantization currently requires the cosine similarity function.",
          blocking: true,
        });
      }
      if (v.quantization === "int8" && v.provider !== "hnsw") {
        issues.push({
          field: "vectorOptions",
          message:
            "Quantization only applies to the HNSW provider — switch provider or set quantization to none.",
          blocking: true,
        });
      }
    }
  }

  // Conflict: range index on same schema is already owned by another
  // constraint. The engine rejects this with `22N73`; we catch it
  // client-side so the user gets a clear pointer to the owning
  // constraint instead of an opaque code.
  if (draft.kind === "RANGE") {
    const conflict = existing.indexes.find(
      (idx) =>
        idx.owned &&
        idx.entity === draft.entity &&
        idx.labelsOrTypes[0] === draft.label &&
        sameStringSet(idx.properties, draft.properties),
    );
    if (conflict && conflict.name !== opts?.selfName) {
      issues.push({
        field: "kind",
        message: `A RANGE index for this schema already exists (owned by constraint “${conflict.ownerConstraint ?? ""}”).`,
        blocking: true,
      });
    }
  }

  return issues;
}

// ---------------------------------------------------------------------------
// Constraint drafts
// ---------------------------------------------------------------------------

const NODE_ONLY: ConstraintKind[] = ["NODE_KEY"];
const REL_ONLY: ConstraintKind[] = ["RELATIONSHIP_KEY"];
const COMPOSITE_KINDS: ConstraintKind[] = ["NODE_KEY", "RELATIONSHIP_KEY"];
const SINGLE_PROP_ONLY: ConstraintKind[] = ["NOT_NULL", "PROPERTY_TYPE"];

export function validateConstraintDraft(
  draft: ConstraintDraft,
  existing: {
    indexes: ReadonlyArray<IndexDef>;
    constraints: ReadonlyArray<ConstraintDef>;
  },
  opts?: { selfName?: string },
): ValidationIssue[] {
  const issues: ValidationIssue[] = [];

  const others = [...existing.indexes, ...existing.constraints];
  issues.push(...checkName(draft.name, others, opts?.selfName));

  // Entity / kind compatibility — the wizard hides incompatible kinds
  // but a stale draft could still land here.
  if (NODE_ONLY.includes(draft.kind) && draft.entity !== "NODE") {
    issues.push({
      field: "kind",
      message: "NODE KEY constraints only apply to nodes.",
      blocking: true,
    });
  }
  if (REL_ONLY.includes(draft.kind) && draft.entity !== "RELATIONSHIP") {
    issues.push({
      field: "kind",
      message: "RELATIONSHIP KEY constraints only apply to relationships.",
      blocking: true,
    });
  }

  if (draft.label.trim().length === 0) {
    issues.push({
      field: "label",
      message:
        draft.entity === "NODE"
          ? "Pick a label (e.g. Person)."
          : "Pick a relationship type (e.g. KNOWS).",
      blocking: true,
    });
  }

  if (draft.properties.length === 0) {
    issues.push({
      field: "properties",
      message: "Pick at least one property.",
      blocking: true,
    });
  }

  if (SINGLE_PROP_ONLY.includes(draft.kind) && draft.properties.length > 1) {
    issues.push({
      field: "properties",
      message:
        draft.kind === "NOT_NULL"
          ? "IS NOT NULL applies to a single property."
          : "Property-type constraints apply to a single property.",
      blocking: true,
    });
  }

  if (COMPOSITE_KINDS.includes(draft.kind) && draft.properties.length < 2) {
    issues.push({
      field: "properties",
      message:
        "Key constraints need at least two properties (use UNIQUE for one).",
      blocking: true,
    });
  }

  // Duplicate constraint on same schema (same kind + same properties).
  const dup = existing.constraints.find(
    (c) =>
      c.kind === draft.kind &&
      c.entity === draft.entity &&
      c.label === draft.label &&
      sameStringSet(c.properties, draft.properties) &&
      c.name !== opts?.selfName,
  );
  if (dup) {
    issues.push({
      field: "kind",
      message: `An identical constraint already exists (“${dup.name}”).`,
      blocking: true,
    });
  }

  return issues;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function sameStringSet(
  a: readonly string[],
  b: readonly string[],
): boolean {
  if (a.length !== b.length) return false;
  const set = new Set(a);
  for (const item of b) if (!set.has(item)) return false;
  return true;
}

/** True when no issue in the list blocks submission. */
export function isSubmittable(issues: readonly ValidationIssue[]): boolean {
  return issues.every((i) => !i.blocking);
}
