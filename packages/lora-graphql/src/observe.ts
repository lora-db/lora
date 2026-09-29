// Observability: an event after every statement, OpenTelemetry-shaped
// spans and a small metrics interface. Tracer and meter are typed
// structurally, so the package does not depend on @opentelemetry/api:
// pass `trace.getTracer("lora-graphql")` and any object with `counter` /
// `histogram` methods.

import type { QueryResult, Statement } from "./driver.js";

export interface StatementEndEvent {
  /** The root field the statements belong to. */
  field: string;
  /** Statements run in one call: reads batch theirs, writes run one by one. */
  statements: Statement[];
  mode: "read" | "write";
  durationMs: number;
  /** Rows returned, over every statement. */
  rows: number;
  /** Set when the statements failed. */
  error?: unknown;
  /** Estimated rows touched, for reads (the `maxCost` estimate). */
  cost?: number;
  operationName?: string;
  /** Id of the persisted operation, when the request used one. */
  persistedId?: string;
}

export interface SpanLike {
  setAttribute(key: string, value: string | number | boolean): unknown;
  setStatus(status: { code: number; message?: string }): unknown;
  recordException(exception: Error | string): unknown;
  end(): void;
}

export interface TracerLike {
  startActiveSpan<T>(name: string, fn: (span: SpanLike) => T): T;
}

export interface MetricsLike {
  counter(name: string, value: number, attributes?: Attributes): void;
  histogram(name: string, value: number, attributes?: Attributes): void;
}

export type Attributes = Record<string, string | number | boolean>;

export interface ObservabilityOptions {
  /** Called after every statement (or batch of read statements). */
  onStatementEnd?: (event: StatementEndEvent) => void;
  /**
   * Emits a `lora.graphql.field` span per root field, containing a
   * `lora.cypher` span per statement call.
   */
  tracer?: TracerLike;
  /** Put the Cypher text in spans as `db.statement`. Default false. */
  traceStatements?: boolean;
  /**
   * Receives `lora.graphql.statements` (counter), `lora.graphql.errors`
   * (counter), `lora.graphql.statement.duration` (ms) and
   * `lora.graphql.cost` (estimated rows) histograms.
   */
  metrics?: MetricsLike;
}

/** What a statement call is part of. */
export interface StatementMeta {
  field: string;
  mode: "read" | "write";
  cost?: number | undefined;
  operationName?: string | undefined;
  persistedId?: string | undefined;
}

/** OpenTelemetry's SpanStatusCode.ERROR. */
const STATUS_ERROR = 2;

export class Observer {
  readonly #options: ObservabilityOptions;

  constructor(options: ObservabilityOptions) {
    this.#options = options;
  }

  get active(): boolean {
    const o = this.#options;
    return !!(o.onStatementEnd || o.tracer || o.metrics);
  }

  /** Run a root field's resolver inside its span. */
  field<T>(
    meta: { field: string; operationName?: string | undefined },
    fn: () => Promise<T>,
  ): Promise<T> {
    const tracer = this.#options.tracer;
    if (!tracer) return fn();
    return tracer.startActiveSpan("lora.graphql.field", async (span) => {
      span.setAttribute("graphql.field.name", meta.field);
      if (meta.operationName) {
        span.setAttribute("graphql.operation.name", meta.operationName);
      }
      try {
        return await fn();
      } catch (err) {
        fail(span, err);
        throw err;
      } finally {
        span.end();
      }
    });
  }

  /** Run statements, timing them and reporting the outcome. */
  statements<T extends QueryResult | QueryResult[]>(
    meta: StatementMeta,
    statements: Statement[],
    run: () => Promise<T>,
  ): Promise<T> {
    if (!this.active) return run();
    const tracer = this.#options.tracer;
    const body = async (span?: SpanLike): Promise<T> => {
      if (span) {
        span.setAttribute("db.system", "loradb");
        span.setAttribute("db.operation.name", meta.mode);
        span.setAttribute("graphql.field.name", meta.field);
        if (this.#options.traceStatements) {
          span.setAttribute(
            "db.statement",
            statements.map((s) => s.text).join(";\n"),
          );
        }
      }
      const start = performance.now();
      let result: T | undefined;
      let error: unknown;
      try {
        result = await run();
        return result;
      } catch (err) {
        error = err;
        if (span) fail(span, err);
        throw err;
      } finally {
        const durationMs = performance.now() - start;
        const rows = result === undefined ? 0 : countRows(result);
        if (span) {
          span.setAttribute("db.response.returned_rows", rows);
          span.end();
        }
        this.#report(meta, statements, durationMs, rows, error);
      }
    };
    return tracer
      ? tracer.startActiveSpan("lora.cypher", (span) => body(span))
      : body();
  }

  #report(
    meta: StatementMeta,
    statements: Statement[],
    durationMs: number,
    rows: number,
    error: unknown,
  ): void {
    const { onStatementEnd, metrics } = this.#options;
    try {
      onStatementEnd?.({
        field: meta.field,
        statements,
        mode: meta.mode,
        durationMs,
        rows,
        ...(error !== undefined ? { error } : {}),
        ...(meta.cost !== undefined ? { cost: meta.cost } : {}),
        ...(meta.operationName ? { operationName: meta.operationName } : {}),
        ...(meta.persistedId ? { persistedId: meta.persistedId } : {}),
      });
      if (metrics) {
        const attributes: Attributes = { field: meta.field, mode: meta.mode };
        if (meta.operationName) attributes["operation"] = meta.operationName;
        metrics.counter(
          "lora.graphql.statements",
          statements.length,
          attributes,
        );
        metrics.histogram(
          "lora.graphql.statement.duration",
          durationMs,
          attributes,
        );
        if (error !== undefined) {
          metrics.counter("lora.graphql.errors", 1, attributes);
        }
        if (meta.cost !== undefined) {
          metrics.histogram("lora.graphql.cost", meta.cost, attributes);
        }
      }
    } catch {
      // Observers must not fail the request.
    }
  }
}

function countRows(result: QueryResult | QueryResult[]): number {
  return Array.isArray(result)
    ? result.reduce((n, r) => n + r.rows.length, 0)
    : result.rows.length;
}

function fail(span: SpanLike, err: unknown): void {
  span.recordException(err instanceof Error ? err : String(err));
  span.setStatus({
    code: STATUS_ERROR,
    message: err instanceof Error ? err.message : String(err),
  });
}
