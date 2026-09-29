// The generated Mutation surface: only for types with @mutation, keyed
// by @key (so every write-set is exact), with nested connect / create /
// disconnect on relationships.

import {
  GraphQLBoolean,
  GraphQLInputObjectType,
  GraphQLInt,
  GraphQLList,
  GraphQLNonNull,
  GraphQLObjectType,
  type GraphQLFieldConfigMap,
  type GraphQLInputFieldConfigMap,
  type GraphQLInputType,
  type GraphQLResolveInfo,
} from "graphql";
import type {
  GraphModel,
  MutationOperation,
  NodeType,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
} from "../model/types.js";
import { upperFirst } from "./names.js";

export type MutationResolver = (
  operation: MutationOperation | "UPSERT",
  node: NodeType,
  info: GraphQLResolveInfo,
  context: unknown,
) => Promise<unknown>;

export const mutationNames = {
  create: (t: NodeType) => `create${upperFirst(t.plural)}`,
  update: (t: NodeType) => `update${t.name}`,
  delete: (t: NodeType) => `delete${t.name}`,
  upsert: (t: NodeType) => `upsert${upperFirst(t.plural)}`,
  upsertInput: (t: string) => `${t}UpsertInput`,
  upsertPayload: (t: NodeType) => `Upsert${upperFirst(t.plural)}Payload`,
  createInput: (t: string) => `${t}CreateInput`,
  updateInput: (t: string) => `${t}UpdateInput`,
  createPayload: (t: NodeType) => `Create${upperFirst(t.plural)}Payload`,
  updatePayload: (t: NodeType) => `Update${t.name}Payload`,
  relationCreate: (t: string, f: string) =>
    `${t}${upperFirst(f)}CreateRelationInput`,
  relationUpdate: (t: string, f: string) =>
    `${t}${upperFirst(f)}UpdateRelationInput`,
  connect: (t: string, f: string) => `${t}${upperFirst(f)}ConnectInput`,
  nestedCreate: (t: string, f: string) => `${t}${upperFirst(f)}CreateNodeInput`,
  /** Payload field holding the created nodes / the updated node. */
  createdField: (t: NodeType) => t.plural,
  updatedField: (t: NodeType) =>
    t.name.charAt(0).toLowerCase() + t.name.slice(1),
};

const nonNull = <T extends GraphQLInputType>(t: T) => new GraphQLNonNull(t);
const listOf = <T extends GraphQLInputType>(t: T) => new GraphQLList(t);

export interface MutationSchemaContext {
  model: GraphModel;
  object: (typeName: string) => GraphQLObjectType;
  inputType: (f: ScalarField) => GraphQLInputType;
}

/** Whether a field may be set by clients on `op`. */
export function settable(f: ScalarField, op: "CREATE" | "UPDATE"): boolean {
  if (f.readonly) return false;
  if (op === "UPDATE" && f.key) return false;
  return true;
}

/** Whether the create input must carry the field. */
export function requiredOnCreate(f: ScalarField): boolean {
  return (
    f.required &&
    !f.defaultValue &&
    !f.timestamp?.has("CREATE") &&
    !(f.key && f.generate)
  );
}

