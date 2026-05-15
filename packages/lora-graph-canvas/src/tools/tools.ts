import type { ComponentType, SVGProps } from "react";
import type { ToolId } from "../types";
import {
  IconAddLink,
  IconAddNode,
  IconCube,
  IconDelete,
  IconDuplicate,
  IconExport,
  IconFit,
  IconImport,
  IconPan,
  IconPause,
  IconResume,
  IconScreenshot,
  IconSelect,
  IconSelectAll,
  IconSquare,
  IconZoomIn,
  IconZoomOut,
} from "./icons";

export interface ToolDescriptor {
  id: ToolId;
  label: string;
  /** Aria label / tooltip. */
  hint: string;
  icon: ComponentType<SVGProps<SVGSVGElement>>;
  /** Tools that toggle an active mode (select, pan, add-node, add-link).
   *  Others are one-shot actions. */
  toggleable?: boolean;
  /** Suggested keybinding. The toolbar surfaces this in tooltips; the
   *  actual binding is registered by the component. */
  shortcut?: string;
}

export const TOOL_DESCRIPTORS: Record<ToolId, ToolDescriptor> = {
  select: {
    id: "select",
    label: "Select",
    hint: "Select / multi-select nodes",
    icon: IconSelect,
    toggleable: true,
    shortcut: "V",
  },
  pan: {
    id: "pan",
    label: "Pan",
    hint: "Pan the canvas",
    icon: IconPan,
    toggleable: true,
    shortcut: "H",
  },
  "add-node": {
    id: "add-node",
    label: "Add node",
    hint: "Click on the canvas to add a node",
    icon: IconAddNode,
    toggleable: true,
    shortcut: "N",
  },
  "add-link": {
    id: "add-link",
    label: "Add link",
    hint: "Click two nodes to connect them",
    icon: IconAddLink,
    toggleable: true,
    shortcut: "L",
  },
  delete: {
    id: "delete",
    label: "Delete",
    hint: "Delete selected node(s)",
    icon: IconDelete,
    shortcut: "⌫",
  },
  fit: {
    id: "fit",
    label: "Fit",
    hint: "Fit graph to viewport",
    icon: IconFit,
    shortcut: "F",
  },
  "zoom-in": {
    id: "zoom-in",
    label: "Zoom in",
    hint: "Zoom in",
    icon: IconZoomIn,
    shortcut: "+",
  },
  "zoom-out": {
    id: "zoom-out",
    label: "Zoom out",
    hint: "Zoom out",
    icon: IconZoomOut,
    shortcut: "-",
  },
  pause: {
    id: "pause",
    label: "Pause",
    hint: "Pause simulation",
    icon: IconPause,
  },
  resume: {
    id: "resume",
    label: "Resume",
    hint: "Resume simulation",
    icon: IconResume,
  },
  screenshot: {
    id: "screenshot",
    label: "Screenshot",
    hint: "Download a PNG of the canvas",
    icon: IconScreenshot,
  },
  "toggle-mode": {
    id: "toggle-mode",
    label: "2D / 3D",
    hint: "Toggle between 2D and 3D",
    icon: IconCube,
    shortcut: "3",
  },
  duplicate: {
    id: "duplicate",
    label: "Duplicate",
    hint: "Duplicate the current selection",
    icon: IconDuplicate,
    shortcut: "⌘D",
  },
  "select-all": {
    id: "select-all",
    label: "Select all",
    hint: "Select every node",
    icon: IconSelectAll,
    shortcut: "⌘A",
  },
  "export-json": {
    id: "export-json",
    label: "Export",
    hint: "Download the graph as JSON",
    icon: IconExport,
  },
  "import-json": {
    id: "import-json",
    label: "Import",
    hint: "Load a JSON graph from disk",
    icon: IconImport,
  },
};

/** Default toolbar order when `tools={true}`. */
export const DEFAULT_TOOL_ORDER: ToolId[] = [
  "select",
  "pan",
  "add-node",
  "add-link",
  "duplicate",
  "delete",
  "select-all",
  "fit",
  "zoom-in",
  "zoom-out",
  "pause",
  "resume",
  "screenshot",
  "export-json",
  "import-json",
  "toggle-mode",
];

/** Icon for the toggle-mode button, depending on the currently active
 *  mode (we render a cube when in 2D — "switch to 3D" — and a square
 *  when in 3D). The toolbar reads from this rather than the static
 *  descriptor for that one button. */
export function toggleModeIcon(currentMode: "2d" | "3d") {
  return currentMode === "2d" ? IconCube : IconSquare;
}
