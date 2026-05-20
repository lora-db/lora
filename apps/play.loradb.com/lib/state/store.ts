"use client";

/**
 * Zustand store composition.
 *
 * Six slices (tabs, results, layout, prefs, schema, inspect) are merged
 * into a single store wrapped with `immer` (imperative draft mutations
 * inside the slices) and `subscribeWithSelector` (so we can subscribe to
 * the exact fields that participate in IDB persistence).
 *
 * Persistence is driven by an explicit selector subscription rather than
 * `zustand/middleware/persist` — we own the IDB schema and want full
 * control over what gets written and when.
 */

import { create, type StoreApi } from "zustand";
import { immer } from "zustand/middleware/immer";
import { subscribeWithSelector } from "zustand/middleware";
import { notifications } from "@mantine/notifications";

import * as sessionPersistence from "@/lib/persistence/session";
import { debounce } from "@/lib/util/async";

import {
  createTabsSlice,
  type TabsSlice,
  type SerializedTab,
  type EditorTab,
} from "./slices/tabs";
import {
  createResultsSlice,
  type ResultsSlice,
} from "./slices/results";
import {
  createLayoutSlice,
  type LayoutSlice,
  type SerializedLayout,
} from "./slices/layout";
import {
  createPrefsSlice,
  type PrefsSlice,
  type SerializedPrefs,
} from "./slices/prefs";
import {
  createSchemaSlice,
  type SchemaSlice,
} from "./slices/schema";
import {
  createInspectSlice,
  type InspectSlice,
} from "./slices/inspect";
import {
  createParamsByTabSlice,
  type ParamsByTabSlice,
} from "./slices/paramsByTab";

export type Store = TabsSlice
  & ResultsSlice
  & LayoutSlice
  & PrefsSlice
  & SchemaSlice
  & InspectSlice
  & ParamsByTabSlice;

export const useStore = create<Store>()(
  subscribeWithSelector(
    immer((set, get, api) => ({
      ...createTabsSlice(
        set as Parameters<typeof createTabsSlice>[0],
        get as Parameters<typeof createTabsSlice>[1],
        api as Parameters<typeof createTabsSlice>[2],
      ),
      ...createResultsSlice(
        set as Parameters<typeof createResultsSlice>[0],
        get as Parameters<typeof createResultsSlice>[1],
        api as Parameters<typeof createResultsSlice>[2],
      ),
      ...createLayoutSlice(
        set as Parameters<typeof createLayoutSlice>[0],
        get as Parameters<typeof createLayoutSlice>[1],
        api as Parameters<typeof createLayoutSlice>[2],
      ),
      ...createPrefsSlice(
        set as Parameters<typeof createPrefsSlice>[0],
        get as Parameters<typeof createPrefsSlice>[1],
        api as Parameters<typeof createPrefsSlice>[2],
      ),
      ...createSchemaSlice(
        set as Parameters<typeof createSchemaSlice>[0],
        get as Parameters<typeof createSchemaSlice>[1],
        api as Parameters<typeof createSchemaSlice>[2],
      ),
      ...createInspectSlice(
        set as Parameters<typeof createInspectSlice>[0],
        get as Parameters<typeof createInspectSlice>[1],
        api as Parameters<typeof createInspectSlice>[2],
      ),
      ...createParamsByTabSlice(
        set as Parameters<typeof createParamsByTabSlice>[0],
        get as Parameters<typeof createParamsByTabSlice>[1],
        api as Parameters<typeof createParamsByTabSlice>[2],
      ),
    })),
  ),
);

// Tell TypeScript the store has the subscribeWithSelector overload available.
type SubscribeWithSelector = StoreApi<Store> & {
  subscribe: <U>(
    selector: (state: Store) => U,
    listener: (selected: U, previous: U) => void,
    options?: {
      equalityFn?: (a: U, b: U) => boolean;
      fireImmediately?: boolean;
    },
  ) => () => void;
};

