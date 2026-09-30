// The generated Mutation surface: only for types with @mutation, keyed
// by @key (so every write-set is exact), with nested connect / create /
// update / disconnect on relationships, math and list operators, and bulk
// updates and deletes that resolve their keys first.

import {
  GraphQLBoolean,
  GraphQLFloat,
  GraphQLInputObjectType,
  GraphQLInt,
  GraphQLList,
  GraphQLNonNull,
  GraphQLObjectType,
  type GraphQLFieldConfigMap,
  type GraphQLInputFieldConfigMap,
  type GraphQLInputType,
  type GraphQLResolveInfo,
  type GraphQLScalarType,
} from "graphql";
import type {
  GraphModel,
  MutationOperation,
  NodeType,
  NestedOperation,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
} from "../model/types.js";
import { memberFields } from "../model/relations.js";
import { hasSettable, isUpdatable, settable } from "../model/inputs.js";

export { isUpdatable, settable };
import { upperFirst } from "./names.js";

/** A generated mutation: the single-node ones, and the bulk ones by where. */
export type MutationKind =
  | MutationOperation
  | "UPSERT"
  | "UPDATE_MANY"
  | "DELETE_MANY";

export type MutationResolver = (
  operation: MutationKind,
  node: NodeType,
  info: GraphQLResolveInfo,
  context: unknown,
) => Promise<unknown>;

export const mutationNames = {
  create: (t: NodeType) => `create${upperFirst(t.plural)}`,
  update: (t: NodeType) => `update${t.name}`,
  delete: (t: NodeType) => `delete${t.name}`,
  upsert: (t: NodeType) => `upsert${upperFirst(t.plural)}`,
  updateMany: (t: NodeType) => `update${upperFirst(t.plural)}`,
  deleteMany: (t: NodeType) => `delete${upperFirst(t.plural)}`,
  upsertInput: (t: string) => `${t}UpsertInput`,
  upsertPayload: (t: NodeType) => `Upsert${upperFirst(t.plural)}Payload`,
  createInput: (t: string) => `${t}CreateInput`,
  updateInput: (t: string) => `${t}UpdateInput`,
  adjustInput: (t: string) => `${t}AdjustInput`,
  createPayload: (t: NodeType) => `Create${upperFirst(t.plural)}Payload`,
  updatePayload: (t: NodeType) => `Update${t.name}Payload`,
  updateManyPayload: (t: NodeType) => `Update${upperFirst(t.plural)}Payload`,
  relationCreate: (t: string, f: string) =>
    `${t}${upperFirst(f)}CreateRelationInput`,
  relationUpdate: (t: string, f: string) =>
    `${t}${upperFirst(f)}UpdateRelationInput`,
  connect: (t: string, f: string) => `${t}${upperFirst(f)}ConnectInput`,
  nestedCreate: (t: string, f: string) => `${t}${upperFirst(f)}CreateNodeInput`,
  nestedUpdate: (t: string, f: string) =>
    `${t}${upperFirst(f)}UpdateConnectedInput`,
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
  where: (typeName: string) => GraphQLInputType;
}

/** Whether the create input must carry the field. */
export function requiredOnCreate(f: ScalarField): boolean {
  return (
    f.required &&
    !f.defaultValue &&
    !f.timestamp?.has("CREATE") &&
    !f.populatedBy?.operations.has("CREATE") &&
    !(f.key && f.generate)
  );
}

