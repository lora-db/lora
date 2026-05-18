import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { Compartment, Prec, type Extension } from "@codemirror/state";
import { EditorView, keymap, type KeyBinding } from "@codemirror/view";
import { type Diagnostic } from "@codemirror/lint";
import { cypherExtensions } from "./cypher/extensions";
import { popupTheme } from "./cypher/theme";
import {
  loraQueryProviders,
  type LoraQueryProviders,
  type ProcedureSignature,
  type PropertyContext,
} from "./cypher/providers";
import {
  analyseAll,
  format,
  outline,
  validateAll,
  type Analysis,
  type Outline,
  type ParseError,
} from "./parser";
import "./editor.css";

/**
 * CSS-variable theme. Each key maps to one of the editor's `--lq-*`
 * CSS variables — set just the ones you want to override. The host
 * may apply it via the `theme` prop or by attaching its own CSS to the
 * `.lora-query` container.
 *
 * The theme covers the editor surface *and* every popup the editor
 * renders — autocomplete popup, hover tooltip, and lint message — so a
 * single object is enough to dress the whole component.
 */
export interface LoraQueryTheme {
  // Editor surface
  background?: string;
  foreground?: string;
  border?: string;
  accent?: string;
  muted?: string;
  activeLine?: string;
  gutterBackground?: string;
  gutterForeground?: string;
  cursor?: string;
  selectionBackground?: string;

  // Typography
  /** Font for popup labels and tooltip prose. Defaults to the UI stack. */
  fontFamily?: string;
  /** Font for the editor content and code snippets in popups. Defaults to a monospace stack. */
  monoFontFamily?: string;
  /** Editor content font size (e.g. `"13px"`). */
  fontSize?: string;
  /** Popup / tooltip font size (e.g. `"12px"`). */
  popupFontSize?: string;

  // Token colours (also used by the AST-driven decorations).
  keyword?: string;
  variable?: string;
  parameter?: string;
  label?: string;
  relType?: string;
  property?: string;
  functionName?: string;
  namespace?: string;
  string?: string;
  number?: string;
  bool?: string;
  null?: string;

  // Popups (autocomplete, hover tooltip, lint message).
  popupBackground?: string;
  popupForeground?: string;
  popupBorder?: string;
  popupSelectedBackground?: string;
  popupSelectedForeground?: string;
  popupShadow?: string;

  // Lint message accent (left border + details background tint).
  errorAccent?: string;
  warningAccent?: string;
  infoAccent?: string;

  // Scrollbar (vertical + horizontal). Honoured by the editor's
  // `.cm-scroller` via the modern `scrollbar-color` / `scrollbar-width`
  // properties and a matching `::-webkit-scrollbar` ruleset.
  /** Track (gutter) colour behind the scrollbar. */
  scrollbarTrack?: string;
  /** Resting colour of the draggable thumb. */
  scrollbarThumb?: string;
  /** Thumb colour on hover. */
  scrollbarThumbHover?: string;
  /** CSS `scrollbar-width` value — `"auto"`, `"thin"`, or `"none"`. */
  scrollbarWidth?: "auto" | "thin" | "none";
  /** Pixel thickness for the WebKit `::-webkit-scrollbar` rules. */
  scrollbarSize?: string;
}

const THEME_TO_VAR: Record<keyof LoraQueryTheme, string> = {
  background: "--lq-bg",
  foreground: "--lq-fg",
  border: "--lq-border",
  accent: "--lq-accent",
  muted: "--lq-muted",
  activeLine: "--lq-active-line",
  gutterBackground: "--lq-gutter-bg",
  gutterForeground: "--lq-gutter-fg",
  cursor: "--lq-cursor",
  selectionBackground: "--lq-selection-bg",

  fontFamily: "--lq-font",
  monoFontFamily: "--lq-mono-font",
  fontSize: "--lq-font-size",
  popupFontSize: "--lq-popup-font-size",

  keyword: "--lq-color-keyword",
  variable: "--lq-color-variable",
  parameter: "--lq-color-parameter",
  label: "--lq-color-label",
  relType: "--lq-color-rel-type",
  property: "--lq-color-property",
  functionName: "--lq-color-function",
  namespace: "--lq-color-namespace",
  string: "--lq-color-string",
  number: "--lq-color-number",
  bool: "--lq-color-bool",
  null: "--lq-color-null",

  popupBackground: "--lq-popup-bg",
  popupForeground: "--lq-popup-fg",
  popupBorder: "--lq-popup-border",
  popupSelectedBackground: "--lq-popup-selected-bg",
  popupSelectedForeground: "--lq-popup-selected-fg",
  popupShadow: "--lq-popup-shadow",

  errorAccent: "--lq-error",
  warningAccent: "--lq-warning",
  infoAccent: "--lq-info",

  scrollbarTrack: "--lq-scrollbar-track",
  scrollbarThumb: "--lq-scrollbar-thumb",
  scrollbarThumbHover: "--lq-scrollbar-thumb-hover",
  scrollbarWidth: "--lq-scrollbar-width",
  scrollbarSize: "--lq-scrollbar-size",
};

