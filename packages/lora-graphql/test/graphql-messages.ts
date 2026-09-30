import { versionInfo } from "graphql";

/** graphql-js's message for an unknown input object field, on 16 or 17. */
export function unknownField(field: string, type: string): RegExp {
  return versionInfo.major >= 17
    ? new RegExp(
        `Expected value of type "${type}" not to include unknown field "${field}"`,
      )
    : new RegExp(`"${field}" is not defined by type "${type}"`);
}
