import { version } from "graphql";

// Guards the graphql 17 run: if the alias stops applying, the suite would
// silently test 16 twice.
test("the suite runs on the graphql major it was asked for", () => {
  const wanted = process.env["LORA_GRAPHQL_VERSION"] ?? "16";
  expect(version.split(".")[0]).toBe(wanted);
});
