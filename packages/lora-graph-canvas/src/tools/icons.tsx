/* Inline SVG icons. No font dependency. Each icon is 16×16 by default
 * and inherits `currentColor` so the toolbar's `color` CSS variable
 * controls them. Strokes are unitless so they scale crisply at any
 * `font-size`. */

import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement>;

const base: IconProps = {
  width: 16,
  height: 16,
  viewBox: "0 0 16 16",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.5,
  strokeLinecap: "round",
  strokeLinejoin: "round",
  "aria-hidden": true,
};

export const IconSelect = (p: IconProps) => (
  <svg {...base} {...p}>
    <path d="M3 2.5l4 10 1.4-3.6 3.6-1.4z" />
  </svg>
);

export const IconPan = (p: IconProps) => (
  <svg {...base} {...p}>
    <path d="M8 3v10M3 8h10M5 5l6 6M11 5L5 11" />
  </svg>
);

export const IconAddNode = (p: IconProps) => (
  <svg {...base} {...p}>
    <circle cx="6" cy="8" r="3" />
    <path d="M12 5v6M9 8h6" />
  </svg>
);

export const IconAddLink = (p: IconProps) => (
  <svg {...base} {...p}>
    <circle cx="4" cy="4" r="2" />
    <circle cx="12" cy="12" r="2" />
    <path d="M5.5 5.5l5 5" />
  </svg>
);

export const IconDelete = (p: IconProps) => (
  <svg {...base} {...p}>
    <path d="M3 5h10M6 5V3h4v2M5 5l1 8h4l1-8" />
  </svg>
);

export const IconFit = (p: IconProps) => (
  <svg {...base} {...p}>
    <path d="M3 6V3h3M13 6V3h-3M3 10v3h3M13 10v3h-3" />
  </svg>
);

export const IconZoomIn = (p: IconProps) => (
  <svg {...base} {...p}>
    <circle cx="7" cy="7" r="4" />
    <path d="M5 7h4M7 5v4M10 10l3 3" />
  </svg>
);

export const IconZoomOut = (p: IconProps) => (
  <svg {...base} {...p}>
    <circle cx="7" cy="7" r="4" />
    <path d="M5 7h4M10 10l3 3" />
  </svg>
);

export const IconPause = (p: IconProps) => (
  <svg {...base} {...p}>
    <path d="M6 3v10M10 3v10" />
  </svg>
);

export const IconResume = (p: IconProps) => (
  <svg {...base} {...p}>
    <path d="M5 3l8 5-8 5z" />
  </svg>
);

export const IconScreenshot = (p: IconProps) => (
  <svg {...base} {...p}>
    <path d="M3 5h2l1-2h4l1 2h2v8H3z" />
    <circle cx="8" cy="9" r="2.5" />
  </svg>
);

export const IconCube = (p: IconProps) => (
  <svg {...base} {...p}>
    <path d="M8 2l5 3v6l-5 3-5-3V5z" />
    <path d="M8 8v7M8 8L3 5M8 8l5-3" />
  </svg>
);

export const IconSquare = (p: IconProps) => (
  <svg {...base} {...p}>
    <rect x="3" y="3" width="10" height="10" rx="1" />
  </svg>
);

export const IconDuplicate = (p: IconProps) => (
  <svg {...base} {...p}>
    <rect x="3" y="3" width="8" height="8" rx="1" />
    <rect x="5" y="5" width="8" height="8" rx="1" />
  </svg>
);

export const IconSelectAll = (p: IconProps) => (
  <svg {...base} {...p}>
    <rect x="3" y="3" width="10" height="10" rx="1" strokeDasharray="2 2" />
    <circle cx="6" cy="6" r="1" fill="currentColor" />
    <circle cx="10" cy="6" r="1" fill="currentColor" />
    <circle cx="6" cy="10" r="1" fill="currentColor" />
    <circle cx="10" cy="10" r="1" fill="currentColor" />
  </svg>
);

export const IconExport = (p: IconProps) => (
  <svg {...base} {...p}>
    <path d="M8 2v8M5 5l3-3 3 3" />
    <path d="M3 12h10v2H3z" />
  </svg>
);

export const IconImport = (p: IconProps) => (
  <svg {...base} {...p}>
    <path d="M8 10V2M5 7l3 3 3-3" />
    <path d="M3 12h10v2H3z" />
  </svg>
);

/** Source node on the left, a "+" on the right, joined by an edge —
 *  visualises "create a new node connected to the selection". */
export const IconAddConnected = (p: IconProps) => (
  <svg {...base} {...p}>
    <circle cx="4" cy="8" r="2" />
    <path d="M6 8h2.5" />
    <circle cx="11" cy="8" r="2" />
    <path d="M11 6.5v3M9.5 8h3" />
  </svg>
);
