// The output side: interfaces and unions, one object and connection per
// @node type, relationship connections with their properties and
// aggregates, and the types and arguments of @cypher fields.

import {
  GraphQLID,
  GraphQLInputObjectType,
  GraphQLInt,
  GraphQLInterfaceType,
  GraphQLObjectType,
  GraphQLString,
  GraphQLUnionType,
  type GraphQLFieldConfig,
  type GraphQLFieldConfigArgumentMap,
  type GraphQLFieldConfigMap,
  type GraphQLInputType,
  type GraphQLOutputType,
} from "graphql";
import { encodeCursor } from "../compile/cursor.js";
import type { RawConnection, RawEdge } from "../compile/read/types.js";
import { requestError } from "../errors.js";
import type {
  CypherField,
  GraphModel,
  NodeType,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
  ScalarType,
} from "../model/types.js";
import { aggregatable, type AggregateFieldType } from "./aggregates.js";
import { listOf, nonNull, type BaseTypes } from "./base.js";
import type { Filters } from "./filters.js";
import { toGlobalId } from "./global-id.js";
import { assertReadable } from "./guard.js";
import type { SchemaHooks } from "./hooks.js";
import { hasOwnConnection, names } from "./names.js";
import {
  byResponseKey,
  nodesByResponseKey,
  pageOf,
  vectorByResponseKey,
  type SortedEdge,
} from "./resolvers.js";
import type { Sorts } from "./sorts.js";

