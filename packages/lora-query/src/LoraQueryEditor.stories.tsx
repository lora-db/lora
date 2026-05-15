import { useMemo, useRef, useState } from "react";
import type { Meta, StoryObj } from "@storybook/react";
import {
  LoraQueryEditor,
  type LoraQueryEditorHandle,
} from "./LoraQueryEditor";
import { useLoraQueryStatus } from "./useLoraQueryStatus";
import { darkTheme, lightTheme } from "./themes";
import type { PropertyContext } from "./cypher/providers";
import { formatSync } from "./parser";

/**
 * Pre-prettify a predefined query so the editor mounts already showing
 * its prettified form — no flash from raw to formatted on first render.
 * The WASM `format()` is synchronous under the hood; `formatSync` is a
 * straight wrapper, so this is cheap and runs once per story load.
 */
const pretty = (q: string) => formatSync(q);

// ─── Shared demo schema ───────────────────────────────────────────
// Every story passes these in so the example query inside the
// editor has working `:Label` and `{ property }` autocompletion.

const DEMO_LABELS = ["Person", "Company", "Movie", "User"] as const;

const DEMO_REL_TYPES = [
  "KNOWS",
  "WORKS_AT",
  "EMPLOYS",
  "ACTED_IN",
  "DIRECTED",
  "RATED",
  "FOLLOWS",
] as const;

/** Node + relationship property schema covering every query in this file. */
const DEMO_PROPERTY_SCHEMA: Record<string, readonly string[]> = {
  // Node labels
  Person: [
    "name",
    "age",
    "email",
    "role",
    "active",
    "archived",
    "createdAt",
    "lastSeen",
  ],
  Company: ["name", "industry", "founded"],
  Movie: ["title", "year", "rating", "genre"],
  User: ["name", "email", "createdAt"],
  // Relationship types
  KNOWS: ["since"],
  WORKS_AT: ["since", "role"],
  EMPLOYS: ["since"],
  ACTED_IN: ["role"],
  DIRECTED: [],
  RATED: ["score", "at"],
  FOLLOWS: ["since", "weight"],
};

const DEMO_GET_PROPERTY_KEYS = (ctx: PropertyContext): readonly string[] => {
  if (!ctx.label) return [];
  return DEMO_PROPERTY_SCHEMA[ctx.label] ?? [];
};

const meta: Meta<typeof LoraQueryEditor> = {
  title: "Editor/LoraQueryEditor",
  component: LoraQueryEditor,
  parameters: { layout: "padded" },
  tags: ["autodocs"],
};

export default meta;

type Story = StoryObj<typeof LoraQueryEditor>;

/**
 * Reusable Prettify toolbar — every story renders one so the host
 * pattern (call `ref.current.prettify()`) is obvious. Style is plain
 * and minimal so it doesn't compete with the editor's own theming.
 */
function StoryToolbar({
  editorRef,
  extras,
}: {
  editorRef: React.RefObject<LoraQueryEditorHandle | null>;
  extras?: React.ReactNode;
}) {
  return (
    <div
      style={{
        display: "flex",
        gap: 6,
        alignItems: "center",
        fontSize: 12,
      }}
    >
      <button
        type="button"
        onClick={() => void editorRef.current?.prettify()}
        title="Reformat the buffer (⌘⇧F)"
        style={{
          padding: "4px 10px",
          borderRadius: 4,
          border: "1px solid #d0d7de",
          background: "#ffffff",
          cursor: "pointer",
        }}
      >
        Prettify
      </button>
      {extras}
    </div>
  );
}

function Controlled({
  initial,
  readOnly = false,
}: {
  initial: string;
  readOnly?: boolean;
}) {
  const editorRef = useRef<LoraQueryEditorHandle>(null);
  const [value, setValue] = useState(() => pretty(initial));
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <StoryToolbar editorRef={editorRef} />
      <LoraQueryEditor
        ref={editorRef}
        value={value}
        onChange={setValue}
        readOnly={readOnly}
        labels={DEMO_LABELS}
        relTypes={DEMO_REL_TYPES}
        getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
      />
      <pre
        style={{
          background: "#f5f5f5",
          padding: 8,
          borderRadius: 4,
          fontSize: 12,
          whiteSpace: "pre-wrap",
          margin: 0,
        }}
      >
        {value}
      </pre>
    </div>
  );
}

export const Default: Story = {
  render: () => <Controlled initial="MATCH (n) RETURN n" />,
};

