// How an error thrown while a root field runs reaches the client: mapped
// to a request error where the package knows the cause, reported to
// `onError` and masked (see `maskErrors`) when it is the database's own.

import { GraphQLError } from "graphql";
import { requestError } from "../errors.js";
import { mapWriteError } from "../execute/mutate/errors.js";
import type { GraphModel } from "../model/types.js";
import type { DatabaseErrorEvent } from "../options.js";

export interface DatabaseErrorEnv {
  model: GraphModel;
  /** Hide database error details from clients (the `maskErrors` option). */
  maskErrors: boolean;
  onError: ((event: DatabaseErrorEvent) => void) | undefined;
}

/** The error to throw for `err`, raised while `field` ran. */
export function databaseError(
  env: DatabaseErrorEnv,
  field: string,
  err: unknown,
): unknown {
  let error: unknown;
  if (err instanceof GraphQLError) error = err;
  else {
    const mapped = mapWriteError(env.model, err);
    if (mapped !== err) error = mapped;
    else if ((err as { code?: string }).code === "LORA_INVALID_VECTOR") {
      error = requestError(
        "BAD_USER_INPUT",
        err instanceof Error ? err.message : String(err),
        err,
      );
    } else {
      error = requestError(
        "DATABASE_ERROR",
        err instanceof Error ? err.message : String(err),
        err,
      );
    }
  }
  if (!(error instanceof GraphQLError)) return error;
  if (error.extensions["code"] === "DATABASE_ERROR") {
    const id = globalThis.crypto.randomUUID();
    try {
      env.onError?.({
        id,
        field,
        message: error.message,
        error: error.originalError ?? err,
      });
    } catch {
      // A failing error hook must not replace the request's error.
    }
    return env.maskErrors
      ? new GraphQLError(`database error (id ${id})`, {
          extensions: { code: "DATABASE_ERROR", id },
        })
      : new GraphQLError(error.message, {
          extensions: { ...error.extensions, id },
          originalError: error.originalError ?? undefined,
        });
  }
  // Errors the package wrote itself are safe to show; masked, they
  // lose the engine error they were mapped from.
  return env.maskErrors && error.originalError
    ? new GraphQLError(error.message, { extensions: error.extensions })
    : error;
}
