// The documents a `LoraGraphQL` instance runs: sources parsed and
// validated once and cached by text, and persisted operations by id.

import {
  GraphQLError,
  Kind,
  parse,
  specifiedRules,
  validate,
  type DocumentNode,
  type ExecutionResult,
  type GraphQLSchema,
  type OperationDefinitionNode,
  type ValidationRule,
} from "graphql";
import type { OperationManifest } from "../codegen.js";
import { parseOptions, type DocumentGuards } from "../guards.js";
import type { ExecuteArgs } from "../options.js";

const DOCUMENT_CACHE_SIZE = 500;
/**
 * A parsed document's approximate size: measured at about 100 bytes of
 * AST, locations and tokens per source character.
 */
const documentBytes = (source: string) => 100 * source.length;

export interface DocumentsOptions {
  guards: DocumentGuards | false;
  persistedOnly: boolean;
  /** Approximate bytes the parsed document cache may hold. */
  bytesMax: number;
  /** The executable schema, built on first use. */
  schema: () => GraphQLSchema;
  /** The configured document guards as validation rules. */
  rules: () => ValidationRule[];
}

export class Documents {
  readonly #guards: DocumentGuards | false;
  readonly #persistedOnly: boolean;
  readonly #schema: () => GraphQLSchema;
  readonly #rules: () => ValidationRule[];
  /** Persisted operation ids by operation node, for statement events. */
  readonly #persistedIds = new WeakMap<OperationDefinitionNode, string>();
  readonly #documents = new Map<string, DocumentNode>();
  readonly #persisted = new Map<string, DocumentNode>();
  readonly #documentBytesMax: number;
  #documentBytes = 0;

  constructor(options: DocumentsOptions) {
    this.#guards = options.guards;
    this.#persistedOnly = options.persistedOnly;
    this.#documentBytesMax = options.bytesMax;
    this.#schema = options.schema;
    this.#rules = options.rules;
  }

  #parse(source: string): DocumentNode {
    return parse(
      source,
      this.#guards === false ? undefined : parseOptions(this.#guards),
    );
  }

  #validate(document: DocumentNode): readonly GraphQLError[] {
    return validate(this.#schema(), document, [
      ...specifiedRules,
      ...this.#rules(),
    ]);
  }

  /**
   * Register persisted operations by id. Every document is parsed and
   * validated now, so a broken one fails at startup; at request time an
   * id is looked up and executed without parsing or validating.
   */
  persist(operations: Record<string, string>): void {
    const problems: string[] = [];
    for (const [id, source] of Object.entries(operations)) {
      try {
        const doc = this.#parse(source);
        const errors = this.#validate(doc);
        if (errors.length > 0) {
          problems.push(`${id}: ${errors.map((e) => e.message).join("; ")}`);
        } else {
          this.#persisted.set(id, doc);
          for (const def of doc.definitions) {
            if (def.kind === Kind.OPERATION_DEFINITION) {
              this.#persistedIds.set(def, id);
            }
          }
        }
      } catch (err) {
        problems.push(
          `${id}: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }
    if (problems.length > 0) {
      throw new Error(
        `invalid persisted operations:\n  ${problems.join("\n  ")}`,
      );
    }
  }

  /** Register a manifest's operations, without parsing or validating. */
  load(manifest: OperationManifest): void {
    for (const [id, entry] of Object.entries(manifest.operations)) {
      this.#persisted.set(id, entry.document);
      for (const def of entry.document.definitions) {
        if (def.kind === Kind.OPERATION_DEFINITION) {
          this.#persistedIds.set(def, id);
        }
      }
    }
  }

  /** A persisted operation's document, by id. */
  persisted(id: string): DocumentNode | undefined {
    return this.#persisted.get(id);
  }

  /** The persisted id an operation node was registered under. */
  persistedId(operation: OperationDefinitionNode): string | undefined {
    return this.#persistedIds.get(operation);
  }

  /**
   * The document `args` names: a persisted one by id, or `source` parsed
   * and validated through the cache. An error result when there is none.
   */
  document(args: ExecuteArgs): DocumentNode | ExecutionResult {
    let document: DocumentNode | undefined;
    if (args.id !== undefined) {
      document = this.#persisted.get(args.id);
      if (!document) {
        return {
          errors: [new GraphQLError(`unknown persisted operation ${args.id}`)],
        };
      }
    } else if (args.source !== undefined) {
      if (this.#persistedOnly) {
        return {
          errors: [
            new GraphQLError("only persisted operations are accepted", {
              extensions: { code: "PERSISTED_QUERY_ONLY" },
            }),
          ],
        };
      }
      document = this.#documents.get(args.source);
      if (!document) {
        try {
          document = this.#parse(args.source);
        } catch (err) {
          return { errors: [err as GraphQLError] };
        }
        const errors = this.#validate(document);
        if (errors.length > 0) return { errors };
        const bytes = documentBytes(args.source);
        if (bytes <= this.#documentBytesMax) {
          while (
            this.#documents.size > 0 &&
            (this.#documents.size >= DOCUMENT_CACHE_SIZE ||
              this.#documentBytes + bytes > this.#documentBytesMax)
          ) {
            const oldest = this.#documents.keys().next().value!;
            this.#documents.delete(oldest);
            this.#documentBytes -= documentBytes(oldest);
          }
          this.#documents.set(args.source, document);
          this.#documentBytes += bytes;
        }
      }
    } else {
      return {
        errors: [new GraphQLError("a source or an id is needed")],
      };
    }
    return document;
  }
}
