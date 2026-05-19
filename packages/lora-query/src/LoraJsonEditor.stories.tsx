import { useMemo, useRef, useState } from "react";
import type { Meta, StoryObj } from "@storybook/react";
import {
  LoraJsonEditor,
  type LoraJsonEditorHandle,
} from "./LoraJsonEditor";
import { useLoraJsonStatus } from "./useLoraJsonStatus";
import { darkJsonTheme, lightJsonTheme } from "./jsonThemes";
import { formatJson } from "./json/format";

/** Pre-prettify a literal so the editor mounts already formatted. */
const pretty = (s: string) => formatJson(s);

const meta: Meta<typeof LoraJsonEditor> = {
  title: "Editor/LoraJsonEditor",
  component: LoraJsonEditor,
  parameters: { layout: "padded" },
  tags: ["autodocs"],
};

export default meta;

type Story = StoryObj<typeof LoraJsonEditor>;

function StoryToolbar({
  editorRef,
  extras,
}: {
  editorRef: React.RefObject<LoraJsonEditorHandle | null>;
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
      <button
        type="button"
        onClick={() => void editorRef.current?.minify()}
        title="Minify the buffer"
        style={{
          padding: "4px 10px",
          borderRadius: 4,
          border: "1px solid #d0d7de",
          background: "#ffffff",
          cursor: "pointer",
        }}
      >
        Minify
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
  const editorRef = useRef<LoraJsonEditorHandle>(null);
  const [value, setValue] = useState(() => pretty(initial));
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <StoryToolbar editorRef={editorRef} />
      <LoraJsonEditor
        ref={editorRef}
        value={value}
        onChange={setValue}
        readOnly={readOnly}
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

const DEMO_PAYLOAD = `{
  "userId": "alice",
  "active": true,
  "tags": ["admin", "editor"],
  "limits": { "rate": 100, "burst": 250 },
  "lastSeen": null
}`;

export const Default: Story = {
  render: () => <Controlled initial={DEMO_PAYLOAD} />,
};

export const ReadOnlyResultsViewer: Story = {
  render: () => (
    <Controlled
      readOnly
      initial={`[
  { "name": "Alice", "age": 32 },
  { "name": "Bob",   "age": 47 },
  { "name": "Carol", "age": 28 }
]`}
    />
  ),
};

export const Invalid: Story = {
  render: () => <Controlled initial={`{ "userId": "alice", "active": true,`} />,
};

export const DarkColorScheme: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(pretty(DEMO_PAYLOAD));
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          colorScheme="dark"
        />
      </div>
    );
  },
};

export const LightThemePreset: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(pretty(DEMO_PAYLOAD));
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          theme={lightJsonTheme}
        />
      </div>
    );
  },
};

export const DarkThemePreset: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(pretty(DEMO_PAYLOAD));
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          theme={darkJsonTheme}
        />
      </div>
    );
  },
};

export const SolarizedTheme: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(pretty(DEMO_PAYLOAD));
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
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
            key: "#d33682",
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
        />
      </div>
    );
  },
};

export const WithKnownKeys: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(`{\n  "\n}`);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          knownKeys={["userId", "minAge", "cap", "includeArchived"]}
        />
        <p style={{ margin: 0, fontSize: 12, color: "#6e7781" }}>
          Place the caret between the braces and trigger autocomplete
          (<kbd>Ctrl+Space</kbd>) — the suggestions come from the
          <code> knownKeys</code> prop and would normally match the
          <code> $param</code> names declared in a sibling Cypher query.
        </p>
      </div>
    );
  },
};

export const StatusHook: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(pretty(DEMO_PAYLOAD));
    const [status, statusProps] = useLoraJsonStatus();
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          {...statusProps}
        />
        <div
          style={{
            fontSize: 12,
            fontFamily: "ui-monospace, monospace",
            background: "#f5f5f5",
            padding: 10,
            borderRadius: 4,
          }}
        >
          <strong>{status.ok ? "✓ Valid JSON" : `✗ ${status.diagnostics.length} error(s)`}</strong>
          <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
            {status.diagnostics.map((d, i) => (
              <li key={i}>{d.message}</li>
            ))}
          </ul>
        </div>
      </div>
    );
  },
};

