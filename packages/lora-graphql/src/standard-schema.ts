// The Standard Schema interface (https://standardschema.dev), which zod,
// valibot, arktype, yup, joi and others implement. The specification is
// types only and written to be copied, so accepting these validators
// costs no dependency.

/** A validator from any library that implements Standard Schema v1. */
export interface StandardSchemaV1<Input = unknown, Output = Input> {
  readonly "~standard": {
    readonly version: 1;
    readonly vendor: string;
    readonly validate: (
      value: unknown,
    ) => StandardResult<Output> | Promise<StandardResult<Output>>;
    readonly types?:
      | { readonly input: Input; readonly output: Output }
      | undefined;
  };
}

export type StandardResult<Output> =
  | { readonly value: Output; readonly issues?: undefined }
  | { readonly issues: ReadonlyArray<StandardIssue> };

export interface StandardIssue {
  readonly message: string;
  /** Keys into the value; a segment is a key, or an object holding one. */
  readonly path?:
    | ReadonlyArray<PropertyKey | { readonly key: PropertyKey }>
    | undefined;
}

/**
 * The validator of one node type's mutation inputs: one schema for every
 * input, or one per kind of input. An update's input holds only the
 * fields it sets, and an upsert's only the key for certain, so a schema
 * that requires fields belongs under `create`.
 */
export type TypeValidator =
  | StandardSchemaV1
  | {
      create?: StandardSchemaV1;
      update?: StandardSchemaV1;
      upsert?: StandardSchemaV1;
    };

export function isStandardSchema(value: unknown): value is StandardSchemaV1 {
  if (
    value === null ||
    (typeof value !== "object" && typeof value !== "function")
  ) {
    return false;
  }
  const standard = (value as { "~standard"?: unknown })["~standard"];
  return (
    standard !== null &&
    typeof standard === "object" &&
    typeof (standard as { validate?: unknown }).validate === "function"
  );
}
