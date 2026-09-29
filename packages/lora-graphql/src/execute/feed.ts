// The engine's committed change feed as write-sets. With `changeFeed:
// true`, subscriptions and `changes()` are fed from here: every committed
// write, whichever path or process made it (hand-written Cypher and
// `@cypher` mutations included), in commit order.

import type { DriverChange, DriverChangeBatch, LoraDriver } from "../driver.js";
import { keyOf } from "../compile/read.js";
import type {
  GraphModel,
  NodeType,
  RelationshipField,
} from "../model/types.js";
import type { EntityRef, RelationshipRef, WriteChange } from "./changes.js";

/** Node ids seen, for relationship ends and deletions. */
const ID_CACHE = 50_000;

export class EngineFeed {
  readonly #driver: LoraDriver;
  readonly #model: GraphModel;
  readonly #emit: (change: WriteChange) => void;
  readonly #byLabel = new Map<string, NodeType>();
  readonly #ids = new Map<number, EntityRef>();
  #controller: AbortController | undefined;
  #lastLsn: number | undefined;

  constructor(
    driver: LoraDriver,
    model: GraphModel,
    emit: (change: WriteChange) => void,
  ) {
    this.#driver = driver;
    this.#model = model;
    this.#emit = emit;
    for (const node of model.nodes.values()) {
      this.#byLabel.set(node.labels[0]!, node);
    }
  }

  /** Subscribe; resolves once every later commit will be delivered. */
  async start(): Promise<void> {
    if (this.#controller) return;
    this.#controller = new AbortController();
    await this.#open();
  }

  stop(): void {
    this.#controller?.abort();
    this.#controller = undefined;
  }

  async #open(): Promise<void> {
    const signal = this.#controller!.signal;
    const feed = this.#driver.changes!({
      ...(this.#lastLsn !== undefined ? { fromLsn: this.#lastLsn } : {}),
      signal,
    });
    await feed.ready;
    void this.#pump(feed, signal);
  }

  async #pump(
    feed: AsyncIterable<DriverChangeBatch>,
    signal: AbortSignal,
  ): Promise<void> {
    try {
      for await (const batch of feed) {
        this.#lastLsn = batch.lsn;
        const change = await this.#toChange(batch);
        if (change) this.#emit(change);
      }
    } catch (err) {
      if (signal.aborted) return;
      // A consumer that fell behind resumes where it stopped.
      if ((err as { code?: string }).code === "LORA_CHANGES_LAGGED") {
        await this.#open();
        return;
      }
      throw err;
    }
  }

  async #toChange(batch: DriverChangeBatch): Promise<WriteChange | undefined> {
    const change: WriteChange = {
      operation: "EXTERNAL",
      field: "",
      created: [],
      updated: [],
      deleted: [],
      connected: [],
      disconnected: [],
      entities: [],
      types: [],
      relationshipTypes: [],
      broad: false,
    };
    const relationships: DriverChange[] = [];
    for (const c of batch.changes) {
      if (c.kind === "reset") {
        this.#ids.clear();
        change.broad = true;
        continue;
      }
      if (c.kind.startsWith("relationship")) {
        relationships.push(c);
        continue;
      }
      const ref = this.#entity(c.labels ?? [], c.properties ?? {});
      if (!ref) continue;
      this.#remember(c.id!, ref);
      if (c.kind === "nodeCreated") change.created.push(ref);
      else if (c.kind === "nodeDeleted") change.deleted.push(ref);
      else change.updated.push(ref);
    }
    await this.#resolveEnds(relationships);
    for (const r of relationships) {
      const from = this.#ids.get(r.startId!);
      const to = this.#ids.get(r.endId!);
      if (!from || !to) continue;
      const ref = this.#relationship(r.type!, from, to);
      if (r.kind === "relationshipDeleted") change.disconnected.push(ref);
      else if (r.kind === "relationshipCreated") change.connected.push(ref);
      else {
        // Properties changed: both ends' connections changed.
        change.updated.push(from, to);
      }
    }
    fill(change);
    return change.entities.length > 0 || change.broad ? change : undefined;
  }

  /** The @node type and key of a node, from its labels and properties. */
  #entity(
    labels: string[],
    properties: Record<string, unknown>,
  ): EntityRef | undefined {
    for (const label of labels) {
      const node = this.#byLabel.get(label);
      if (!node) continue;
      const key = properties[node.key.property];
      return key === undefined ? undefined : { type: node.name, key };
    }
    return undefined;
  }

  #remember(id: number, ref: EntityRef): void {
    this.#ids.delete(id);
    this.#ids.set(id, ref);
    if (this.#ids.size > ID_CACHE) {
      this.#ids.delete(this.#ids.keys().next().value!);
    }
  }

  /** Look up relationship ends the feed has not shown yet. */
  async #resolveEnds(relationships: DriverChange[]): Promise<void> {
    const missing = [
      ...new Set(
        relationships
          .flatMap((r) => [r.startId!, r.endId!])
          .filter((id) => !this.#ids.has(id)),
      ),
    ];
    if (missing.length === 0) return;
    const [result] = await this.#driver.run(
      [
        {
          text: "UNWIND $ids AS i MATCH (n) WHERE id(n) = i RETURN id(n) AS id, labels(n) AS labels, properties(n) AS props",
          params: { ids: missing },
        },
      ],
      { mode: "read" },
    );
    for (const row of result!.rows) {
      const ref = this.#entity(
        row["labels"] as string[],
        row["props"] as Record<string, unknown>,
      );
      if (ref) this.#remember(row["id"] as number, ref);
    }
  }

  /** The declaring field of a relationship between these types, if any. */
  #relationship(type: string, from: EntityRef, to: EntityRef): RelationshipRef {
    const owners: Array<[EntityRef, EntityRef, "OUT" | "IN"]> = [
      [from, to, "OUT"],
      [to, from, "IN"],
    ];
    for (const [owner, target, direction] of owners) {
      const node = this.#model.nodes.get(owner.type);
      const field = [...(node?.fields.values() ?? [])].find(
        (f): f is RelationshipField =>
          f.kind === "relationship" &&
          f.type === type &&
          f.direction === direction &&
          f.members.includes(target.type),
      );
      if (field) {
        return {
          type,
          field: `${node!.name}.${field.name}`,
          from: owner,
          to: target,
        };
      }
    }
    return { type, field: "", from, to };
  }
}

function fill(change: WriteChange): void {
  const entities = new Map<string, EntityRef>();
  const add = (e: EntityRef) => entities.set(`${e.type}\0${keyOf(e.key)}`, e);
  for (const e of [...change.created, ...change.updated, ...change.deleted]) {
    add(e);
  }
  for (const r of [...change.connected, ...change.disconnected]) {
    add(r.from);
    add(r.to);
  }
  change.entities = [...entities.values()];
  change.types = [...new Set(change.entities.map((e) => e.type))].sort();
  change.relationshipTypes = [
    ...new Set(
      [...change.connected, ...change.disconnected].map((r) => r.type),
    ),
  ].sort();
}
