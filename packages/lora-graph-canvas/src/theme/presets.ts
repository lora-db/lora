import type { LoraGraphTheme } from "../types";

/** Light theme — matches the package default. Useful as a reset value
 *  when toggling between presets. */
export const lightTheme: LoraGraphTheme = {
  background: "#ffffff",
  foreground: "#1c1f23",
  border: "#d8dde3",
  accent: "#4f8ef7",
  toolbarBackground: "rgba(255, 255, 255, 0.92)",
  toolbarForeground: "#1c1f23",
  toolbarBorder: "#d8dde3",
  toolActiveBackground: "rgba(79, 142, 247, 0.18)",
  toolHoverBackground: "rgba(0, 0, 0, 0.05)",
  tooltipBackground: "rgba(28, 31, 35, 0.9)",
  tooltipForeground: "#ffffff",
  menuBackground: "#ffffff",
  menuForeground: "#1c1f23",
  menuHoverBackground: "rgba(0, 0, 0, 0.06)",
};

/** Dark theme — dark surface, accent kept blue. The engine
 *  `backgroundColor` accessor is independent; pair this with a dark
 *  `backgroundColor` prop for the full effect. */
export const darkTheme: LoraGraphTheme = {
  background: "#101216",
  foreground: "#e6e9ee",
  border: "#2a2f37",
  accent: "#6aa3ff",
  toolbarBackground: "rgba(20, 23, 28, 0.92)",
  toolbarForeground: "#e6e9ee",
  toolbarBorder: "#2a2f37",
  toolActiveBackground: "rgba(106, 163, 255, 0.25)",
  toolHoverBackground: "rgba(255, 255, 255, 0.08)",
  tooltipBackground: "rgba(240, 240, 240, 0.92)",
  tooltipForeground: "#101216",
  menuBackground: "#161a20",
  menuForeground: "#e6e9ee",
  menuHoverBackground: "rgba(255, 255, 255, 0.08)",
};