export function buildMutations(
  ctx: MutationSchemaContext,
  resolve: MutationResolver,
): GraphQLFieldConfigMap<unknown, unknown> {
  const { model } = ctx;
  const inputs = new Map<string, GraphQLInputObjectType>();
  const once = (
    name: string,
    fields: () => GraphQLInputFieldConfigMap,
    description?: string,
  ) => {
    let t = inputs.get(name);
    if (!t) {
      t = new GraphQLInputObjectType({ name, fields, description });
      inputs.set(name, t);
    }
    return t;
  };

  const scalarInputs = (
    fields: Iterable<ScalarField>,
    op: "CREATE" | "UPDATE",
  ): GraphQLInputFieldConfigMap => {
    const out: GraphQLInputFieldConfigMap = {};
    for (const f of fields) {
      if (!settable(f, op)) continue;
      const base = ctx.inputType(f);
      const t: GraphQLInputType = f.list ? listOf(nonNull(base)) : base;
      out[f.name] = {
        type: op === "CREATE" && requiredOnCreate(f) ? nonNull(t) : t,
        description:
          op === "UPDATE" && !f.required
            ? `${f.description ? f.description + " " : ""}null removes the value.`
            : f.description,
      };
    }
    return out;
  };

  const propsCreate = (props: RelationshipPropertiesType) =>
    once(`${props.name}CreateInput`, () =>
      scalarInputs(props.fields.values(), "CREATE"),
    );
  const propsRequired = (props: RelationshipPropertiesType) =>
    [...props.fields.values()].some(
      (f) => settable(f, "CREATE") && requiredOnCreate(f),
    );

  const edgeField = (rel: RelationshipField): GraphQLInputFieldConfigMap => {
    if (!rel.properties) return {};
    const props = model.relationshipProperties.get(rel.properties)!;
    const t = propsCreate(props);
    return { edge: { type: propsRequired(props) ? nonNull(t) : t } };
  };

  const connectInput = (owner: NodeType, rel: RelationshipField) => {
    const target = model.nodes.get(rel.target)!;
    return once(
      mutationNames.connect(owner.name, rel.name),
      () => ({
        [target.key.name]: { type: nonNull(ctx.inputType(target.key)) },
        ...edgeField(rel),
      }),
      `Connect an existing ${target.name} by ${target.key.name}.`,
    );
  };

  const nestedCreateInput = (owner: NodeType, rel: RelationshipField) =>
    once(mutationNames.nestedCreate(owner.name, rel.name), () => ({
      node: { type: nonNull(createInput(model.nodes.get(rel.target)!)) },
      ...edgeField(rel),
    }));

  const canCreate = (rel: RelationshipField) =>
    model.nodes.get(rel.target)!.mutations.has("CREATE");

  const relationFields = (
    owner: NodeType,
    rel: RelationshipField,
    update: boolean,
  ): GraphQLInputFieldConfigMap => {
    const target = model.nodes.get(rel.target)!;
    const connect = connectInput(owner, rel);
    const fields: GraphQLInputFieldConfigMap = {
      connect: { type: rel.list ? listOf(nonNull(connect)) : connect },
    };
    if (canCreate(rel)) {
      const create = nestedCreateInput(owner, rel);
      fields["create"] = { type: rel.list ? listOf(nonNull(create)) : create };
    }
    if (update) {
      fields["disconnect"] = rel.list
        ? {
            type: listOf(nonNull(ctx.inputType(target.key))),
            description: `${target.key.name}s of the ${target.name} nodes to disconnect.`,
          }
        : {
            type: GraphQLBoolean,
            description: `true removes the current ${target.name}.`,
          };
    }
    return fields;
  };

  const createInput = (node: NodeType): GraphQLInputObjectType =>
    once(mutationNames.createInput(node.name), () => {
      const fields = scalarInputs(
        [...node.fields.values()].filter(
          (f): f is ScalarField => f.kind === "scalar",
        ),
        "CREATE",
      );
      for (const rel of node.fields.values()) {
        if (rel.kind !== "relationship") continue;
        const t = once(mutationNames.relationCreate(node.name, rel.name), () =>
          relationFields(node, rel, false),
        );
        fields[rel.name] = { type: rel.required && !rel.list ? nonNull(t) : t };
      }
      return fields;
    });

  // Like the create input, but the key is always given and nothing else
  // is required up front: required fields are checked for new nodes only.
  const upsertInput = (node: NodeType): GraphQLInputObjectType =>
    once(mutationNames.upsertInput(node.name), () => {
      const fields: GraphQLInputFieldConfigMap = {};
      for (const f of node.fields.values()) {
        if (f.kind === "scalar") {
          if (!settable(f, "CREATE")) continue;
          const base = ctx.inputType(f);
          const t: GraphQLInputType = f.list ? listOf(nonNull(base)) : base;
          fields[f.name] = {
            type: f.key ? nonNull(t) : t,
            description: f.description,
          };
        } else if (f.kind === "relationship") {
          fields[f.name] = {
            type: once(mutationNames.relationCreate(node.name, f.name), () =>
              relationFields(node, f, false),
            ),
          };
        }
      }
      return fields;
    });

  const updateInput = (node: NodeType): GraphQLInputObjectType =>
    once(mutationNames.updateInput(node.name), () => {
      const fields = scalarInputs(
        [...node.fields.values()].filter(
          (f): f is ScalarField => f.kind === "scalar",
        ),
        "UPDATE",
      );
      for (const rel of node.fields.values()) {
        if (rel.kind !== "relationship") continue;
        fields[rel.name] = {
          type: once(mutationNames.relationUpdate(node.name, rel.name), () =>
            relationFields(node, rel, true),
          ),
        };
      }
      return fields;
    });

  const info = new GraphQLObjectType({
    name: "MutationInfo",
    description: "What a mutation wrote.",
    fields: {
      nodesCreated: { type: new GraphQLNonNull(GraphQLInt) },
      nodesUpdated: { type: new GraphQLNonNull(GraphQLInt) },
      nodesDeleted: { type: new GraphQLNonNull(GraphQLInt) },
      relationshipsCreated: { type: new GraphQLNonNull(GraphQLInt) },
      relationshipsDeleted: { type: new GraphQLNonNull(GraphQLInt) },
    },
  });

  const out: GraphQLFieldConfigMap<unknown, unknown> = {};
  for (const node of model.nodes.values()) {
    const obj = ctx.object(node.name);
    const key = { type: nonNull(ctx.inputType(node.key)) };
    const run =
      (op: MutationOperation | "UPSERT") =>
      (
        _src: unknown,
        _args: unknown,
        context: unknown,
        i: GraphQLResolveInfo,
      ) =>
        resolve(op, node, i, context);
    if (node.mutations.has("CREATE")) {
      out[mutationNames.create(node)] = {
        description: `Create ${node.name} nodes, with their relationships, atomically.`,
        type: new GraphQLNonNull(
          new GraphQLObjectType({
            name: mutationNames.createPayload(node),
            fields: {
              [mutationNames.createdField(node)]: {
                type: new GraphQLNonNull(
                  new GraphQLList(new GraphQLNonNull(obj)),
                ),
              },
              info: { type: new GraphQLNonNull(info) },
            },
          }),
        ),
        args: { input: { type: nonNull(listOf(nonNull(createInput(node)))) } },
        resolve: run("CREATE"),
      };
    }
    // A type with nothing settable after create has no update.
    const updatable = [...node.fields.values()].some(
      (f) =>
        f.kind === "relationship" ||
        (f.kind === "scalar" && settable(f, "UPDATE")),
    );
    if (node.mutations.has("UPDATE") && updatable) {
      out[mutationNames.update(node)] = {
        description: `Update the ${node.name} with this ${node.key.name}. The payload's ${mutationNames.updatedField(node)} is null when there is none.`,
        type: new GraphQLNonNull(
          new GraphQLObjectType({
            name: mutationNames.updatePayload(node),
            fields: {
              [mutationNames.updatedField(node)]: { type: obj },
              info: { type: new GraphQLNonNull(info) },
            },
          }),
        ),
        args: {
          [node.key.name]: key,
          update: { type: nonNull(updateInput(node)) },
        },
        resolve: run("UPDATE"),
      };
    }
    if (node.mutations.has("CREATE") && node.mutations.has("UPDATE")) {
      out[mutationNames.upsert(node)] = {
        description: `Create the ${node.name} nodes whose ${node.key.name} is new and update the others, atomically. Fields required on create are required only for new nodes.`,
        type: new GraphQLNonNull(
          new GraphQLObjectType({
            name: mutationNames.upsertPayload(node),
            fields: {
              [mutationNames.createdField(node)]: {
                type: new GraphQLNonNull(
                  new GraphQLList(new GraphQLNonNull(obj)),
                ),
              },
              info: { type: new GraphQLNonNull(info) },
            },
          }),
        ),
        args: { input: { type: nonNull(listOf(nonNull(upsertInput(node)))) } },
        resolve: run("UPSERT"),
      };
    }
    if (node.mutations.has("DELETE")) {
      out[mutationNames.delete(node)] = {
        description: `Delete the ${node.name} with this ${node.key.name} and its relationships.`,
        type: new GraphQLNonNull(info),
        args: { [node.key.name]: key },
        resolve: run("DELETE"),
      };
    }
  }
  return out;
}
