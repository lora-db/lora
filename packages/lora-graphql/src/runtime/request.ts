// Helpers for reading a request: which operation a document selects and
// what kind it is, which root field of the model a field name stands for,
// and the selection context of a resolver call.

import {
  getOperationAST,
  GraphQLError,
  Kind,
  type DocumentNode,
  type ExecutionResult,
  type GraphQLResolveInfo,
  type OperationDefinitionNode,
} from "graphql";
import type { RootKind } from "../compile/read.js";
import type { SelectionContext } from "../compile/selection.js";
import type { QueryResult, Statement } from "../driver.js";
import type { LoraTransaction } from "../execute/transaction.js";
import type { GraphModel, NodeType, SearchIndex } from "../model/types.js";
import { names } from "../schema/names.js";

/** The read root field of a node type that `fieldName` names, if any. */
export function rootOf(
  model: GraphModel,
  fieldName: string,
): { kind: RootKind; node: NodeType } | undefined {
  for (const node of model.nodes.values()) {
    if (!node.read) continue;
    if (fieldName === names.listRoot(node)) return { kind: "list", node };
    if (fieldName === names.connectionRoot(node))
      return { kind: "connection", node };
    if (fieldName === names.singleRoot(node)) return { kind: "single", node };
    if (node.aggregate && fieldName === names.aggregateRoot(node)) {
      return { kind: "aggregate", node };
    }
    if (node.aggregate && fieldName === names.groupedRoot(node)) {
      return { kind: "grouped", node };
    }
  }
  return undefined;
}

export function searchOf(
  model: GraphModel,
  fieldName: string,
): { node: NodeType; index: SearchIndex; connection: boolean } | undefined {
  for (const node of model.nodes.values()) {
    if (!node.read) continue;
    for (const index of node.search) {
      if (index.queryName === fieldName) {
        return { node, index, connection: false };
      }
      if (`${index.queryName}Connection` === fieldName) {
        return { node, index, connection: true };
      }
    }
  }
  return undefined;
}

/**
 * A mutation with several root fields; a fragment at the root counts as
 * several, since it may hold more than one.
 */
export function multiRoot(operation: OperationDefinitionNode): boolean {
  const selections = operation.selectionSet.selections;
  return (
    selections.length +
      (selections.some((s) => s.kind !== Kind.FIELD) ? 1 : 0) >
    1
  );
}

export function isDocument(
  value: DocumentNode | ExecutionResult,
): value is DocumentNode {
  return (value as DocumentNode).kind === Kind.DOCUMENT;
}

/**
 * The type of the operation `operationName` selects; undefined when it
 * selects none, which graphql then reports.
 */
export function operationType(
  document: DocumentNode,
  operationName: string | null | undefined,
): "query" | "mutation" | "subscription" | undefined {
  return getOperationAST(document, operationName)?.operation as
    | "query"
    | "mutation"
    | "subscription"
    | undefined;
}

/** A variable graphql-js could not coerce: a request error with no code. */
export function isVariableError(e: GraphQLError): boolean {
  return (
    e.extensions?.["code"] === undefined &&
    e.path === undefined &&
    e.message.startsWith('Variable "$')
  );
}

export function findOperation(
  doc: DocumentNode,
  operationName: string | undefined,
): OperationDefinitionNode {
  const ops = doc.definitions.filter(
    (d): d is OperationDefinitionNode => d.kind === Kind.OPERATION_DEFINITION,
  );
  const op = operationName
    ? ops.find((o) => o.name?.value === operationName)
    : ops.length === 1
      ? ops[0]
      : undefined;
  if (!op) {
    throw new GraphQLError(
      operationName
        ? `unknown operation ${operationName}`
        : "the document must contain exactly one operation, or name one",
    );
  }
  return op;
}

export function infoContext(info: GraphQLResolveInfo): SelectionContext {
  return {
    schema: info.schema,
    fragments: info.fragments,
    variables: info.variableValues,
  };
}

/** Run statements one after the other in a caller-owned transaction. */
export async function runInOrder(
  tx: LoraTransaction,
  statements: Statement[],
): Promise<QueryResult[]> {
  const out: QueryResult[] = [];
  for (const s of statements) out.push(await tx.driverTransaction.execute(s));
  return out;
}