export const MultiLine: Story = {
  render: () => (
    <Controlled
      initial={`MATCH (a:Person)-[:KNOWS]->(b)
WHERE a.name = 'Alice'
RETURN a, b
ORDER BY b.name
LIMIT 10`}
    />
  ),
};

export const WithSyntaxError: Story = {
  render: () => <Controlled initial="MATCH (n)\nWHERE a.name = 'Alice" />,
};

export const ReadOnly: Story = {
  render: () => <Controlled initial="MATCH (n) RETURN n.name" readOnly />,
};

export const RichGraphPattern: Story = {
  render: () => (
    <Controlled
      initial={`MATCH (alice:Person {name: 'Alice'})-[r:KNOWS*1..3]->(friend)
WHERE friend.age > 21 AND NOT friend.archived
WITH alice, friend, count(r) AS hops
RETURN friend.name AS name, hops
ORDER BY hops ASC
LIMIT 5`}
    />
  ),
};

export const WithSchemaProviders: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`MATCH (n:Person {name: 'Alice'})-[r:KNOWS]->(m)\nRETURN n, m`),
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
        <p style={{ margin: 0, fontSize: 12, color: "#6e7781" }}>
          Try typing <code>:</code> after <code>(n</code> to see label
          suggestions, or <code>{"{"}</code> inside <code>(n:Person ...)</code> to
          see schema-driven property keys.
        </p>
      </div>
    );
  },
};

/**
 * Forced dark via the `colorScheme` prop — no JS palette required.
 * The CSS-variable layer flips on `data-color-scheme="dark"`.
 */
export const DarkTheme: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`MATCH (alice:Person {name: 'Alice'})-[r:KNOWS]->(friend)\nWHERE friend.age > 21\nRETURN friend.name, count(r) AS hops`),
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          colorScheme="dark"
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * Status hook — `useLoraQueryStatus()` bundles every callback into a
 * single state object the host can render.
 */
export const StatusHook: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`MATCH (alice:Person {name: 'Alice'})-[r:KNOWS]->(friend)\nWHERE friend.age > $minAge AND friend.name <> $excluded\nRETURN friend.name AS name, count(r) AS hops`),
    );
    const [status, statusProps] = useLoraQueryStatus();
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
          {...statusProps}
        />
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(3, 1fr)",
            gap: 8,
            fontSize: 12,
            fontFamily: "ui-monospace, monospace",
            background: "#f5f5f5",
            padding: 10,
            borderRadius: 4,
          }}
        >
          <div>
            <strong>Errors</strong> ({status.errors.length})
            <pre style={{ margin: 0 }}>
              {status.errors.map((e) => `• ${e.message}`).join("\n") || "—"}
            </pre>
          </div>
          <div>
            <strong>Warnings</strong> ({status.warnings.length})
            <pre style={{ margin: 0 }}>
              {status.warnings.map((w) => `• ${w.message}`).join("\n") || "—"}
            </pre>
          </div>
          <div>
            <strong>Variables</strong>
            <pre style={{ margin: 0 }}>
              {status.variables
                .map((v) => `• ${v.name}${v.label ? `:${v.label}` : ""}`)
                .join("\n") || "—"}
            </pre>
            <strong style={{ marginTop: 6, display: "inline-block" }}>
              Parameters
            </strong>
            <pre style={{ margin: 0 }}>
              {status.parameters.map((p) => `• $${p}`).join("\n") || "—"}
            </pre>
          </div>
        </div>
      </div>
    );
  },
};

/**
 * `onRun` callback wired to a mock query executor — Cmd/Ctrl + Enter
 * "runs" the current source.
 */
export const RunCallback: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`MATCH (p:Person)-[:KNOWS]->(f)\nWHERE p.name = 'Alice'\nRETURN f.name AS friend\nLIMIT 5`),
    );
    const [runs, setRuns] = useState<
      Array<{ at: number; source: string }>
    >([]);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar
          editorRef={editorRef}
          extras={
            <button
              type="button"
              onClick={() => editorRef.current?.run()}
              title="Trigger onRun"
              style={{
                padding: "4px 10px",
                borderRadius: 4,
                border: "1px solid #d0d7de",
                background: "#ffffff",
                cursor: "pointer",
              }}
            >
              Run
            </button>
          }
        />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
          onRun={(source) =>
            setRuns((r) => [
              { at: Date.now(), source },
              ...r,
            ].slice(0, 5))
          }
        />
        <p style={{ margin: 0, fontSize: 12, color: "#6e7781" }}>
          Press <kbd>⌘/Ctrl + Enter</kbd> to fire <code>onRun</code>.
        </p>
        <ol
          style={{
            margin: 0,
            paddingLeft: 18,
            fontSize: 12,
            fontFamily: "ui-monospace, monospace",
          }}
        >
          {runs.map((r) => (
            <li key={r.at}>
              <code>{new Date(r.at).toLocaleTimeString()}</code> —{" "}
              {r.source.split("\n")[0]?.slice(0, 60)}
              {r.source.length > 60 ? "…" : ""}
            </li>
          ))}
          {runs.length === 0 && (
            <li style={{ listStyle: "none", color: "#9d9d9d" }}>(no runs yet)</li>
          )}
        </ol>
      </div>
    );
  },
};

