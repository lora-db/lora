"use client";

/**
 * Ergonomic re-exports so consumers can `import { runActiveTab, newTab }`
 * from a single barrel rather than chasing individual files.
 */

export * from "./tabActions";
export * from "./uiActions";
export { runActiveTab } from "./runActiveTab";