/**
 * Snapshot of the raw store fields we persist. Holding raw immer-produced
 * references (instead of building a fresh object per field) means shallow
 * equality on this snapshot is sufficient — immer reuses references for
 * any subtree that didn't change, so unrelated mutations don't fire the
 * persistence write.
 */
interface PersistedSnapshot {
  tabs: EditorTab[];
  // layout
  activitySection: SerializedLayout["activitySection"];
  sidebarOpen: SerializedLayout["sidebarOpen"];
  sidebarWidth: SerializedLayout["sidebarWidth"];
  workspace: SerializedLayout["workspace"];
  activePaneId: SerializedLayout["activePaneId"];
  // prefs
  graphMode: SerializedPrefs["graphMode"];
  autoRunOnSave: SerializedPrefs["autoRunOnSave"];
  autoFormatOnRun: SerializedPrefs["autoFormatOnRun"];
  nodeCap: SerializedPrefs["nodeCap"];
  resultRowCap: SerializedPrefs["resultRowCap"];
  autoRestore: SerializedPrefs["autoRestore"];
  focusOnNodeClick: SerializedPrefs["focusOnNodeClick"];
  alwaysShowLabels: SerializedPrefs["alwaysShowLabels"];
  fitOnSelect: SerializedPrefs["fitOnSelect"];
}

function selectPersisted(s: Store): PersistedSnapshot {
  return {
    tabs: s.tabs,
    activitySection: s.activitySection,
    sidebarOpen: s.sidebarOpen,
    sidebarWidth: s.sidebarWidth,
    workspace: s.workspace,
    activePaneId: s.activePaneId,
    graphMode: s.graphMode,
    autoRunOnSave: s.autoRunOnSave,
    autoFormatOnRun: s.autoFormatOnRun,
    nodeCap: s.nodeCap,
    resultRowCap: s.resultRowCap,
    autoRestore: s.autoRestore,
    focusOnNodeClick: s.focusOnNodeClick,
    alwaysShowLabels: s.alwaysShowLabels,
    fitOnSelect: s.fitOnSelect,
  };
}

function shallowEqualSnapshot(
  a: PersistedSnapshot,
  b: PersistedSnapshot,
): boolean {
  // Compare every key by reference / Object.is — immer reuses references
  // for unchanged subtrees so this is equivalent to a deep equality check
  // for our purposes.
  return (
    a.tabs === b.tabs &&
    a.activitySection === b.activitySection &&
    a.sidebarOpen === b.sidebarOpen &&
    a.sidebarWidth === b.sidebarWidth &&
    a.workspace === b.workspace &&
    a.activePaneId === b.activePaneId &&
    a.graphMode === b.graphMode &&
    a.autoRunOnSave === b.autoRunOnSave &&
    a.autoFormatOnRun === b.autoFormatOnRun &&
    a.nodeCap === b.nodeCap &&
    a.resultRowCap === b.resultRowCap &&
    a.autoRestore === b.autoRestore &&
    a.focusOnNodeClick === b.focusOnNodeClick &&
    a.alwaysShowLabels === b.alwaysShowLabels &&
    a.fitOnSelect === b.fitOnSelect
  );
}

function serializeSnapshot(s: PersistedSnapshot) {
  return {
    tabs: s.tabs.map<SerializedTab>((t) => ({
      id: t.id,
      name: t.name,
      body: t.body,
      params: t.params,
      savedQueryId: t.savedQueryId,
      createdAt: t.createdAt,
    })),
    // `activeTabId` is now derived from the workspace tree; we still
    // write `null` so legacy IDB readers (older preview tabs) don't
    // crash on its absence.
    activeTabId: null as string | null,
    layout: {
      activitySection: s.activitySection,
      sidebarOpen: s.sidebarOpen,
      sidebarWidth: s.sidebarWidth,
      workspace: s.workspace,
      activePaneId: s.activePaneId,
    },
    prefs: {
      graphMode: s.graphMode,
      autoRunOnSave: s.autoRunOnSave,
      autoFormatOnRun: s.autoFormatOnRun,
      nodeCap: s.nodeCap,
      resultRowCap: s.resultRowCap,
      autoRestore: s.autoRestore,
      focusOnNodeClick: s.focusOnNodeClick,
      alwaysShowLabels: s.alwaysShowLabels,
      fitOnSelect: s.fitOnSelect,
    },
  };
}