/**
 * Light theme preset (matches the CSS defaults shipped in
 * `editor.css`).
 */
export const LightThemePreset: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`MATCH (n:Movie)-[:ACTED_IN]-(actor)\nWHERE n.year > 2000\nRETURN n.title AS movie, collect(actor.name) AS cast\nORDER BY movie\nLIMIT 10`),
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          theme={lightTheme}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * Dark theme preset.
 */
export const DarkThemePreset: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`MATCH (m:Movie)<-[r:RATED]-(u:User)\nWHERE r.score >= 8\nWITH m, avg(r.score) AS avgScore, count(r) AS votes\nWHERE votes > 100\nRETURN m.title, avgScore, votes\nORDER BY avgScore DESC, votes DESC\nLIMIT 25`),
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          theme={darkTheme}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * Solarized-style custom theme — shows how the `theme` prop can build
 * any palette on top of the CSS variables.
 */
export const SolarizedTheme: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`MATCH (c:Company)-[:EMPLOYS]->(p:Person)\nWHERE c.industry = 'Tech' AND p.role STARTS WITH 'Senior'\nRETURN c.name AS company, p.name AS employee\nORDER BY company\nLIMIT 20`),
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          theme={{
          background: "#fdf6e3",
          foreground: "#586e75",
          border: "#eee8d5",
          accent: "#268bd2",
          muted: "#93a1a1",
          activeLine: "#eee8d5",
          gutterBackground: "#fdf6e3",
          gutterForeground: "#93a1a1",
          cursor: "#dc322f",
          selectionBackground: "rgba(38, 139, 210, 0.18)",
          keyword: "#859900",
          variable: "#268bd2",
          parameter: "#6c71c4",
          label: "#b58900",
          relType: "#cb4b16",
          property: "#d33682",
          functionName: "#2aa198",
          namespace: "#93a1a1",
          string: "#2aa198",
          number: "#dc322f",
          bool: "#cb4b16",
          null: "#cb4b16",
          popupBackground: "#fdf6e3",
          popupForeground: "#586e75",
          popupBorder: "#93a1a1",
          popupSelectedBackground: "#268bd2",
          popupSelectedForeground: "#fdf6e3",
        }}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * Procedures provider — `CALL ` lists known stored procedures with
 * their signatures.
 */
export const ProcedureCompletion: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`// Type "CALL " and pick a procedure from the popup.\nCALL `),
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
          procedures={[
            {
              name: "db.indexes",
              signature: "db.indexes() :: (name, state, type, …)",
              info: "List every index in the database.",
            },
            {
              name: "db.constraints",
              signature: "db.constraints() :: (name, type, entity, …)",
              info: "List every constraint.",
            },
            {
              name: "db.labels",
              signature: "db.labels() :: (label)",
              info: "All distinct node labels.",
            },
            {
              name: "db.relationshipTypes",
              signature: "db.relationshipTypes() :: (relationshipType)",
              info: "All distinct relationship types.",
            },
            {
              name: "db.propertyKeys",
              signature: "db.propertyKeys() :: (propertyKey)",
              info: "All known property keys.",
            },
          ]}
        />
      </div>
    );
  },
};

/**
 * Common Cypher patterns — long enough to exercise code folding,
 * scrolling, and the semantic colouring.
 */