export const RunCallback: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(pretty(DEMO_PAYLOAD));
    const [runs, setRuns] = useState<Array<{ at: number; source: string }>>(
      [],
    );
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
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          onRun={(source) =>
            setRuns((r) => [{ at: Date.now(), source }, ...r].slice(0, 5))
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
            <li style={{ listStyle: "none", color: "#9d9d9d" }}>
              (no runs yet)
            </li>
          )}
        </ol>
      </div>
    );
  },
};

export const ImperativeHandle: Story = {
  render: () => {
    const ref = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(
      `{"userId":"alice","active":true,"tags":["admin","editor"]}`,
    );
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <button onClick={() => void ref.current?.prettify()}>Prettify</button>
          <button onClick={() => void ref.current?.minify()}>Minify</button>
          <button
            onClick={async () => {
              const diags = await ref.current?.validate();
              console.log("diagnostics", diags);
            }}
          >
            Validate (log to console)
          </button>
          <button
            onClick={() =>
              ref.current?.setJson({
                userId: "bob",
                limits: { rate: 200 },
              })
            }
          >
            Set from JS value
          </button>
          <button
            onClick={() => {
              const v = ref.current?.getJson();
              console.log("parsed", v);
            }}
          >
            Parse (log to console)
          </button>
        </div>
        <LoraJsonEditor ref={ref} value={value} onChange={setValue} />
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

export const FullBleedHeight: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(pretty(DEMO_PAYLOAD));
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <div
          style={{
            width: "100%",
            height: 400,
            border: "1px dashed #aaa",
            padding: 12,
            boxSizing: "border-box",
          }}
        >
          <LoraJsonEditor
            ref={editorRef}
            value={value}
            onChange={setValue}
          />
        </div>
        <p style={{ margin: 0, fontSize: 12, color: "#6e7781" }}>
          The dashed box is 400 px tall. The editor stretches to fill it.
        </p>
      </div>
    );
  },
};

export const Placeholder: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState("");
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          placeholder='{ "userId": "alice", "minAge": 18 }'
        />
      </div>
    );
  },
};

export const LineNumbersOff: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(pretty(DEMO_PAYLOAD));
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          showLineNumbers={false}
        />
      </div>
    );
  },
};

export const SmartEditing: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(`{\n  "userId": "alice",\n}`);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
        />
        <ul style={{ margin: 0, fontSize: 12, color: "#6e7781", paddingLeft: 18 }}>
          <li>Place the caret between <code>{"{"}</code> and <code>{"}"}</code> and press <kbd>Enter</kbd> — splits onto three lines.</li>
          <li>Place the caret right after the trailing comma in the buffer and press <kbd>Enter</kbd> — opens a quoted key and the autocomplete.</li>
          <li>Collapse the object via the fold gutter — the placeholder shows the entry count.</li>
        </ul>
      </div>
    );
  },
};

export const AllowedRequiredKeys: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(
      `{\n  "userId": "alice",\n  "extra": true\n}`,
    );
    const [status, statusProps] = useLoraJsonStatus();
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          allowedKeys={["userId", "minAge", "cap"]}
          requiredKeys={["userId", "minAge"]}
          {...statusProps}
        />
        <div
          style={{
            fontSize: 12,
            fontFamily: "ui-monospace, monospace",
            background: "#f5f5f5",
            padding: 10,
            borderRadius: 4,
          }}
        >
          <div>
            <strong>allowedKeys</strong>{" "}
            <code>{`["userId", "minAge", "cap"]`}</code>
          </div>
          <div>
            <strong>requiredKeys</strong>{" "}
            <code>{`["userId", "minAge"]`}</code>
          </div>
          <div style={{ marginTop: 6 }}>
            <strong>{status.ok ? "✓ Valid" : `✗ ${status.diagnostics.length} issue(s)`}</strong>
            <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
              {status.diagnostics.map((d, i) => (
                <li key={i}>
                  <em>[{d.severity}]</em> {d.message}
                </li>
              ))}
            </ul>
          </div>
        </div>
        <p style={{ margin: 0, fontSize: 12, color: "#6e7781" }}>
          The <code>{`"extra"`}</code> key is flagged because it isn{`'`}t in
          {" "}<code>allowedKeys</code>. <code>{`"minAge"`}</code> is flagged as
          missing. The autocomplete only offers the allowed keys.
        </p>
      </div>
    );
  },
};

