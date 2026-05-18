# @loradb/lora-query

React CodeMirror editor for the LoraDB Cypher dialect, backed by a WASM
[pest](https://pest.rs/) parser. Designed to be drop-in: mount it once
and you get scope-aware autocomplete, AST-driven semantic colouring,
rich diagnostics, code folding, hover / signature tooltips, find +
replace, jump-to-declaration, and a structured imperative API.

```tsx
import {
  LoraQueryEditor,
  useLoraQueryStatus,
  darkTheme,
} from "@loradb/lora-query";

function MyEditor() {
  const [value, setValue] = useState("MATCH (n) RETURN n");
  const [status, statusProps] = useLoraQueryStatus();

  return (
    <>
      <LoraQueryEditor
        value={value}
        onChange={setValue}
        theme={darkTheme}
        labels={["Person", "Company"]}
        relTypes={["KNOWS", "WORKS_AT"]}
        getPropertyKeys={({ label }) =>
          label === "Person" ? ["name", "age", "email"] : []
        }
        onRun={(src) => execute(src)}
        {...statusProps}
      />
      <footer>
        {status.errors.length} errors, {status.warnings.length} warnings
        — params: {status.parameters.join(", ")}
      </footer>
    </>
  );
}
```

## Surfaces

- `@loradb/lora-query` — the React component, hook, themes, and the
  CodeMirror extension bundle.
- `@loradb/lora-query/parser` — the standalone async parser API
  (`parse`, `validate`, `format`, `highlight`, `outline`, `analyse`)
  backed by the WASM build. Use this when you want diagnostics or
  formatting without the editor.

## Props

| Prop | Type | Description |
| ---- | ---- | ----------- |
| `value` | `string` | Controlled source. |
| `onChange` | `(next: string) => void` | Fires on every edit. |
| `readOnly` | `boolean` | Disable editing. |
| `theme` | `LoraQueryTheme` | CSS-variable overrides — see [Theming](#theming). |
| `style` | `CSSProperties` | Inline style on the outer container. |
| `className` | `string` | Extra class on the outer container. |
| `labels` | `string[]` | Known node labels — surfaced after `:` inside `(...)`. |
| `relTypes` | `string[]` | Known relationship types — surfaced after `:` inside `[...]`. |
| `getPropertyKeys` | `(ctx) => string[] \| Promise<string[]>` | Schema callback for property keys inside `{...}` and after `var.`. |
| `onRun` | `(source: string) => void` | Fires on `Cmd/Ctrl + Enter`. |
| `onDiagnostics` | `(errors) => void` | Syntax-error diagnostics. |
| `onAnalysis` | `(analysis) => void` | Semantic warnings + fold ranges. |
| `onOutline` | `(outline) => void` | Declared variables, params, distinct labels / rel-types. |

## Imperative handle

```ts
interface LoraQueryEditorHandle {
  prettify(): Promise<void>;
  format(): Promise<void>; // alias for prettify
  validate(): Promise<Diagnostic[]>;
  run(): void;             // triggers onRun with the current source
  getValue(): string;
  setValue(next: string): void;
  getParameters(): Promise<string[]>;
  getDeclaredVariables(): Promise<string[]>;
  view(): EditorView | null;
}
```

```tsx
const ref = useRef<LoraQueryEditorHandle>(null);
// ...
ref.current?.prettify();
const params = await ref.current?.getParameters();
```

## Theming

Every visual choice is a CSS variable on the `.lora-query` container.
The editor is light by default; dark is opt-in. Resolution order
(highest priority first):

1. inline `style="--lq-*"` set by the `theme` prop
2. `data-color-scheme="dark"` on the container (set by the
   `colorScheme` prop)
3. the light defaults shipped in `editor.css`

```tsx
// 1. Default — light palette, no props needed.
<LoraQueryEditor value={q} onChange={setQ} />

// 2. Flip to dark via the colorScheme prop (purely CSS-driven).
<LoraQueryEditor colorScheme="dark" value={q} onChange={setQ} />

// 3. Custom palette via `theme`. `createTheme` derives a full
//    `LoraQueryTheme` from a base palette plus overrides.
import { createTheme, githubDark } from "@loradb/lora-query";
const theme = createTheme(githubDark, { accent: "#ff6b6b" });
<LoraQueryEditor theme={theme} value={q} onChange={setQ} />

// 4. Auto-follow the host's color-scheme — wire it yourself with
//    `window.matchMedia('(prefers-color-scheme: dark)')`.
```

Built-in presets: `lightTheme` (Catppuccin Latte), `darkTheme`
(GitHub Dark hues on a VS-Code-style surface).
Palette primitives: `latte`, `githubDark`, `typography`,
`createTheme`.

| Group | Keys |
| ----- | ---- |
| Surface | `background`, `foreground`, `border`, `accent`, `muted`, `activeLine`, `gutterBackground`, `gutterForeground`, `cursor`, `selectionBackground` |
| Typography | `fontFamily`, `monoFontFamily`, `fontSize`, `popupFontSize` |
| Tokens | `keyword`, `variable`, `parameter`, `label`, `relType`, `property`, `functionName`, `namespace`, `string`, `number`, `bool`, `null` |
| Popups | `popupBackground`, `popupForeground`, `popupBorder`, `popupSelectedBackground`, `popupSelectedForeground`, `popupShadow` |
| Diagnostics | `errorAccent`, `warningAccent`, `infoAccent` |
| Scrollbar | `scrollbarTrack`, `scrollbarThumb`, `scrollbarThumbHover`, `scrollbarWidth` (`auto`/`thin`/`none`), `scrollbarSize` |

Popups (autocomplete / hover / lint) are styled through
`EditorView.theme()` so they stay correctly themed even when CodeMirror
renders them into `document.body`.

## Keyboard shortcuts

| Shortcut | Action |
| -------- | ------ |
| `⌘/Ctrl + Enter` | Trigger `onRun(source)` |
| `⌘/Ctrl + Shift + F` | Prettify the buffer |
| `⌘/Ctrl + F` | Open find / replace |
| `F12`, `⌘/Ctrl + D` | Jump to variable declaration |
| `⌘/Ctrl + click` on a variable | Jump to declaration |
| `Tab` / `Shift + Tab` | Indent / outdent |

## Smart features

- **Scope-precise autocomplete** — variables surface only after their
  declaration site; clauses are not suggested inside an unclosed `(`,
  `[`, or `{`.
- **`namespace.member` completion** — typing `math.` lists every
  `math.*` function with its signature inline.
- **`var.property` completion** — typing `alice.` resolves `alice` in
  the outline, finds its label, and calls your `getPropertyKeys`
  callback with `{ kind: "node", label: "Person", variable: "alice" }`.
- **Property-map completion** — typing `(alice:Person {|})` hits the
  same callback with `{ kind: "node", label: "Person" }`.
- **Snippet expansions** — `MATCH`, `MERGE`, `CASE`, `UNWIND`,
  `WITH`, and `MATCH ()-[]->()` insert tab-stop templates.
- **Rich diagnostics** — every error carries pest's full positional
  report (`--> 2:16` block with caret), a one-line summary, and a
  *Try one of:* list of valid code snippets.
- **Semantic warnings** — second-pass analysis flags undeclared
  variables in RETURN/WITH, unknown labels / rel types (when the host
  provides a strict list), and unused bindings.
- **Hover tooltips** — variables show their binding line + label;
  keywords / functions show their signature + description.
- **Signature hints** — a tooltip follows the cursor inside `fn(|)`.
- **AST-driven semantic colouring** — variables, labels, rel types,
  property keys, function names, namespaces, literals each get their
  own CSS class.
- **Code folding** — clauses, patterns, projections, and CASE blocks
  fold via the gutter.

## Building

```bash
yarn workspace @loradb/lora-query build:wasm
yarn workspace @loradb/lora-query build
```

`build:wasm` runs `wasm-pack build` against the embedded Rust crate
(`crates/lora-query-wasm`). `build` runs the Vite library build on top.

## Storybook

```bash
yarn workspace @loradb/lora-query storybook
```

Stories: Default · MultiLine · WithSyntaxError · ReadOnly ·
RichGraphPattern · WithSchemaProviders · DarkTheme ·
ImperativeHandle.

## Layout

```
packages/lora-query/
├── .storybook/                # Storybook config
├── Cargo.toml                 # Rust crate manifest ([lib] → rust/lib.rs)
├── package.json               # npm package manifest
├── rust/
│   └── lib.rs                 # pest parser + wasm-bindgen surface
├── src/                       # React + CodeMirror sources
│   ├── LoraQueryEditor.tsx
│   ├── useLoraQueryStatus.ts
│   ├── themes.ts
│   ├── parser.ts
│   ├── editor.css
│   └── cypher/                # CodeMirror extension modules
└── wasm/                      # wasm-pack output (gitignored)
```