export function buildObjects(
  model: GraphModel,
  hooks: SchemaHooks,
  base: BaseTypes,
  filters: Filters,
  sorts: Sorts,
  aggregateFieldType: AggregateFieldType,
) {
  const {
    sortDirection,
    pageInfo,
    enums,
    nodeInterface,
    customScalar,
    baseType,
    scalarOutput,
  } = base;
  const { whereOf, polymorphic, connectionWhere } = filters;

  // --- Objects and connections ----------------------------------------------

  const objects = new Map<string, GraphQLObjectType>();
  const connections = new Map<string, GraphQLObjectType>();

  // Interfaces and unions over @node types; `__typename` in each projected
  // node tells them apart.
  const abstractTypes = new Map<
    string,
    GraphQLInterfaceType | GraphQLUnionType
  >();
  const resolveType = (value: unknown) =>
    (value as { __typename?: string }).__typename ?? undefined;
  for (const abstract of model.abstracts.values()) {
    abstractTypes.set(
      abstract.name,
      abstract.kind === "interface"
        ? new GraphQLInterfaceType({
            name: abstract.name,
            description: abstract.description,
            fields: () => ({
              ...Object.fromEntries(
                [...abstract.fields.values()]
                  .filter((f) => !f.private && f.selectableOn.read)
                  .map((f) => [
                    f.name,
                    { type: scalarOutput(f), description: f.description },
                  ]),
              ),
              // Declared relationships: the implementations' field, whose
              // type and arguments the model checked are the same.
              ...Object.fromEntries(
                [...abstract.relationships.values()].map((rel) => {
                  const impl = objects.get(abstract.members[0]!)!.getFields()[
                    rel.name
                  ]!;
                  return [
                    rel.name,
                    {
                      type: impl.type,
                      description: rel.description,
                      args: Object.fromEntries(
                        impl.args.map((a) => [
                          a.name,
                          {
                            type: a.type,
                            defaultValue: a.defaultValue,
                            description: a.description,
                          },
                        ]),
                      ),
                    },
                  ];
                }),
              ),
            }),
            resolveType,
          })
        : new GraphQLUnionType({
            name: abstract.name,
            description: abstract.description,
            types: () => abstract.members.map((m) => objects.get(m)!),
            resolveType,
          }),
    );
  }
  const outputOf = (
    name: string,
  ): GraphQLObjectType | GraphQLInterfaceType | GraphQLUnionType =>
    objects.get(name) ?? abstractTypes.get(name)!;

  const connectionResolvers = {
    edges: (src: RawConnection) =>
      pageOf(src).map((row) => {
        assertReadable(row.node);
        return { ...row, __sort: src.__sort };
      }),
    pageInfo: (src: RawConnection) => {
      const page = pageOf(src);
      // Cursors carry sort values: as readable as the nodes themselves.
      for (const row of page) assertReadable(row.node);
      const cursor = (row: RawEdge | undefined) =>
        row ? encodeCursor(src.__sort, row.__cursor, model.cursorSecret) : null;
      const more = src.__rows.length > src.__first;
      return {
        hasNextPage: src.__backward ? src.__after : more,
        hasPreviousPage: src.__backward ? more : src.__after,
        startCursor: cursor(page[0]),
        endCursor: cursor(page[page.length - 1]),
      };
    },
    totalCount: (src: RawConnection & { __denied?: number }) => {
      if ((src.__denied ?? 0) > 0) {
        throw requestError(
          "FORBIDDEN",
          "not allowed to read some of these nodes",
        );
      }
      return src.__totalCount ?? 0;
    },
  };

  const makeConnection = (
    name: string,
    edgeName: string,
    nodeType: () => GraphQLObjectType,
    propertiesType?: () => GraphQLObjectType,
    aggregate?: () => GraphQLObjectType | undefined,
  ) => {
    const edge = new GraphQLObjectType<SortedEdge>({
      name: edgeName,
      fields: () => ({
        cursor: {
          type: nonNull(GraphQLString),
          resolve: (src) =>
            encodeCursor(src.__sort, src.__cursor, model.cursorSecret),
        },
        node: {
          type: nonNull(nodeType()),
          resolve: (src: SortedEdge) => src.node,
        },
        ...(propertiesType
          ? {
              properties: {
                type: nonNull(propertiesType()),
                resolve: (src: SortedEdge) => src.properties,
              },
            }
          : {}),
      }),
    });
    return new GraphQLObjectType({
      name,
      fields: () => ({
        edges: {
          type: nonNull(listOf(nonNull(edge))),
          resolve: connectionResolvers.edges,
        },
        pageInfo: {
          type: nonNull(pageInfo),
          resolve: connectionResolvers.pageInfo,
        },
        totalCount: {
          type: nonNull(GraphQLInt),
          resolve: connectionResolvers.totalCount,
        },
        ...(aggregate?.()
          ? {
              aggregate: {
                type: nonNull(aggregate()!),
                description:
                  "Aggregates over every matching node (not just this page).",
                resolve: (src: RawConnection) =>
                  (src as RawConnection & { __aggregate?: unknown })
                    .__aggregate,
              },
            }
          : {}),
      }),
    });
  };

  /**
   * `{ count, node { f { min max avg sum } }, edge { … } }` for connections
   * whose target has @query(aggregate: true).
   */
  const connectionAggregates = new Map<string, GraphQLObjectType>();
  const aggregateSides = new Map<string, GraphQLObjectType | undefined>();
  const relationshipCount = new GraphQLObjectType({
    name: "RelationshipCount",
    description:
      "Related nodes, and the relationships to them: they differ when several relationships lead to the same node.",
    fields: {
      nodes: { type: nonNull(GraphQLInt) },
      edges: { type: nonNull(GraphQLInt) },
    },
  });
  const connectionAggregate = (
    name: string,
    target: NodeType,
    props: RelationshipPropertiesType | undefined,
    relationship = false,
  ): GraphQLObjectType | undefined => {
    if (!target.aggregate) return undefined;
    let t = connectionAggregates.get(name);
    if (!t) {
      const side = (sideName: string, fields: Iterable<ScalarField>) => {
        if (aggregateSides.has(sideName)) return aggregateSides.get(sideName);
        const usable = [...fields].filter(aggregatable);
        const made =
          usable.length > 0
            ? new GraphQLObjectType({
                name: sideName,
                fields: Object.fromEntries(
                  usable.map((f) => [
                    f.name,
                    { type: nonNull(aggregateFieldType(f)) },
                  ]),
                ),
              })
            : undefined;
        aggregateSides.set(sideName, made);
        return made;
      };
      const node = side(
        `${target.name}AggregateNode`,
        [...target.fields.values()].filter(
          (f): f is ScalarField => f.kind === "scalar",
        ),
      );
      const edge = props
        ? side(`${props.name}AggregateEdge`, props.fields.values())
        : undefined;
      t = new GraphQLObjectType({
        name,
        fields: {
          count: relationship
            ? { type: nonNull(relationshipCount) }
            : { type: nonNull(GraphQLInt), description: "Matching nodes." },
          ...(node ? { node: { type: nonNull(node) } } : {}),
          ...(edge ? { edge: { type: nonNull(edge) } } : {}),
        },
      });
      connectionAggregates.set(name, t);
    }
    return t;
  };

  const propsObjects = new Map<string, GraphQLObjectType>();
  const propsObject = (props: RelationshipPropertiesType) => {
    let t = propsObjects.get(props.name);
    if (!t) {
      t = new GraphQLObjectType({
        name: props.name,
        description: props.description,
        fields: () =>
          Object.fromEntries(
            [...props.fields.values()]
              // `@selectable(onRead: false)` hides it here as on a node.
              .filter((f) => !f.private && f.selectableOn.read)
              .map((f) => [
                f.name,
                {
                  type: scalarOutput(f),
                  description: f.description,
                  resolve: byResponseKey,
                },
              ]),
          ),
      });
      propsObjects.set(props.name, t);
    }
    return t;
  };

  const listArgs = (target: string): GraphQLFieldConfigArgumentMap => ({
    where: { type: whereOf(target) },
    ...(sorts.has(target)
      ? { sort: { type: listOf(nonNull(sorts.get(target)!)) } }
      : {}),
    limit: { type: GraphQLInt },
  });

  // Relationship connections whose properties have @sortable fields sort
  // by `edge` too: `sort: [{ edge: { since: DESC } }, { name: ASC }]`.
  const edgeSorts = new Map<string, GraphQLInputObjectType>();
  const edgeSortOf = (
    node: NodeType,
    rel: RelationshipField,
    props: RelationshipPropertiesType,
  ): GraphQLInputObjectType | undefined => {
    const sortable = [...props.fields.values()].filter((f) => f.sortable);
    const target = model.nodes.get(rel.target)!;
    if (sortable.length === 0 || target.fields.has("edge")) return undefined;
    const name = `${names.relConnection(node.name, rel.name)}Sort`;
    let t = edgeSorts.get(name);
    if (!t) {
      const edgeName = names.sort(props.name);
      let edge = edgeSorts.get(edgeName);
      if (!edge) {
        edge = new GraphQLInputObjectType({
          name: edgeName,
          description: `Sorts by ${props.name} relationship properties.`,
          fields: Object.fromEntries(
            sortable.map((f) => [f.name, { type: sortDirection }]),
          ),
        });
        edgeSorts.set(edgeName, edge);
      }
      t = new GraphQLInputObjectType({
        name,
        description: `Sorts ${rel.name} by ${target.name} fields or, under \`edge\`, relationship properties. One field per item; the @key breaks ties.`,
        fields: () => ({
          ...Object.fromEntries(
            Object.values(sorts.get(rel.target)!.getFields()).map((f) => [
              f.name,
              { type: f.type, description: f.description },
            ]),
          ),
          edge: { type: edge },
        }),
      });
      edgeSorts.set(name, t);
    }
    return t;
  };

  const connectionArgs = (
    target: string,
    where: GraphQLInputType = whereOf(target),
    sort: GraphQLInputObjectType = sorts.get(target)!,
  ): GraphQLFieldConfigArgumentMap => ({
    where: { type: where },
    sort: { type: listOf(nonNull(sort)) },
    first: { type: GraphQLInt },
    after: { type: GraphQLString },
    last: {
      type: GraphQLInt,
      description: "Page backward: the last n before `before`.",
    },
    before: { type: GraphQLString },
  });

  for (const node of model.nodes.values()) {
    const obj: GraphQLObjectType = new GraphQLObjectType({
      name: node.name,
      description: node.description,
      interfaces: () => [
        ...(node.key.relayId && nodeInterface ? [nodeInterface] : []),
        ...node.interfaces
          .map((i) => abstractTypes.get(i))
          .filter(
            (t): t is GraphQLInterfaceType => t instanceof GraphQLInterfaceType,
          ),
      ],
      fields: () => {
        const fields: GraphQLFieldConfigMap<
          Record<string, unknown>,
          unknown
        > = {};
        if (node.key.relayId) {
          fields["id"] = {
            type: nonNull(GraphQLID),
            description: "Global id.",
            resolve: (src, _a, _c, info) =>
              toGlobalId(node.name, src[info.path.key as string]),
          };
        }
        for (const f of node.fields.values()) {
          if (f.kind === "scalar") {
            if (f.private || !f.selectableOn.read) continue;
            fields[f.name] = {
              type: scalarOutput(f),
              description: f.description,
              // Vectors are stored tagged; clients see the numbers.
              resolve: f.vector ? vectorByResponseKey : byResponseKey,
            };
            continue;
          }
          if (f.kind === "cypher") {
            fields[f.name] = {
              type: cypherOutput(f),
              description: f.description,
              args: cypherArgs(f),
              resolve: f.node ? nodesByResponseKey : byResponseKey,
            };
            continue;
          }
          if (f.kind === "custom") {
            const base = namedType(f.type.named) as GraphQLOutputType;
            const item =
              f.type.list && f.type.itemRequired ? nonNull(base) : base;
            const t: GraphQLOutputType = f.type.list ? listOf(item) : item;
            fields[f.name] = {
              type: f.type.required ? nonNull(t) : t,
              description: f.description,
              resolve: hooks.customResolver(node, f),
            };
            continue;
          }
          const target = outputOf(f.target);
          if (!f.list) {
            fields[f.name] = {
              type: f.required ? nonNull(target) : target,
              description: f.description,
              args: { where: { type: whereOf(f.target) } },
              resolve: nodesByResponseKey,
            };
            continue;
          }
          fields[f.name] = {
            type: nonNull(listOf(nonNull(target))),
            description: f.description,
            args: listArgs(f.target),
            resolve: nodesByResponseKey,
          };
          if (!polymorphic(f)) {
            fields[names.connectionField(f.name)] = relationshipConnectionField(
              node,
              f,
            );
          }
        }
        return fields;
      },
    });
    objects.set(node.name, obj);
    connections.set(
      node.name,
      makeConnection(
        names.connection(node.name),
        names.edge(node.name),
        () => obj,
        undefined,
        () =>
          connectionAggregate(
            `${names.connection(node.name)}Aggregate`,
            node,
            undefined,
          ),
      ),
    );
  }

  function relationshipConnectionField(
    node: NodeType,
    rel: RelationshipField,
  ): GraphQLFieldConfig<Record<string, unknown>, unknown> {
    if (!hasOwnConnection(rel)) {
      // Aggregates through a relationship count relationships too, and
      // opting out removes them: both need a connection type of its own.
      const target = model.nodes.get(rel.target)!;
      const conn =
        rel.aggregate && !target.aggregate
          ? connections.get(rel.target)!
          : makeConnection(
              names.relConnection(node.name, rel.name),
              names.relEdge(node.name, rel.name),
              () => objects.get(rel.target)!,
              undefined,
              rel.aggregate
                ? () =>
                    connectionAggregate(
                      `${names.relConnection(node.name, rel.name)}Aggregate`,
                      target,
                      undefined,
                      true,
                    )
                : undefined,
            );
      return {
        type: nonNull(conn),
        args: connectionArgs(rel.target),
        resolve: byResponseKey,
      };
    }
    const props = model.relationshipProperties.get(rel.properties!)!;
    const where = connectionWhere(node, rel);
    const conn = makeConnection(
      names.relConnection(node.name, rel.name),
      names.relEdge(node.name, rel.name),
      () => objects.get(rel.target)!,
      // Every property hidden from reads (@private, @selectable(onRead:
      // false)): no edge type, and no `properties` on the edge. The
      // properties stay settable through `edge` inputs.
      [...props.fields.values()].some((f) => !f.private && f.selectableOn.read)
        ? () => propsObject(props)
        : undefined,
      rel.aggregate
        ? () =>
            connectionAggregate(
              `${names.relConnection(node.name, rel.name)}Aggregate`,
              model.nodes.get(rel.target)!,
              props,
              true,
            )
        : undefined,
    );
    return {
      type: nonNull(conn),
      args: connectionArgs(
        rel.target,
        where,
        edgeSortOf(node, rel, props) ?? sorts.get(rel.target)!,
      ),
      resolve: byResponseKey,
    };
  }

  // --- @cypher -----------------------------------------------------------------

  // Object types without @node: shapes of @cypher results, read from maps.
  const plainObjects = new Map<string, GraphQLObjectType>();
  for (const plain of model.objects.values()) {
    plainObjects.set(
      plain.name,
      new GraphQLObjectType({
        name: plain.name,
        description: plain.description,
        fields: () =>
          Object.fromEntries(
            [...plain.fields.values()].map((f) => {
              const base = namedType(f.type.named) as GraphQLOutputType;
              const item =
                f.type.list && f.type.itemRequired ? nonNull(base) : base;
              const t: GraphQLOutputType = f.type.list ? listOf(item) : item;
              return [f.name, { type: f.type.required ? nonNull(t) : t }];
            }),
          ),
      }),
    );
  }

  const namedType = (named: string) =>
    objects.get(named) ??
    abstractTypes.get(named) ??
    plainObjects.get(named) ??
    enums.get(named) ??
    customScalar(named) ??
    baseType(named as ScalarType, undefined);

  function cypherOutput(f: CypherField): GraphQLOutputType {
    const base = namedType(f.type.named) as GraphQLOutputType;
    const item = f.type.list && f.type.itemRequired ? nonNull(base) : base;
    const t: GraphQLOutputType = f.type.list ? listOf(item) : item;
    return f.type.required ? nonNull(t) : t;
  }

  function cypherArgs(f: CypherField): GraphQLFieldConfigArgumentMap {
    const out: GraphQLFieldConfigArgumentMap = {};
    for (const a of f.args) {
      const base = namedType(a.type.named) as GraphQLInputType;
      const item = a.type.list && a.type.itemRequired ? nonNull(base) : base;
      const t: GraphQLInputType = a.type.list ? listOf(item) : item;
      out[a.name] = {
        type: a.type.required ? nonNull(t) : t,
        defaultValue: a.defaultValue,
        description: a.description,
      };
    }
    return out;
  }

  return {
    objects,
    connections,
    abstractTypes,
    plainObjects,
    connectionResolvers,
    listArgs,
    connectionArgs,
    cypherOutput,
    cypherArgs,
  };
}

export type ObjectTypes = ReturnType<typeof buildObjects>;