export const CursorPathBreadcrumb: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(
      pretty(
        JSON.stringify({
          users: [
            { name: "Alice", roles: ["admin", "editor"], age: 32 },
            { name: "Bob", roles: ["viewer"], age: 47 },
          ],
          flags: { archived: false, verified: true },
        }),
      ),
    );
    const [status, statusProps] = useLoraJsonStatus();
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          {...statusProps}
        />
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            fontSize: 12,
            fontFamily: "ui-monospace, monospace",
            background: "#f5f5f5",
            padding: "6px 10px",
            borderRadius: 4,
          }}
        >
          <span>
            <strong>path</strong> {status.cursorPathString}
          </span>
          <span style={{ color: "#6e7781" }}>
            {status.cursorPath.length} segment(s)
          </span>
        </div>
      </div>
    );
  },
};

export const FormatOnPaste: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(`{}`);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          formatOnPaste
        />
        <p style={{ margin: 0, fontSize: 12, color: "#6e7781" }}>
          Try pasting a minified blob like
          <code> {`{"userId":"alice","tags":["admin","editor"]}`}</code> —
          it lands already prettified.
        </p>
      </div>
    );
  },
};

export const SortFoldToggleQuotes: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const [value, setValue] = useState(`{
  'zeta': 1,
  'alpha': { 'beta': 2 },
  'gamma': [3, 2, 1]
}`);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <button onClick={() => editorRef.current?.toggleQuotes()}>
            Single → Double quotes
          </button>
          <button onClick={() => editorRef.current?.sortKeys()}>
            Sort keys A→Z
          </button>
          <button onClick={() => editorRef.current?.foldAll()}>Fold all</button>
          <button onClick={() => editorRef.current?.unfoldAll()}>
            Unfold all
          </button>
        </div>
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
        />
        <p style={{ margin: 0, fontSize: 12, color: "#6e7781" }}>
          Start with single-quoted strings, convert to JSON, sort keys.
          Fold-all + the count placeholder lets you eyeball nested
          structure at a glance. Keyboard shortcut for Sort:{" "}
          <kbd>⌥⇧S</kbd>.
        </p>
      </div>
    );
  },
};

export const LargePayload: Story = {
  render: () => {
    const editorRef = useRef<LoraJsonEditorHandle>(null);
    const initial = useMemo(() => {
      const rows = Array.from({ length: 30 }, (_, i) => ({
        id: `node-${i}`,
        name: `Person ${i}`,
        email: `person${i}@example.com`,
        roles: i % 2 === 0 ? ["admin", "editor"] : ["viewer"],
        active: i % 3 !== 0,
        createdAt: new Date(2024, 0, i + 1).toISOString(),
      }));
      return JSON.stringify(rows, null, 2);
    }, []);
    const [value, setValue] = useState(initial);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        <StoryToolbar editorRef={editorRef} />
        <LoraJsonEditor
          ref={editorRef}
          value={value}
          onChange={setValue}
          maxHeight="320px"
          theme={darkJsonTheme}
          readOnly
        />
        <p style={{ margin: 0, fontSize: 12, color: "#6e7781" }}>
          Read-only results viewer — fold gutters collapse each object,
          scrollbar handles overflow.
        </p>
      </div>
    );
  },
};