export interface LoraQueryEditorProps {
  value: string;
  onChange?: (next: string) => void;
  readOnly?: boolean;
  className?: string;
  /** Inline style applied to the outer container. */
  style?: CSSProperties;
  /** CSS-variable theme overrides — see {@link LoraQueryTheme}. */
  theme?: LoraQueryTheme;
  /** Called whenever the validator has new diagnostics. */
  onDiagnostics?: (errors: ParseError[]) => void;
  /** Called whenever the semantic analyser has new warnings + fold ranges. */
  onAnalysis?: (analysis: Analysis) => void;
  /** Called whenever the outline (variables / params / labels) changes. */
  onOutline?: (outline: Outline) => void;
  /**
   * Fired on `Cmd/Ctrl + Enter`. Hosts typically wire this to
   * "execute the query". The current source string is passed in.
   */
  onRun?: (source: string) => void;

  /** Known node labels — surfaced after `:` inside `(...)`. */
  labels?: readonly string[];
  /** Known relationship types — surfaced after `:` inside `[...]`. */
  relTypes?: readonly string[];
  /** Stored procedures — surfaced after `CALL ` and inside `YIELD`. */
  procedures?: readonly ProcedureSignature[];

  /**
   * Force a color scheme regardless of the host's
   * `prefers-color-scheme` setting. Defaults to `"auto"`, which lets
   * the system value win. Inline `theme` overrides take priority over
   * this prop.
   */
  colorScheme?: "light" | "dark" | "auto";
  /** Show or hide the line-number gutter. Defaults to `true`. */
  showLineNumbers?: boolean;
  /** Placeholder text rendered when the buffer is empty. */
  placeholder?: string;
  /** Minimum height of the editor container. */
  minHeight?: string;
  /** Maximum height of the editor container (enables vertical scroll). */
  maxHeight?: string;
  /**
   * Extra CodeMirror extensions to merge in. Appended to the default
   * bundle so the host can add language overlays, decorations, custom
   * lint sources, etc.
   */
  extraExtensions?: Extension | readonly Extension[];
  /**
   * Extra keybindings, merged at `Prec.high` so host shortcuts win
   * over the defaults. Example: `{ key: "Mod-/", run: toggleComment }`.
   */
  extraKeymap?: readonly KeyBinding[];
  /**
   * Called when the cursor sits inside a `{ ... }` property map of a
   * node or relationship pattern. Return (or resolve to) the property
   * keys you want to offer.
   *
   * The `ctx` argument carries the surrounding label hint
   * (`(alice:Person {|})` → `{ kind: 'node', label: 'Person', ... }`)
   * so the host can fetch schema-aware results.
   */
  getPropertyKeys?: (
    ctx: PropertyContext,
  ) => readonly string[] | Promise<readonly string[]>;
}

export interface LoraQueryEditorHandle {
  /** Reformat the current buffer in place via the WASM prettifier. */
  prettify: () => Promise<void>;
  /** Alias for {@link prettify}. */
  format: () => Promise<void>;
  /** Run a one-shot validation pass and return the diagnostics. */
  validate: () => Promise<Diagnostic[]>;
  /** Current source code. */
  getValue: () => string;
  /** Imperatively replace the editor content. */
  setValue: (next: string) => void;
  /** Names of every `$param` referenced in the query. */
  getParameters: () => Promise<string[]>;
  /** Names of every variable declared anywhere in the query. */
  getDeclaredVariables: () => Promise<string[]>;
  /** Trigger the host-provided `onRun` callback with the current source. */
  run: () => void;
  /**
   * Copy the current buffer to the clipboard. Resolves with `true` on
   * success, `false` if the Clipboard API is unavailable or refused.
   */
  copy: () => Promise<boolean>;
  /** Move keyboard focus to the editor. */
  focus: () => void;
  /** Direct access to the underlying CodeMirror view, if needed. */
  view: () => EditorView | null;
}

