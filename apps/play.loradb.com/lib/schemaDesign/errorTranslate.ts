/**
 * Translate engine error messages from LoraDB's constraint / index
 * surface into beginner-friendly copy.
 *
 * The engine returns errors with a bracketed code prefix like
 * `[22N65] …`; this module pattern-matches on the code first and
 * falls back to substring sniffing when the code isn't present (older
 * engine builds, wrapped errors).
 */

export interface FriendlyError {
  /** Code from the engine, when we found one. */
  code?: string;
  /** Short headline for the toast / inline banner. */
  title: string;
  /** Longer explanation; safe to render as plain text. */
  body: string;
  /** Optional CTA the caller can wire up. */
  suggestedAction?: "rename" | "showConflict" | "fixData" | "useExisting";
}

const KNOWN: Record<string, Omit<FriendlyError, "code">> = {
  "22N65": {
    title: "An identical constraint already exists",
    body: "A constraint with the same kind and schema is already in place. There's nothing to add here.",
    suggestedAction: "showConflict",
  },
  "22N70": {
    title: "An identical index already exists",
    body: "An index with the same kind and schema is already in place. LoraDB will keep using that one — nothing to do.",
    suggestedAction: "useExisting",
  },
  "22N66": {
    title: "Conflicting constraint on the same property",
    body: "Another constraint already covers this schema with a different kind (for example UNIQUE vs NODE KEY). Drop or change that one first.",
    suggestedAction: "showConflict",
  },
  "22N67": {
    title: "Name already in use",
    body: "Another index or constraint already uses this name. Pick a different one.",
    suggestedAction: "rename",
  },
  "22N71": {
    title: "Name collides with an index",
    body: "There's already an index with that name. Names are shared between indexes and constraints — pick something else.",
    suggestedAction: "rename",
  },
  "22N73": {
    title: "A range index on this property already exists",
    body: "LoraDB will use the existing range index automatically. Drop it first if you really want to replace it.",
    suggestedAction: "useExisting",
  },
  "22N77": {
    title: "Existing rows are missing this property",
    body: "Some nodes or relationships don't have the property yet, so an IS NOT NULL / KEY constraint would fail. Fill in the missing values first.",
    suggestedAction: "fixData",
  },
  "22N78": {
    title: "Some values don't match this type",
    body: "Existing rows hold values that wouldn't satisfy the type predicate. Convert or remove them before adding the constraint.",
    suggestedAction: "fixData",
  },
  "22N79": {
    title: "Duplicate values already exist",
    body: "Existing rows already share a value on this property, so a UNIQUE / KEY constraint can't be added until the duplicates are resolved.",
    suggestedAction: "fixData",
  },
  "22N80": {
    title: "Backing index detected a duplicate at write time",
    body: "A mutation hit a value that a UNIQUE / KEY constraint already owns. The other record needs a different value, or this constraint isn't right for the data.",
    suggestedAction: "fixData",
  },
  "22N90": {
    title: "Property type not supported in this constraint",
    body: "LoraDB rejects MAP, ANY, and a few other shapes for constraints. Pick a scalar, list, or vector type the engine accepts.",
  },
  "42N51": {
    title: "Index or constraint not found",
    body: "The name you tried to drop doesn't exist. Refresh the panel to see what's actually in the catalog.",
  },
  "50N11": {
    title: "Existing data violates this constraint",
    body: "LoraDB scanned the graph and found rows that don't satisfy the constraint. See the underlying details to find them.",
    suggestedAction: "fixData",
  },
};

const CODE_RE = /\[?(\d{2}N\d{2})\]?/;

function pickCode(message: string): string | undefined {
  const m = CODE_RE.exec(message);
  return m ? m[1] : undefined;
}

/** Translate an engine error message into a friendly envelope. */
export function translateError(message: string): FriendlyError {
  const code = pickCode(message);
  if (code && KNOWN[code]) {
    return { code, ...KNOWN[code] };
  }
  const lower = message.toLowerCase();
  if (lower.includes("already exists") && lower.includes("constraint")) {
    return { ...KNOWN["22N65"]! };
  }
  if (lower.includes("already exists") && lower.includes("index")) {
    return { ...KNOWN["22N73"]! };
  }
  if (lower.includes("violates") || lower.includes("conflict")) {
    return { ...KNOWN["50N11"]! };
  }
  return {
    title: "The database rejected the change",
    body: message.length > 0 ? message : "Unknown error.",
  };
}
