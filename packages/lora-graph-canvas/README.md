# @loradb/lora-graph-canvas

React graph canvas for LoraDB. One component, `<LoraGraphCanvas />`, that
wraps `force-graph` (2D HTML5 canvas) and `3d-force-graph` (WebGL via
Three.js). Switch between 2D and 3D at runtime without re-mounting your
data, and use the built-in tool palette to build, edit, select, and
delete nodes interactively.

```tsx
import { LoraGraphCanvas } from "@loradb/lora-graph-canvas";
import "@loradb/lora-graph-canvas/styles.css";

<LoraGraphCanvas
  defaultData={{
    nodes: [{ id: "a" }, { id: "b" }],
    links: [{ source: "a", target: "b" }],
  }}
  nodeLabel="id"
  nodeAutoColorBy="group"
/>;
```

## Install

```sh
yarn add @loradb/lora-graph-canvas three
# or
npm install @loradb/lora-graph-canvas three
```

`three` is a required peer dependency — the package itself does not bundle
Three.js, so 2D-only consumers can dedupe with the rest of the app's
Three usage, and 3D consumers can pin a specific version.

## Built-in tools

The toolbar ships with twelve tools, controllable individually:

| Tool         | Shortcut | What it does                                       |
| ------------ | -------- | -------------------------------------------------- |
| Select       | `V`      | Click / shift-click nodes to select them.          |
| Pan          | `H`      | Pan-only cursor.                                   |
| Add node     | `N`      | Click on the canvas to drop a node.                |
| Add link     | `L`      | Click two nodes to connect them.                   |
| Delete       | `⌫` / `⌦` | Delete the current selection (cascades links).     |
| Fit          | `F`      | `zoomToFit` to the current graph bbox.             |
| Zoom in/out  | `+` `-`  | Step the zoom level.                               |
| Pause/Resume | —        | Stop / start the d3 simulation.                    |
| Screenshot   | —        | Download a PNG of the canvas.                      |
| Toggle 2D/3D | `3`      | Swap engines, preserving data and selection.       |

```tsx
// Pick a subset…
<LoraGraphCanvas tools={["select", "add-node", "delete", "toggle-mode"]} />

// …or hide the whole bar and drive everything from the ref:
<LoraGraphCanvas tools={false} ref={ref} />
```

## Ref handle

```tsx
import { useRef } from "react";
import {
  LoraGraphCanvas,
  type LoraGraphCanvasHandle,
} from "@loradb/lora-graph-canvas";

const ref = useRef<LoraGraphCanvasHandle>(null);

ref.current?.addNode({ id: "x", label: "hello" });
ref.current?.addLink({ source: "x", target: "y" });
ref.current?.removeNode("x");        // also removes attached links
ref.current?.fit(400, 40);
ref.current?.setMode("3d");
const blob = await ref.current?.screenshot();
```

Available methods: data (`getData`, `setData`, `addNode`, `addNodes`,
`updateNode`, `removeNode`, `removeNodes`, `addLink`, `addLinks`,
`removeLink`, `clear`); selection (`getSelection`, `setSelection`,
`selectAll`, `clearSelection`); view (`getMode`, `setMode`, `fit`,
`centerAt`, `zoom`, `zoomIn`, `zoomOut`); engine (`pause`, `resume`,
`reheat`, `screenshot`); escape hatches (`engine2D`, `engine3D`).

## Controlled vs uncontrolled

```tsx
// Uncontrolled — internal state owns the graph, host gets a notification:
<LoraGraphCanvas defaultData={initialData} onDataChange={save} />

// Controlled — host owns the graph:
<LoraGraphCanvas data={dataFromState} onDataChange={setDataState} />
```

The same dichotomy applies to `mode` (`defaultMode` vs `mode`).

## Theming

The chrome (toolbar, context menu, tooltips) is driven by `--lgc-*` CSS
variables. Override the ones you want either through the `theme` prop or
by attaching CSS to the `.lora-graph-canvas` container. The engine's own
canvas reads `backgroundColor` from a regular prop, independent of the
theme.

```tsx
import { LoraGraphCanvas, darkTheme } from "@loradb/lora-graph-canvas";

<LoraGraphCanvas
  backgroundColor="#0e1014"
  theme={{ ...darkTheme, accent: "#ff6699" }}
/>;
```

Two presets are exported: `lightTheme` and `darkTheme`.

## Performance knobs

For large graphs, cap `cooldownTicks` (default ∞) and increase
`warmupTicks` to spend more time computing layout off-screen before the
first paint:

```tsx
<LoraGraphCanvas cooldownTicks={50} warmupTicks={20} />
```

## License

BUSL-1.1. Third-party attributions for `force-graph` and `3d-force-graph`
(MIT, © Vasco Asturiano) live in `THIRD_PARTY.md`.
