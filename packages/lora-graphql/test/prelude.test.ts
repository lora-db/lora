import { buildSchema } from "graphql";
import { directiveTypeDefs } from "../src/index.js";

test("the directive prelude is valid SDL", () => {
  expect(() =>
    buildSchema(directiveTypeDefs + "type Query { a: Int }"),
  ).not.toThrow();
});