export const ComplexExample: Story = {
  render: () => {
    const [value, setValue] = useState(
      pretty(`// Find the top 5 most-influential users who follow Alice transitively,
// scored by an aggregate over their second-degree connections.
MATCH (alice:Person {name: 'Alice'})
MATCH path = (alice)-[:FOLLOWS*1..3]->(follower:Person)
WHERE follower.active = TRUE
  AND follower.createdAt > datetime('2024-01-01T00:00:00Z')

WITH alice, follower, length(path) AS distance

OPTIONAL MATCH (follower)-[:FOLLOWS]->(fof:Person)
WHERE fof <> alice

WITH alice, follower, distance, count(DISTINCT fof) AS reach

WITH follower, reach, distance,
     reach * 1.0 / (distance * distance) AS influenceScore
ORDER BY influenceScore DESC
LIMIT 5

RETURN follower.name AS user,
       reach,
       distance,
       influenceScore`),
    );
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * Showcases CREATE / MERGE with ON CREATE SET — write-side queries.
 */
export const WriteSideQueries: Story = {
  render: () => {
    const [value, setValue] = useState(
      pretty(`// 1. Idempotent upsert with conditional initialisation.
MERGE (n:Person {email: $email})
ON CREATE SET n.name = $name, n.createdAt = timestamp()
ON MATCH  SET n.lastSeen = timestamp()
RETURN n

// 2. Bulk import via UNWIND.
UNWIND $rows AS row
MERGE (p:Person {email: row.email})
SET p += row
RETURN count(p) AS created

// 3. Bidirectional relationship.
MATCH (a:Person {email: $a}), (b:Person {email: $b})
MERGE (a)-[r:KNOWS]->(b)
ON CREATE SET r.since = date()
RETURN r`),
    );
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * DDL commands — index + constraint syntax.
 */
export const SchemaCommands: Story = {
  render: () => {
    const [value, setValue] = useState(
      pretty(`// Index a frequently-filtered property.
CREATE INDEX person_email FOR (n:Person) ON (n.email)

// Composite index for combined filters.
CREATE INDEX person_active_role FOR (n:Person) ON (n.active, n.role)

// Uniqueness constraint (implies an index).
CREATE CONSTRAINT person_email_unique
FOR (n:Person)
REQUIRE n.email IS UNIQUE

// Existence constraint.
CREATE CONSTRAINT person_created_present
FOR (n:Person)
REQUIRE n.createdAt IS NOT NULL

// Inspect the current schema.
SHOW INDEXES
SHOW CONSTRAINTS`),
    );
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * EXPLAIN / PROFILE prefixes for query analysis.
 */
export const ExplainProfile: Story = {
  render: () => {
    const [value, setValue] = useState(
      pretty(`// EXPLAIN compiles the query and shows the plan without executing.
EXPLAIN
MATCH (alice:Person {name: 'Alice'})-[:KNOWS*1..2]->(friend)
RETURN friend.name AS friend

// PROFILE actually runs it and adds row-counts + db-hits per step.
PROFILE
MATCH (m:Movie {year: 2023})
RETURN m.title
LIMIT 50`),
    );
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * CASE expressions for inline branching in projections.
 */
export const CaseExpression: Story = {
  render: () => {
    const [value, setValue] = useState(
      pretty(`MATCH (n:Person)
RETURN n.name AS name,
       CASE
         WHEN n.age < 18 THEN 'minor'
         WHEN n.age < 65 THEN 'adult'
         ELSE 'senior'
       END AS bracket,
       CASE n.role
         WHEN 'admin'   THEN 'red'
         WHEN 'editor'  THEN 'orange'
         ELSE                'gray'
       END AS roleColor`),
    );
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * Subqueries with CALL { … } and list comprehensions.
 */
export const Subqueries: Story = {
  render: () => {
    const [value, setValue] = useState(
      pretty(`MATCH (user:Person {name: $user})

CALL {
  WITH user
  MATCH (user)-[:RATED]->(m:Movie)
  RETURN collect(m.genre) AS preferredGenres
}

MATCH (rec:Movie)
WHERE rec.genre IN preferredGenres
  AND NOT EXISTS { (user)-[:RATED]->(rec) }

WITH rec, [g IN rec.genre WHERE g IN preferredGenres | g] AS overlap
RETURN rec.title, size(overlap) AS score
ORDER BY score DESC
LIMIT 10`),
    );
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * Compact preview that combines several features at once — useful for
 * embedding in a landing page or readme screenshot.
 */
export const Showcase: Story = {
  render: () => {
    const [value, setValue] = useState(
      pretty(`MATCH (you:Person {name: $userName})
MATCH (you)-[:KNOWS*2..3]->(suggestion:Person)
WHERE NOT (you)-[:KNOWS]->(suggestion)
  AND suggestion.archived = FALSE
WITH suggestion, count(*) AS mutualFriends
ORDER BY mutualFriends DESC
LIMIT 10
RETURN suggestion.name AS name,
       suggestion.email AS email,
       mutualFriends`),
    );
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [status, statusProps] = useLoraQueryStatus();
    const summary = useMemo(() => {
      const parts: string[] = [];
      if (status.variables.length > 0)
        parts.push(`${status.variables.length} variables`);
      if (status.parameters.length > 0)
        parts.push(`${status.parameters.length} params`);
      if (status.errors.length > 0)
        parts.push(`${status.errors.length} errors`);
      else if (status.warnings.length > 0)
        parts.push(`${status.warnings.length} warnings`);
      else parts.push("no issues");
      return parts.join(" • ");
    }, [status]);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          theme={darkTheme}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
          {...statusProps}
        />
        <div
          style={{
            fontSize: 12,
            color: "#6e7781",
            fontFamily: "ui-sans-serif, system-ui, sans-serif",
          }}
        >
          {summary}
        </div>
      </div>
    );
  },
};

/**
 * The editor fills its container 100% × 100%. Wrap it in a host element
 * with whatever bounds you want — the editor stretches to fit.
 */
export const FullBleedHeight: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`MATCH (n:Person)-[:KNOWS]->(m)\nRETURN n.name, m.name\nLIMIT 10`),
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <div
          style={{
            // Host-controlled bounds — editor fills this box exactly.
            width: "100%",
            height: 400,
            border: "1px dashed #aaa",
            padding: 12,
            boxSizing: "border-box",
          }}
        >
          <LoraQueryEditor
            ref={editorRef}
            value={value}
            onChange={setValue}
            labels={DEMO_LABELS}
            relTypes={DEMO_REL_TYPES}
            getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
          />
        </div>
        <p style={{ margin: 0, fontSize: 12, color: "#6e7781" }}>
          The dashed box is 400 px tall. The editor stretches to fill it.
        </p>
      </div>
    );
  },
};

/**
 * `showLineNumbers={false}` hides the gutter — useful for compact
 * embeds (sidebar widgets, inline doc snippets, etc.).
 */
export const LineNumbersOff: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`MATCH (n) RETURN n LIMIT 1`),
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          showLineNumbers={false}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * `placeholder` renders ghost text when the buffer is empty.
 */
export const PlaceholderProp: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState("");
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          placeholder="Write a Cypher query — e.g. MATCH (n:Person) RETURN n LIMIT 10"
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
      </div>
    );
  },
};

/**
 * Multi-statement script — each top-level `;` becomes a fold anchor,
 * so the gutter lets you collapse / expand whole queries.
 */
export const MultiStatementFolds: Story = {
  render: () => {
    const editorRef = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`// Three queries — fold each one from the gutter.
MATCH (alice:Person {name: 'Alice'})
RETURN alice.email, alice.createdAt
LIMIT 1;

MATCH (a:Person)-[r:KNOWS]->(b:Person)
WHERE a.active AND b.active
RETURN a.name, b.name, r.since
ORDER BY r.since DESC
LIMIT 25;

MATCH (m:Movie)<-[v:RATED]-(u:User)
WITH m, avg(v.score) AS avgScore, count(v) AS votes
WHERE votes > 100
RETURN m.title, avgScore, votes
ORDER BY avgScore DESC, votes DESC
LIMIT 10;`),
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraQueryEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
        <p style={{ margin: 0, fontSize: 12, color: "#6e7781" }}>
          Click the chevron in the gutter on a query's first line to fold it.
        </p>
      </div>
    );
  },
};

/**
 * Host-driven Prettify via the imperative handle. There is no built-in
 * toolbar — the host renders its own buttons and calls
 * `ref.current.prettify()`.
 */
export const ImperativeHandle: Story = {
  render: () => {
    const ref = useRef<LoraQueryEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(`match (n) where n.age > 18 return n order by n.name limit 5`),
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <div style={{ display: "flex", gap: 8 }}>
          <button onClick={() => void ref.current?.prettify()}>
            Prettify
          </button>
          <button
            onClick={async () => {
              const errs = await ref.current?.validate();
              // eslint-disable-next-line no-console
              console.log("diagnostics", errs);
            }}
          >
            Validate (log to console)
          </button>
          <button
            onClick={() =>
              ref.current?.setValue("MATCH (n:Person) RETURN n LIMIT 10")
            }
          >
            Replace
          </button>
        </div>
        <LoraQueryEditor
          ref={ref}
          value={value}
          onChange={setValue}
          labels={DEMO_LABELS}
          relTypes={DEMO_REL_TYPES}
          getPropertyKeys={DEMO_GET_PROPERTY_KEYS}
        />
        <pre
          style={{
            background: "#f5f5f5",
            padding: 8,
            borderRadius: 4,
            fontSize: 12,
            whiteSpace: "pre-wrap",
            margin: 0,
          }}
        >
          {value}
        </pre>
      </div>
    );
  },
};
