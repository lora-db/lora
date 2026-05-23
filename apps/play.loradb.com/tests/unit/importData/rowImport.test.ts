import { describe, expect, it } from "vitest";

import {
  applySmartDefaults,
  buildPreview,
  detectRowFormat,
  effectiveBatchSize,
  renderMappingTemplate,
  wrapStream,
  type MappingKind,
  type PreviewState,
} from "@/lib/importData/rowImport";

function textFile(name: string, body: string): File {
  return new File([body], name, { type: "text/plain" });
}

async function streamText(stream: ReadableStream<Uint8Array>): Promise<string> {
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  for (;;) {
    const next = await reader.read();
    if (next.done) break;
    chunks.push(next.value);
  }
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return new TextDecoder().decode(bytes);
}

describe("row import helpers", () => {
  it("detects supported formats from filenames", () => {
    expect(detectRowFormat("users.csv")).toBe("csv");
    expect(detectRowFormat("events.ndjson")).toBe("jsonl");
    expect(detectRowFormat("events.JSONL")).toBe("jsonl");
    expect(detectRowFormat("snapshot.lorasnap")).toBeNull();
  });

  it("builds a CSV preview with normalized schema-marker headers", async () => {
    const { preview, format } = await buildPreview(
      textFile(
        "users.csv",
        ":ID,:LABEL,name:string\n1,User,Alice\n2,User,Bob\n",
      ),
    );

    expect(format).toBe("csv");
    expect(preview.columns).toEqual(["_id", "_label", "name"]);
    expect(preview.sample).toEqual([
      { _id: "1", _label: "User", name: "Alice" },
      { _id: "2", _label: "User", name: "Bob" },
    ]);
    expect(preview.estimatedRows).toBe(2);
  });

  it("sniffs JSONL without an extension", async () => {
    const { preview, format } = await buildPreview(
      textFile("rows", '{"name":"Alice","age":30}\n{"name":"Bob"}\n'),
    );

    expect(format).toBe("jsonl");
    expect(preview.columns).toEqual(["name", "age"]);
    expect(preview.parsedSampleRows).toBe(2);
  });

  it("rejects JSONL rows that are not objects", async () => {
    await expect(
      buildPreview(textFile("rows.jsonl", '{"name":"Alice"}\nnull\n')),
    ).rejects.toThrow("expected JSON object on line 2");
  });

  it("rejects JSON arrays that do not contain object rows", async () => {
    await expect(
      buildPreview(textFile("rows.json", '[{"name":"Alice"}, 42]')),
    ).rejects.toThrow("expected JSON object at array index 1");
  });

  it("applies smart defaults for relationship-ish CSV previews", () => {
    const calls = makeDefaultSetters();
    const preview: PreviewState = {
      columns: ["_start_id", "_end_id", "_type", "since", "weight"],
      sample: [{ _type: "FOLLOWS" }],
      estimatedRows: 10,
      parsedSampleRows: 2,
    };

    applySmartDefaults("relationships.csv", preview, calls.setters);

    expect(calls.state.mappingKind).toBe("relationship");
    expect(calls.state.relType).toBe("FOLLOWS");
    expect(calls.state.startColumn).toBe("_start_id");
    expect(calls.state.endColumn).toBe("_end_id");
    expect(calls.state.propertyColumns).toEqual(["since", "weight"]);
  });

  it("rewrites only the CSV header when column type overrides are set", async () => {
    const file = textFile("users.csv", "name,age\nAlice,30\nBob,25\n");
    const rewritten = await streamText(wrapStream(file, "csv", { age: "int" }));

    expect(rewritten).toBe("name,age:int\nAlice,30\nBob,25\n");
  });

  it("preserves CRLF line endings when rewriting the CSV header", async () => {
    const file = textFile("users.csv", "name,age\r\nAlice,30\r\nBob,25\r\n");
    const rewritten = await streamText(wrapStream(file, "csv", { age: "int" }));

    expect(rewritten).toBe("name,age:int\r\nAlice,30\r\nBob,25\r\n");
  });

  it("renders preview Cypher for node and relationship mappings", () => {
    expect(
      renderMappingTemplate({
        kind: "node",
        label: "User",
        id_column: "id",
        id_property: "uid",
        properties: [
          { source: "id", property: "id" },
          { source: "name", property: "name" },
        ],
      }),
    ).toBe("UNWIND $rows AS r CREATE (:User {uid: r.id, name: r.name})");

    expect(
      renderMappingTemplate({
        kind: "relationship",
        rel_type: "FOLLOWS",
        start_label: "User",
        start_column: "src",
        start_match_property: "uid",
        end_label: "User",
        end_column: "dst",
        end_match_property: "uid",
        properties: [{ source: "since", property: "since" }],
      }),
    ).toContain("CREATE (a)-[:FOLLOWS {since: r.since}]->(b)");
  });

  // CSV headers like `User Id` aren't valid Cypher identifiers — the
  // preview must show the backtick form the engine actually runs, so a
  // copy-paste-into-the-workbench round trip stays honest.
  it("backtick-quotes property names that aren't simple identifiers", () => {
    expect(
      renderMappingTemplate({
        kind: "node",
        label: "People100000",
        id_column: null,
        id_property: null,
        properties: [
          { source: "User Id", property: "User Id" },
          { source: "First Name", property: "First Name" },
          { source: "Email", property: "Email" },
        ],
      }),
    ).toBe(
      "UNWIND $rows AS r CREATE (:People100000 " +
        "{`User Id`: r.`User Id`, `First Name`: r.`First Name`, Email: r.Email})",
    );
  });

  it("normalizes invalid batch sizes to the default", () => {
    expect(effectiveBatchSize("")).toBe(1_000);
    expect(effectiveBatchSize(0)).toBe(1_000);
    expect(effectiveBatchSize(250)).toBe(250);
  });

  // Regression: Excel and Google Sheets prepend a UTF-8 BOM (`﻿`)
  // on save. Without the strip, the first column name parses as
  // `﻿name` and smart-defaults + Cypher `r.name` lookups all miss.
  it("strips a UTF-8 BOM from the first CSV column name", async () => {
    const { preview } = await buildPreview(
      textFile("users.csv", "﻿name,age\nAlice,30\nBob,25\n"),
    );
    expect(preview.columns).toEqual(["name", "age"]);
    expect(preview.sample[0]).toEqual({ name: "Alice", age: "30" });
  });

  // Regression: the previous sniffer did `text.split(/\r?\n/)` and then
  // re-split each line for cells, so any quoted cell with an embedded
  // newline shattered the preview's column count. The Rust decoder
  // handled it correctly, so users saw "preview broken, import works."
  it("keeps quoted newlines attached to their CSV record in the preview", async () => {
    const csv =
      "name,note\n" + '"Alice","line one\nline two"\n' + '"Bob","plain"\n';
    const { preview } = await buildPreview(textFile("notes.csv", csv));
    expect(preview.columns).toEqual(["name", "note"]);
    expect(preview.sample).toEqual([
      { name: "Alice", note: "line one\nline two" },
      { name: "Bob", note: "plain" },
    ]);
  });

  // Regression: the rewriter joined cells with bare `,` and never
  // re-quoted them, so a header with a comma round-tripped to broken
  // CSV. Re-encoding through the RFC-4180 helper keeps the output
  // valid even when the type override changes a cell's quoting needs.
  it("re-quotes rewritten CSV header cells that need RFC-4180 escaping", async () => {
    const file = textFile("users.csv", '"first, last",age\nAlice Smith,30\n');
    const rewritten = await streamText(wrapStream(file, "csv", { age: "int" }));
    expect(rewritten.startsWith('"first, last",age:int\n')).toBe(true);
  });

  it("strips a UTF-8 BOM from the CSV header when rewriting overrides", async () => {
    const file = textFile("users.csv", "﻿name,age\nAlice,30\n");
    const rewritten = await streamText(wrapStream(file, "csv", { age: "int" }));
    expect(rewritten.startsWith("name,age:int\n")).toBe(true);
  });
});

function makeDefaultSetters(): {
  state: {
    label: string;
    mappingKind: MappingKind;
    relType: string;
    idColumn: string | null;
    startColumn: string | null;
    endColumn: string | null;
    propertyColumns: string[];
  };
  setters: Parameters<typeof applySmartDefaults>[2];
} {
  const state = {
    label: "",
    mappingKind: "node" as MappingKind,
    relType: "",
    idColumn: null as string | null,
    startColumn: null as string | null,
    endColumn: null as string | null,
    propertyColumns: [] as string[],
  };
  return {
    state,
    setters: {
      setLabel: (s) => {
        state.label = s;
      },
      setMappingKind: (k) => {
        state.mappingKind = k;
      },
      setRelType: (s) => {
        state.relType = s;
      },
      setIdColumn: (s) => {
        state.idColumn = s;
      },
      setStartColumn: (s) => {
        state.startColumn = s;
      },
      setEndColumn: (s) => {
        state.endColumn = s;
      },
      setStartLabel: () => {},
      setEndLabel: () => {},
      setPropertyColumns: (s) => {
        state.propertyColumns = s;
      },
      setColumnTypes: () => {},
    },
  };
}
