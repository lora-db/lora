// The shapes a compiled read is made of: the statements with their result
// shape, what they read, and the raw values the resolvers finish.

import type { QueryResult, Statement } from "../../driver.js";
import type { Clause, Expr } from "../cypher.js";

export type RootKind =
  | "list"
  | "single"
  | "connection"
  | "aggregate"
  | "grouped";

/** How the plan of a statement is expected to find its first rows (S2). */
export interface SeekExpectation {
  statement: number;
  label: string;
  access: "exact" | "range" | "text" | "point";
  reason: string;
}

export interface ReadSet {
  labels: string[];
  relationships: string[];
  /**
   * A `@cypher` statement (or a mutation) is part of the read: it may
   * read anything, so every change affects it.
   */
  opaque?: boolean;
}

export interface CompiledRead {
  statements: Statement[];
  /** Turns the statements' results into the root field's value. */
  shape: (results: QueryResult[]) => unknown;
  /** Result columns the first statement must produce. */
  columns: string[];
  reads: ReadSet;
  expectations: SeekExpectation[];
  /** Estimated rows touched: the bound the cost limit checks. */
  cost: number;
  /** `read` for queries; `write` for @cypher mutations. */
  mode: "read" | "write";
  /**
   * At most one row, found by an exact @key seek, projecting only stored
   * properties: no relationship, @cypher field or rule traversal. Cheap
   * enough for a driver to run synchronously.
   */
  bounded?: boolean;
}

export type Args = Record<string, unknown>;

export type Where = Record<string, unknown>;

/** A connection value before its resolvers turn it into edges/pageInfo. */
export interface RawConnection {
  __rows: RawEdge[];
  __first: number;
  /** A cursor (`after` or `before`) was given. */
  __after: boolean;
  /** `last` / `before`: rows arrive in reverse and are flipped back. */
  __backward: boolean;
  __sort: string;
  __totalCount?: number;
}

export interface RawEdge {
  node: unknown;
  properties?: unknown;
  __cursor: unknown[];
}

export interface Projection {
  /** Subqueries that must run before the projection is evaluated. */
  pre: Clause[];
  expr: Expr;
}
