// S7: one SDL, two diffs. A change to the annotated SDL changes both the
// database (constraints, indexes, labels, property names) and the public
// API; `diffSchemas` reports both from the same two documents.

import * as graphql from "graphql";
import type { DocumentNode, GraphQLSchema } from "graphql";
import { name } from "../compile/cypher.js";
import { buildModel } from "../model/build.js";
import { type ModelOptions } from "../model/build/limits.js";
import type { Field, GraphModel, ScalarField } from "../model/types.js";
import { buildSchema } from "../schema/build.js";
import {
  inferRequirements,
  requirementDdl,
  requirementId,
  type SchemaRequirement,
} from "./indexes.js";

export interface ApiChange {
  /** graphql-js change type, e.g. `FIELD_REMOVED`. */
  type: string;
  description: string;
}

export interface SchemaDiff {
  database: {
    create: SchemaRequirement[];
    drop: SchemaRequirement[];
    /**
     * Statements in the order to run them. Drops and data migrations are
     * destructive: review them before running.
     */
    statements: Array<{ text: string; destructive: boolean; reason: string }>;
  };
  api: {
    /** Changes that break existing clients. */
    breaking: ApiChange[];
    /** Changes that may change behaviour for existing clients. */
    dangerous: ApiChange[];
  };
  /** Consequences worth a human look, e.g. a rename kept by @alias. */
  notes: string[];
}

export function diffSchemas(
  before: string | DocumentNode,
  after: string | DocumentNode,
  options: ModelOptions = {},
): SchemaDiff {
  const oldModel = buildModel(before, options);
  const newModel = buildModel(after, options);

  const id = requirementId;
  const oldReqs = new Map(inferRequirements(oldModel).map((r) => [id(r), r]));
  const newReqs = new Map(inferRequirements(newModel).map((r) => [id(r), r]));
  const create = [...newReqs]
    .filter(([k]) => !oldReqs.has(k))
    .map(([, r]) => r);
  const drop = [...oldReqs].filter(([k]) => !newReqs.has(k)).map(([, r]) => r);

  const statements: SchemaDiff["database"]["statements"] = [];
  const notes: string[] = [];
  migrations(oldModel, newModel, statements, notes);
  for (const r of drop) {
    statements.push({
      text: `DROP ${r.kind === "constraint" ? "CONSTRAINT" : "INDEX"} \`${r.name.replace(/`/g, "``")}\` IF EXISTS`,
      destructive: true,
      reason: `no longer needed: ${r.reason}`,
    });
  }
  for (const r of create) {
    statements.push({
      text: requirementDdl(r),
      destructive: false,
      reason: r.reason,
    });
  }

  const api = apiChanges(publicSchema(oldModel), publicSchema(newModel));
  return { database: { create, drop, statements }, api, notes };
}

/** Relabelled types and renamed properties need their data moved. */
function migrations(
  before: GraphModel,
  after: GraphModel,
  statements: SchemaDiff["database"]["statements"],
  notes: string[],
) {
  for (const [typeName, oldNode] of before.nodes) {
    const newNode = after.nodes.get(typeName);
    if (!newNode) {
      notes.push(
        `${typeName} was removed; its :${oldNode.labels[0]} nodes stay in the database`,
      );
      continue;
    }
    const oldLabel = oldNode.labels[0]!;
    const newLabel = newNode.labels[0]!;
    if (oldLabel !== newLabel) {
      statements.push({
        text: `MATCH (n:${name(oldLabel)}) SET n:${name(newLabel)} REMOVE n:${name(oldLabel)}`,
        destructive: true,
        reason: `${typeName}'s label changed from ${oldLabel} to ${newLabel}`,
      });
    }
    const oldFields = scalars(oldNode.fields);
    const newFields = scalars(newNode.fields);
    for (const [fieldName, oldField] of oldFields) {
      const newField = newFields.get(fieldName);
      if (newField && newField.property !== oldField.property) {
        statements.push({
          text: `MATCH (n:${name(newLabel)}) WHERE n.${name(oldField.property)} IS NOT NULL SET n.${name(newField.property)} = n.${name(oldField.property)} REMOVE n.${name(oldField.property)}`,
          destructive: true,
          reason: `${typeName}.${fieldName} now stores \`${newField.property}\` instead of \`${oldField.property}\``,
        });
      }
      if (!newField) {
        const renamed = [...newFields.values()].find(
          (f) => f.property === oldField.property && !oldFields.has(f.name),
        );
        if (renamed) {
          notes.push(
            `${typeName}.${fieldName} is now ${typeName}.${renamed.name} over the same property \`${oldField.property}\`: an API break with no data migration`,
          );
        }
      }
    }
  }
}

function scalars(fields: ReadonlyMap<string, Field>): Map<string, ScalarField> {
  const out = new Map<string, ScalarField>();
  for (const f of fields.values()) if (f.kind === "scalar") out.set(f.name, f);
  return out;
}

function publicSchema(model: GraphModel): GraphQLSchema {
  const never = () => Promise.reject(new Error("not connected"));
  return buildSchema(model, {
    customResolver: () => () => null,
    resolveSearch: never,
    resolveAbstract: never,
    subscribe: () => {
      throw new Error("not connected");
    },
    resolveChangedNode: never,
    previousValue: () => null,
    resolveRoot: never,
    resolveNode: never,
    resolveCypher: never,
    resolveMutation: never,
  });
}

type ChangeFinder = (
  a: GraphQLSchema,
  b: GraphQLSchema,
) => Array<{ type: string; description: string }>;

function apiChanges(
  before: GraphQLSchema,
  after: GraphQLSchema,
): SchemaDiff["api"] {
  // graphql-js 16 ships these; 17 removed them.
  const lib = graphql as unknown as {
    findBreakingChanges?: ChangeFinder;
    findDangerousChanges?: ChangeFinder;
  };
  if (!lib.findBreakingChanges || !lib.findDangerousChanges) {
    return {
      breaking: [
        {
          type: "UNSUPPORTED",
          description:
            "this graphql version has no findBreakingChanges; compare printPublicSchema() output with @graphql-inspector/core",
        },
      ],
      dangerous: [],
    };
  }
  const pick = (c: { type: string; description: string }) => ({
    type: c.type,
    description: c.description,
  });
  return {
    breaking: lib.findBreakingChanges(before, after).map(pick),
    dangerous: lib.findDangerousChanges(before, after).map(pick),
  };
}
