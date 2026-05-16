// Tiny universal accessor resolver. Maps three input shapes to a
// single callable:
//   - function → returned as-is
//   - string   → property-lookup closure
//   - other    → constant-returning closure
//
// LORA: internalised from `accessor-fn` (MIT, © Vasco Asturiano).
// Originally one tiny module; replicated here so we no longer pull
// it as a runtime dep.

export type Accessor<T> = T | string | ((obj: unknown) => T);

export default function accessorFn<T = unknown>(
  param: Accessor<T> | null | undefined,
): (obj: unknown) => T {
  if (typeof param === "function") {
    return param as (obj: unknown) => T;
  }
  if (typeof param === "string") {
    const key = param;
    return (obj: unknown) => (obj as Record<string, T>)?.[key] as T;
  }
  const constant = param as T;
  return () => constant;
}