function buildProviders(
  labels: readonly string[] | undefined,
  relTypes: readonly string[] | undefined,
  procedures: readonly ProcedureSignature[] | undefined,
  getPropertyKeys:
    | ((ctx: PropertyContext) => readonly string[] | Promise<readonly string[]>)
    | undefined,
): LoraQueryProviders {
  return {
    labels: labels ?? [],
    relTypes: relTypes ?? [],
    procedures: procedures ?? [],
    ...(getPropertyKeys ? { getPropertyKeys: (ctx) => getPropertyKeys(ctx) } : {}),
  };
}

function toExtensionArray(
  extras: Extension | readonly Extension[] | undefined,
): Extension[] {
  if (!extras) return [];
  return (Array.isArray(extras) ? [...extras] : [extras]) as Extension[];
}

function themeToStyle(theme: LoraQueryTheme | undefined): CSSProperties {
  if (!theme) return {};
  const out: Record<string, string> = {};
  for (const k of Object.keys(theme) as (keyof LoraQueryTheme)[]) {
    const v = theme[k];
    if (v) out[THEME_TO_VAR[k]] = v;
  }
  return out as CSSProperties;
}

/**
 * Subset of the React theme that lives inside the CodeMirror style
 * namespace (popups). The CSS variables on `.lora-query` cover the
 * editor surface itself; the popup theme needs explicit values because
 * tooltips render into `document.body`.
 */
function themeToPopupValues(theme: LoraQueryTheme | undefined) {
  if (!theme) return {};
  const out: Record<string, string> = {};
  if (theme.fontFamily) out.fontFamily = theme.fontFamily;
  if (theme.monoFontFamily) out.monoFontFamily = theme.monoFontFamily;
  if (theme.popupFontSize) out.popupFontSize = theme.popupFontSize;
  if (theme.popupBackground) out.popupBackground = theme.popupBackground;
  if (theme.popupForeground) out.popupForeground = theme.popupForeground;
  if (theme.popupBorder) out.popupBorder = theme.popupBorder;
  if (theme.popupSelectedBackground)
    out.popupSelectedBackground = theme.popupSelectedBackground;
  if (theme.popupSelectedForeground)
    out.popupSelectedForeground = theme.popupSelectedForeground;
  if (theme.popupShadow) out.popupShadow = theme.popupShadow;
  if (theme.muted) out.muted = theme.muted;
  if (theme.errorAccent) out.errorAccent = theme.errorAccent;
  return out;
}

export const LoraQueryEditor = forwardRef<
  LoraQueryEditorHandle,
  LoraQueryEditorProps