// Hydration gate — true while `hydrateFromIDB` is replacing slice state.
// The persistence subscription is wired with `fireImmediately: false`
// already, but any subsequent re-render from hydration would still queue
// a write. We skip those writes entirely so the just-loaded record isn't
// immediately re-written.
let isHydrating = false;

// One-shot guard so a broken IDB doesn't spam toasts on every save.
let persistFailureToasted = false;

const persistDebounced = debounce((snapshot: PersistedSnapshot) => {
  void sessionPersistence.write(serializeSnapshot(snapshot)).then((ok) => {
    if (ok === false && !persistFailureToasted) {
      persistFailureToasted = true;
      notifications.show({
        color: "yellow",
        title: "Couldn't save your session",
        message:
          "Your tabs and layout won't persist across reloads. Check whether storage is allowed for this site.",
        autoClose: 8000,
      });
    }
  });
}, 500);

// Wire up the persistence subscription only on the client. Server-side
// rendering must never touch IDB.
if (typeof window !== "undefined") {
  (useStore as unknown as SubscribeWithSelector).subscribe(
    selectPersisted,
    (snapshot) => {
      if (isHydrating) return;
      persistDebounced(snapshot);
    },
    { equalityFn: shallowEqualSnapshot },
  );
  // Dev-only escape hatch so headless tests can introspect the live
  // store. Removed from production via tree-shaking when this module
  // is bundled with NODE_ENV=production.
  if (process.env.NODE_ENV !== "production") {
    (window as unknown as { __loradbStore?: typeof useStore }).__loradbStore = useStore;
  }
}

/**
 * Reads the persisted session record from IDB and applies it to the store.
 * Idempotent: safe to call multiple times (e.g. from a layout effect that
 * remounts during dev HMR). Silently no-ops on the server or when no
 * persisted record exists.
 */
export async function hydrateFromIDB(): Promise<void> {
  if (typeof window === "undefined") return;
  const record = await sessionPersistence.read();
  if (!record) return;

  // Gate the persistence subscription so the writes we'd otherwise queue
  // for these three setters don't immediately echo the just-loaded record
  // back to IDB. The gate is cleared in a microtask so any *real* state
  // changes that happen on the same tick still get persisted.
  isHydrating = true;
  try {
    const state = useStore.getState();
    state.hydrateTabs(record.tabs);
    // `record.layout` may be on either the legacy shape (panelSizes +
    // resultTab) or the new workspace tree — `hydrateLayout` runs it
    // through `migrateLayout` first so both paths converge here.
    state.hydrateLayout(record.layout as SerializedLayout);
    state.hydratePrefs(record.prefs);
    // Legacy records carried a top-level `activeTabId`. If the
    // restored workspace's first editor view doesn't yet have a
    // tabId (e.g. legacy migrations) and the record's value still
    // identifies a known tab, plant it as that view's active tab so
    // the user lands where they left off.
    const legacyActiveTabId = (record as unknown as { activeTabId?: string | null }).activeTabId ?? null;
    if (legacyActiveTabId) {
      const after = useStore.getState();
      const knownTab = after.tabs.some((t) => t.id === legacyActiveTabId);
      if (knownTab) {
        for (const leaf of (function* walk(node: SerializedLayout["workspace"]): Generator<{ id: string; views: { id: string; kind: string; tabId?: string; tabIds?: string[] }[] }> {
          if (node.type === "leaf") {
            yield node;
            return;
          }
          for (const c of node.children) yield* walk(c);
        })(after.workspace)) {
          const editorView = leaf.views.find((v) => v.kind === "editor");
          if (editorView && !editorView.tabId) {
            after.setViewTabId(editorView.id, legacyActiveTabId);
            break;
          }
        }
      }
    }
  } finally {
    queueMicrotask(() => {
      isHydrating = false;
    });
  }
}