/** Numeric and list fields take operators in `adjust`. */
export function adjustable(f: ScalarField): boolean {
  if (!settable(f, "UPDATE") || f.vector) return false;
  if (f.list) return f.type !== "Point" && f.type !== "CartesianPoint";
  return f.type === "Int" || f.type === "Float" || f.type === "BigInt";
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
  const propsUpdate = (props: RelationshipPropertiesType) => {
    const fields = scalarInputs(props.fields.values(), "UPDATE");
    return Object.keys(fields).length > 0
      ? once(`${props.name}UpdateInput`, () => fields)
      : undefined;
  };
  const propsRequired = (props: RelationshipPropertiesType) =>
    [...props.fields.values()].some(
      (f) => settable(f, "CREATE") && requiredOnCreate(f),
    );

  const edgeField = (rel: RelationshipField): GraphQLInputFieldConfigMap => {
    if (!rel.properties) return {};
    const props = model.relationshipProperties.get(rel.properties)!;
    // Nothing settable on create: no `edge` at all, not an empty input.
    if (!hasSettable(props, "CREATE")) return {};
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
      `Connect an existing ${target.name} by ${target.key.name}. Connecting a connected pair sets the relationship properties given and keeps the others.`,
    );
  };

  const nestedCreateInput = (owner: NodeType, rel: RelationshipField) =>
    once(mutationNames.nestedCreate(owner.name, rel.name), () => ({
      node: { type: nonNull(createInput(model.nodes.get(rel.target)!)) },
      ...edgeField(rel),
    }));

  // `update: [{ key, edge, node }]`: change a connected pair in place.
  const nestedUpdateInput = (owner: NodeType, rel: RelationshipField) => {
    const target = model.nodes.get(rel.target)!;
    const props = rel.properties
      ? model.relationshipProperties.get(rel.properties)
      : undefined;
    const edge = props ? propsUpdate(props) : undefined;
    const node =
      target.mutations.has("UPDATE") && isUpdatable(model, target)
        ? updateInput(target)
        : undefined;
    if (!edge && !node) return undefined;
    return once(mutationNames.nestedUpdate(owner.name, rel.name), () => ({
      ...(rel.list
        ? { [target.key.name]: { type: nonNull(ctx.inputType(target.key)) } }
        : {}),
      ...(edge ? { edge: { type: edge } } : {}),
      ...(node ? { node: { type: node } } : {}),
    }));
  };

  const canCreate = (rel: RelationshipField) =>
    model.nodes.get(rel.target)!.mutations.has("CREATE");

  // Over an interface or union: the same inputs, one field per member.
  const polymorphicFields = (
    owner: NodeType,
    rel: RelationshipField,
    update: boolean,
  ): GraphQLInputFieldConfigMap => {
    const members = memberFields(model, rel).map((m) => ({
      member: m.target,
      // A naming copy, so each member's inputs get their own type names.
      rel: { ...m, name: `${rel.name}${m.target}` },
    }));
    const byMember = (
      suffix: string,
      type: (m: (typeof members)[number]) => GraphQLInputType | undefined,
    ) => {
      const fields: GraphQLInputFieldConfigMap = {};
      for (const m of members) {
        const t = type(m);
        if (t) fields[m.member] = { type: t };
      }
      return Object.keys(fields).length > 0
        ? once(`${owner.name}${upperFirst(rel.name)}${suffix}`, () => fields)
        : undefined;
    };
    const out: GraphQLInputFieldConfigMap = {};
    const allows = (op: NestedOperation) => rel.nestedOperations.has(op);
    const connect = allows("CONNECT")
      ? byMember("ConnectMembers", (m) => {
          const t = connectInput(owner, m.rel);
          return rel.list ? listOf(nonNull(t)) : t;
        })
      : undefined;
    if (connect) out["connect"] = { type: connect };
    const create = allows("CREATE")
      ? byMember("CreateMembers", (m) => {
          if (!model.nodes.get(m.member)!.mutations.has("CREATE"))
            return undefined;
          const t = nestedCreateInput(owner, m.rel);
          return rel.list ? listOf(nonNull(t)) : t;
        })
      : undefined;
    if (create) out["create"] = { type: create };
    if (update && allows("DISCONNECT")) {
      out["disconnect"] = rel.list
        ? {
            type: byMember("DisconnectMembers", (m) =>
              listOf(nonNull(ctx.inputType(model.nodes.get(m.member)!.key))),
            )!,
          }
        : {
            type: GraphQLBoolean,
            description: `true removes the current ${rel.target}.`,
          };
    }
    return out;
  };

  const relationFields = (
    owner: NodeType,
    rel: RelationshipField,
    update: boolean,
  ): GraphQLInputFieldConfigMap => {
    if (model.abstracts.has(rel.target))
      return polymorphicFields(owner, rel, update);
    const target = model.nodes.get(rel.target)!;
    const allows = (op: NestedOperation) => rel.nestedOperations.has(op);
    const fields: GraphQLInputFieldConfigMap = {};
    if (allows("CONNECT")) {
      const connect = connectInput(owner, rel);
      fields["connect"] = {
        type: rel.list ? listOf(nonNull(connect)) : connect,
      };
    }
    if (allows("CREATE") && canCreate(rel)) {
      const create = nestedCreateInput(owner, rel);
      fields["create"] = { type: rel.list ? listOf(nonNull(create)) : create };
    }
    if (update && allows("DISCONNECT")) {
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
    if (update && allows("DELETE") && target.mutations.has("DELETE")) {
      fields["delete"] = rel.list
        ? {
            type: once(
              `${owner.name}${upperFirst(rel.name)}NestedDeleteInput`,
              () => ({
                where: {
                  type: ctx.where(target.name),
                  description: `Which connected ${target.name} nodes; all of them when absent.`,
                },
                limit: {
                  type: GraphQLInt,
                  description:
                    "At most this many (default: maxBatch); more matching is an error.",
                },
              }),
              `Delete connected ${target.name} nodes, following their onDelete rules.`,
            ),
          }
        : {
            type: GraphQLBoolean,
            description: `true deletes the connected ${target.name}.`,
          };
    }
    if (update && allows("UPDATE")) {
      const nested = nestedUpdateInput(owner, rel);
      if (nested) {
        fields["update"] = {
          type: rel.list ? listOf(nonNull(nested)) : nested,
          description: `Update connected ${target.name} nodes or their relationship properties in place.`,
        };
      }
    }
    return fields;
  };

  const scalarsOf = (node: NodeType) =>
    [...node.fields.values()].filter(
      (f): f is ScalarField => f.kind === "scalar",
    );

  const createInput = (node: NodeType): GraphQLInputObjectType =>
    once(mutationNames.createInput(node.name), () => {
      const fields = scalarInputs(scalarsOf(node), "CREATE");
      for (const rel of node.fields.values()) {
        if (rel.kind !== "relationship" || !settable(rel, "CREATE")) continue;
        const inputs = relationFields(node, rel, false);
        if (Object.keys(inputs).length === 0) continue;
        const t = once(
          mutationNames.relationCreate(node.name, rel.name),
          () => inputs,
        );
        // Required single relationships are checked when the mutation
        // runs: a nested create's parent link can satisfy them.
        fields[rel.name] = {
          type: t,
          description:
            rel.required && !rel.list
              ? `Required: connect or create one, unless this ${node.name} is created under it.`
              : undefined,
        };
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
        } else if (f.kind === "relationship" && settable(f, "CREATE")) {
          const inputs = relationFields(node, f, false);
          if (Object.keys(inputs).length === 0) continue;
          fields[f.name] = {
            type: once(
              mutationNames.relationCreate(node.name, f.name),
              () => inputs,
            ),
          };
        }
      }
      return fields;
    });

  function updateInput(node: NodeType): GraphQLInputObjectType {
    return once(mutationNames.updateInput(node.name), () => {
      const fields = scalarInputs(scalarsOf(node), "UPDATE");
      for (const rel of node.fields.values()) {
        if (rel.kind !== "relationship" || !settable(rel, "UPDATE")) continue;
        const inputs = relationFields(node, rel, true);
        if (Object.keys(inputs).length === 0) continue;
        fields[rel.name] = {
          type: once(
            mutationNames.relationUpdate(node.name, rel.name),
            () => inputs,
          ),
        };
      }
      return fields;
    });
  }

  // --- adjust: math and list operators ---------------------------------------

  const numericAdjust = new Map<string, GraphQLInputObjectType>();
  const adjustType = (f: ScalarField): GraphQLInputObjectType => {
    const base = ctx.inputType(f) as GraphQLScalarType;
    if (f.list) {
      const name = `${base.name}ListAdjust`;
      let t = numericAdjust.get(name);
      if (!t) {
        t = new GraphQLInputObjectType({
          name,
          fields: {
            push: {
              type: listOf(nonNull(base)),
              description: "Append these values (a missing list starts empty).",
            },
            pop: {
              type: GraphQLInt,
              description: "Remove this many values from the end.",
            },
            remove: {
              type: listOf(nonNull(base)),
              description: "Remove every occurrence of these values.",
            },
          },
        });
        numericAdjust.set(name, t);
      }
      return t;
    }
    const name = `${base.name}Adjust`;
    let t = numericAdjust.get(name);
    if (!t) {
      const operand = f.type === "Float" ? GraphQLFloat : base;
      t = new GraphQLInputObjectType({
        name,
        description:
          "One operation, applied to the stored value atomically; a missing value counts as 0.",
        fields: {
          add: { type: operand },
          subtract: { type: operand },
          ...(f.type === "BigInt"
            ? {}
            : { multiply: { type: operand }, divide: { type: operand } }),
        },
      });
      numericAdjust.set(name, t);
    }
    return t;
  };

  const adjustInput = (node: NodeType): GraphQLInputObjectType | undefined => {
    const fields = scalarsOf(node).filter(adjustable);
    if (fields.length === 0) return undefined;
    return once(mutationNames.adjustInput(node.name), () =>
      Object.fromEntries(fields.map((f) => [f.name, { type: adjustType(f) }])),
    );
  };

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

  const listPayload = (name: string, node: NodeType) =>
    new GraphQLNonNull(
      new GraphQLObjectType({
        name,
        fields: {
          [mutationNames.createdField(node)]: {
            type: new GraphQLNonNull(
              new GraphQLList(new GraphQLNonNull(ctx.object(node.name))),
            ),
          },
          info: { type: new GraphQLNonNull(info) },
        },
      }),
    );

  const out: GraphQLFieldConfigMap<unknown, unknown> = {};
  for (const node of model.nodes.values()) {
    const obj = ctx.object(node.name);
    const key = { type: nonNull(ctx.inputType(node.key)) };
    const run =
      (op: MutationKind) =>
      (
        _src: unknown,
        _args: unknown,
        context: unknown,
        i: GraphQLResolveInfo,
      ) =>
        resolve(op, node, i, context);
    const updatable = isUpdatable(model, node);
    const adjust = adjustInput(node);
    const updateArgs = {
      ...(updatable ? { update: { type: updateInput(node) } } : {}),
      ...(adjust
        ? { adjust: { type: adjust, description: "Math and list operators." } }
        : {}),
    };

    if (node.mutations.has("CREATE")) {
      out[mutationNames.create(node)] = {
        description: `Create ${node.name} nodes, with their relationships, atomically.`,
        type: listPayload(mutationNames.createPayload(node), node),
        args: { input: { type: nonNull(listOf(nonNull(createInput(node)))) } },
        resolve: run("CREATE"),
      };
    }
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
        args: { [node.key.name]: key, ...updateArgs },
        resolve: run("UPDATE"),
      };
      out[mutationNames.updateMany(node)] = {
        description: `Update every ${node.name} matching \`where\`, atomically. Fails, writing nothing, when more than \`limit\` match (default: the mutation batch limit). Relationships are updated one node at a time, with update${node.name}.`,
        type: listPayload(mutationNames.updateManyPayload(node), node),
        args: {
          where: { type: nonNull(ctx.where(node.name)) },
          ...updateArgs,
          limit: { type: GraphQLInt },
        },
        resolve: run("UPDATE_MANY"),
      };
    }
    if (node.mutations.has("CREATE") && node.mutations.has("UPDATE")) {
      out[mutationNames.upsert(node)] = {
        description: `Create the ${node.name} nodes whose ${node.key.name} is new and update the others, atomically. Fields required on create are required only for new nodes.`,
        type: listPayload(mutationNames.upsertPayload(node), node),
        args: {
          input: { type: nonNull(listOf(nonNull(upsertInput(node)))) },
        },
        resolve: run("UPSERT"),
      };
    }
    if (node.mutations.has("DELETE")) {
      out[mutationNames.delete(node)] = {
        description: `Delete the ${node.name} with this ${node.key.name}, its relationships, and what @relationship(onDelete: CASCADE) reaches.`,
        type: new GraphQLNonNull(info),
        args: { [node.key.name]: key },
        resolve: run("DELETE"),
      };
      out[mutationNames.deleteMany(node)] = {
        description: `Delete every ${node.name} matching \`where\`, atomically. Fails, deleting nothing, when more than \`limit\` match (default: the mutation batch limit).`,
        type: new GraphQLNonNull(info),
        args: {
          where: { type: nonNull(ctx.where(node.name)) },
          limit: { type: GraphQLInt },
        },
        resolve: run("DELETE_MANY"),
      };
    }
  }
  return out;
}