>(function LoraQueryEditor(
  {
    value,
    onChange,
    readOnly = false,
    className,
    style,
    theme,
    onDiagnostics,
    onAnalysis,
    onOutline,
    onRun,
    labels,
    relTypes,
    procedures,
    getPropertyKeys,
    colorScheme = "auto",
    showLineNumbers = true,
    placeholder,
    minHeight,
    maxHeight,
    extraExtensions,
    extraKeymap,
  },
  ref,
) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const cmHostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const [hasSelection, setHasSelection] = useState(false);
  const [copied, setCopied] = useState(false);
  const copiedTimerRef = useRef<number | null>(null);
  const extensionsComp = useRef(new Compartment());
  const providersComp = useRef(new Compartment());
  const themeComp = useRef(new Compartment());
  const keymapComp = useRef(new Compartment());
  const extrasComp = useRef(new Compartment());
  // Signature of the last popup-theme we pushed to CodeMirror. Hosts
  // often pass inline `theme={{ ... }}` objects whose identity changes
  // on every render; without this gate every render would reconfigure
  // the popup theme compartment even when nothing visible changed.
  const lastPopupSigRef = useRef<string>("");
  // Last string the editor pushed up via onChange — used to skip the
  // controlled-value round-trip (host echoes the same string back as
  // the new `value`, which would otherwise dispatch a full-doc replace
  // and clobber selection/undo on every keystroke).
  const lastEmittedRef = useRef<string>(value);

  const prettifyFn = useCallback(async () => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    const next = await format(current);
    if (next === current) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: next },
    });
  }, []);

  const onRunRef = useRef(onRun);
  useEffect(() => {
    onRunRef.current = onRun;
  }, [onRun]);
  const runFn = useCallback(() => {
    const view = viewRef.current;
    if (!view) return;
    onRunRef.current?.(view.state.doc.toString());
  }, []);

  useLayoutEffect(() => {
    if (!cmHostRef.current) return;
    const initialProviders = buildProviders(labels, relTypes, procedures, getPropertyKeys);
    const initialPopupValues = themeToPopupValues(theme);
    lastPopupSigRef.current = JSON.stringify(initialPopupValues);
    const view = new EditorView({
      doc: value,
      parent: cmHostRef.current,
      extensions: [
        extensionsComp.current.of(
          cypherExtensions({
            readOnly,
            showLineNumbers,
            ...(placeholder !== undefined && { placeholder }),
          }),
        ),
        themeComp.current.of(popupTheme(initialPopupValues)),
        providersComp.current.of(loraQueryProviders.of(initialProviders)),
        extrasComp.current.of(toExtensionArray(extraExtensions)),
        keymapComp.current.of(
          extraKeymap && extraKeymap.length > 0
            ? Prec.high(keymap.of([...extraKeymap]))
            : [],
        ),
        Prec.high(
          keymap.of([
            {
              key: "Mod-Shift-f",
              run: () => {
                void prettifyFn();
                return true;
              },
            },
            {
              key: "Mod-Enter",
              run: () => {
                runFn();
                return true;
              },
            },
          ]),
        ),
        EditorView.updateListener.of((update) => {
          if (update.docChanged && onChange) {
            const next = update.state.doc.toString();
            lastEmittedRef.current = next;
            onChange(next);
          }
          if (update.selectionSet || update.docChanged) {
            const sel = update.state.selection.main;
            setHasSelection(!sel.empty);
          }
        }),
      ],
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    // Skip the round-trip: when the host echoes the same string we
    // just emitted, we'd otherwise replace the entire doc on every
    // keystroke (destroying selection + filling the undo stack with
    // no-op edits).
    if (lastEmittedRef.current === value) return;
    if (view.state.doc.toString() === value) {
      lastEmittedRef.current = value;
      return;
    }
    lastEmittedRef.current = value;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: value },
    });
  }, [value]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: extensionsComp.current.reconfigure(
        cypherExtensions({ readOnly }),
      ),
    });
  }, [readOnly]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const values = themeToPopupValues(theme);
    // Stable signature over the popup-relevant keys. Skipping the
    // reconfigure when the signature matches avoids dispatching a
    // transaction (and re-rendering popups) for theme objects that
    // were re-created with identical content.
    const sig = JSON.stringify(values);
    if (sig === lastPopupSigRef.current) return;
    lastPopupSigRef.current = sig;
    view.dispatch({
      effects: themeComp.current.reconfigure(popupTheme(values)),
    });
  }, [theme]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: providersComp.current.reconfigure(
        loraQueryProviders.of(buildProviders(labels, relTypes, procedures, getPropertyKeys)),
      ),
    });
  }, [labels, relTypes, procedures, getPropertyKeys]);

  useEffect(() => {
    if (!onDiagnostics && !onAnalysis && !onOutline) return;
    let cancelled = false;
    const view = viewRef.current;
    if (!view) return;
    const doc = view.state.doc.toString();
    if (!doc.trim()) {
      // Fire all three synchronously so React batches into a single
      // re-render of the host.
      onDiagnostics?.([]);
      onAnalysis?.({ diagnostics: [], foldRanges: [] });
      onOutline?.({ variables: [], parameters: [], labels: [], relTypes: [] });
      return;
    }
    // Resolve every requested task first, *then* fire callbacks in a
    // single microtask. With the parser cache (parser.ts) all three
    // requests share their WASM work; the await-then-emit shape lets
    // React 18 batch the three setStates of useLoraQueryStatus into
    // one render rather than three.
    const cfg: import("./parser").AnalyseConfig = {
      strictLabels: (labels?.length ?? 0) > 0,
      strictRelTypes: (relTypes?.length ?? 0) > 0,
    };
    if (labels) cfg.labels = labels;
    if (relTypes) cfg.relTypes = relTypes;
    const diagP = onDiagnostics ? validateAll(doc) : Promise.resolve(null);
    const anaP = onAnalysis ? analyseAll(doc, cfg) : Promise.resolve(null);
    const outP = onOutline ? outline(doc) : Promise.resolve(null);
    Promise.all([diagP, anaP, outP])
      .then(([d, a, o]) => {
        if (cancelled) return;
        if (onDiagnostics && d) onDiagnostics(d);
        if (onAnalysis && a) onAnalysis(a);
        if (onOutline && o) onOutline(o);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [value, onDiagnostics, onAnalysis, onOutline, labels, relTypes]);

  useImperativeHandle(
    ref,
    () => ({
      view: () => viewRef.current,
      prettify: prettifyFn,
      format: prettifyFn,
      run: runFn,
      focus: () => viewRef.current?.focus(),
      copy: async () => {
        const view = viewRef.current;
        if (!view) return false;
        const text = view.state.doc.toString();
        try {
          await navigator.clipboard.writeText(text);
          return true;
        } catch {
          return false;
        }
      },
      validate: async () => {
        const view = viewRef.current;
        if (!view) return [];
        const errors = await validateAll(view.state.doc.toString());
        return errors.map((err) => ({
          from: err.span.start,
          to: err.span.end,
          severity: err.severity,
          message: err.message,
        }));
      },
      getValue: () => viewRef.current?.state.doc.toString() ?? "",
      setValue: (next: string) => {
        const view = viewRef.current;
        if (!view) return;
        if (view.state.doc.toString() === next) {
          lastEmittedRef.current = next;
          return;
        }
        lastEmittedRef.current = next;
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: next },
        });
      },
      getParameters: async () => {
        const view = viewRef.current;
        if (!view) return [];
        const o = await outline(view.state.doc.toString());
        return o.parameters;
      },
      getDeclaredVariables: async () => {
        const view = viewRef.current;
        if (!view) return [];
        const o = await outline(view.state.doc.toString());
        return o.variables.map((v) => v.name);
      },
    }),
    [prettifyFn, runFn],
  );

  // Reconfigure when the extension-affecting props change.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: extensionsComp.current.reconfigure(
        cypherExtensions({
          readOnly,
          showLineNumbers,
          ...(placeholder !== undefined && { placeholder }),
        }),
      ),
    });
  }, [readOnly, showLineNumbers, placeholder]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: extrasComp.current.reconfigure(toExtensionArray(extraExtensions)),
    });
  }, [extraExtensions]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: keymapComp.current.reconfigure(
        extraKeymap && extraKeymap.length > 0
          ? Prec.high(keymap.of([...extraKeymap]))
          : [],
      ),
    });
  }, [extraKeymap]);

  useEffect(() => {
    return () => {
      if (copiedTimerRef.current !== null) {
        window.clearTimeout(copiedTimerRef.current);
      }
    };
  }, []);

  const handleCopyClick = useCallback(async () => {
    const view = viewRef.current;
    if (!view) return;
    const sel = view.state.selection.main;
    const text = sel.empty
      ? view.state.doc.toString()
      : view.state.sliceDoc(sel.from, sel.to);
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      if (copiedTimerRef.current !== null) {
        window.clearTimeout(copiedTimerRef.current);
      }
      copiedTimerRef.current = window.setTimeout(() => {
        setCopied(false);
        copiedTimerRef.current = null;
      }, 1200);
    } catch {
      /* Clipboard refused — silent no-op. */
    }
  }, []);

  const showCopyButton = readOnly || hasSelection;

  const containerStyle = useMemo<CSSProperties>(
    () => ({
      ...themeToStyle(theme),
      ...(minHeight !== undefined && { minHeight }),
      ...(maxHeight !== undefined && { maxHeight }),
      ...(style ?? {}),
    }),
    [theme, style, minHeight, maxHeight],
  );

  return (
    <div
      ref={hostRef}
      className={["lora-query", className].filter(Boolean).join(" ")}
      data-color-scheme={colorScheme === "auto" ? undefined : colorScheme}
      style={containerStyle}
    >
      <div ref={cmHostRef} style={{ display: "contents" }} />
      {(showCopyButton || !readOnly) && (
        <div className="lora-query__actions">
          {!readOnly && (
            <button
              type="button"
              className="lora-query__action lora-query__format"
              aria-label="Format query"
              title="Format query (⇧⌥F)"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                void prettifyFn();
              }}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M6 21l15 -15l-3 -3l-15 15l3 3" />
                <path d="M15 6l3 3" />
                <path d="M9 3a2 2 0 0 0 2 2a2 2 0 0 0 -2 2a2 2 0 0 0 -2 -2a2 2 0 0 0 2 -2" />
                <path d="M19 13a2 2 0 0 0 2 2a2 2 0 0 0 -2 2a2 2 0 0 0 -2 -2a2 2 0 0 0 2 -2" />
              </svg>
            </button>
          )}
          {showCopyButton && (
            <button
              type="button"
              className={`lora-query__action lora-query__copy${copied ? " lora-query__copy--copied" : ""}`}
              aria-label={copied ? "Copied" : "Copy"}
              title={copied ? "Copied" : hasSelection ? "Copy selection" : "Copy"}
              onMouseDown={(e) => e.preventDefault()}
              onClick={handleCopyClick}
            >
              {copied ? (
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <polyline points="20 6 9 17 4 12" />
                </svg>
              ) : (
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                </svg>
              )}
            </button>
          )}
        </div>
      )}
    </div>
  );
});
