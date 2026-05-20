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
import { jsonParseLinter } from "@codemirror/lang-json";
import { jsonExtensions } from "./json/extensions";
import { jsonPopupTheme } from "./json/theme";
import { loraJsonProviders, type LoraJsonProviders } from "./json/completion";
import {
  keyConstraintsFacet,
  type KeyConstraints,
} from "./json/keyConstraints";
import { formatJson, minifyJson } from "./json/format";
import {
  foldAllCmd,
  sortKeysCmd,
  toggleQuotesCmd,
  unfoldAllCmd,
} from "./json/commands";
import { getJsonPath, type PathSegment } from "./json/path";
import { JSON_THEME_TO_VAR, type LoraJsonTheme } from "./jsonThemes";
import "./editor.css";

export type { LoraJsonTheme } from "./jsonThemes";
export type { PathSegment } from "./json/path";

export interface LoraJsonEditorProps {
  value: string;
  onChange?: (next: string) => void;
  readOnly?: boolean;
  className?: string;
  /** Inline style applied to the outer container. */
  style?: CSSProperties;
  /** CSS-variable theme overrides — see {@link LoraJsonTheme}. */
  theme?: LoraJsonTheme;
  /** Called whenever the JSON linter has new diagnostics. */
  onDiagnostics?: (diagnostics: Diagnostic[]) => void;
  /** Called whenever the cursor moves, with the new JSON path. */
  onCursorPath?: (path: PathSegment[]) => void;
  /**
   * Fired on `Cmd/Ctrl + Enter`. Hosts typically wire this to
   * "run the query with this payload". The current source string
   * is passed in.
   */
  onRun?: (source: string) => void;

  /**
   * Top-level keys the host wants surfaced in the autocomplete
   * popup. Defaults to `allowedKeys` when that prop is set.
   */
  knownKeys?: readonly string[];

  /**
   * When set, the only valid top-level keys. Extra keys are
   * flagged as a lint error. Pair with the autocomplete to lock
   * the payload down to a specific parameter set.
   */
  allowedKeys?: readonly string[];

  /**
   * Top-level keys that must always be present. Missing keys
   * surface as a lint warning anchored at the closing `}`.
   */
  requiredKeys?: readonly string[];

  /**
   * Force a color scheme regardless of the host's
   * `prefers-color-scheme` setting. Defaults to `"auto"`.
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
   * Indent width passed to the prettifier (`JSON.stringify(_, null, indent)`).
   * Defaults to `2`.
   */
  indent?: number;
  /**
   * Auto-prettify when the user pastes content that parses as
   * valid JSON. Defaults to `false` — opt-in.
   */
  formatOnPaste?: boolean;
  /**
   * Extra CodeMirror extensions to merge in. Appended to the default
   * bundle so the host can add overlays, custom lint sources, etc.
   */
  extraExtensions?: Extension | readonly Extension[];
  /**
   * Extra keybindings, merged at `Prec.high` so host shortcuts win
   * over the defaults.
   */
  extraKeymap?: readonly KeyBinding[];
}

export interface LoraJsonEditorHandle {
  /** Reformat the current buffer in place via the JSON prettifier. */
  prettify: () => Promise<void>;
  /** Alias for {@link prettify}. */
  format: () => Promise<void>;
  /** Minify the current buffer in place (single-line output). */
  minify: () => Promise<void>;
  /** Recursively sort every object's keys alphabetically. */
  sortKeys: () => void;
  /** Convert single-quoted strings to double-quoted (strict JSON repair). */
  toggleQuotes: () => void;
  /** Collapse every foldable range. */
  foldAll: () => void;
  /** Expand every folded range. */
  unfoldAll: () => void;
  /** Run a one-shot parse check and return the diagnostics. */
  validate: () => Promise<Diagnostic[]>;
  /** Current source code. */
  getValue: () => string;
  /** Parse the buffer, or `undefined` when invalid. */
  getJson: () => unknown | undefined;
  /** JSON path at the current cursor position. */
  getCursorPath: () => PathSegment[];
  /** Imperatively replace the editor content. */
  setValue: (next: string) => void;
  /** Set the editor content from a JS value (stringified + prettified). */
  setJson: (value: unknown) => void;
  /** Trigger the host-provided `onRun` callback with the current source. */
  run: () => void;
  /** Copy the current buffer to the clipboard. */
  copy: () => Promise<boolean>;
  /** Move keyboard focus to the editor. */
  focus: () => void;
  /** Direct access to the underlying CodeMirror view. */
  view: () => EditorView | null;
}

function toExtensionArray(
  extras: Extension | readonly Extension[] | undefined,
): Extension[] {
  if (!extras) return [];
  return (Array.isArray(extras) ? [...extras] : [extras]) as Extension[];
}

function themeToStyle(theme: LoraJsonTheme | undefined): CSSProperties {
  if (!theme) return {};
  const out: Record<string, string> = {};
  for (const k of Object.keys(theme) as (keyof LoraJsonTheme)[]) {
    const v = theme[k];
    if (v) out[JSON_THEME_TO_VAR[k]] = v;
  }
  return out as CSSProperties;
}

function themeToPopupValues(theme: LoraJsonTheme | undefined) {
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

function buildProviders(
  knownKeys: readonly string[] | undefined,
  allowedKeys: readonly string[] | undefined,
): LoraJsonProviders {
  // `knownKeys` explicitly wins; otherwise fall through to
  // `allowedKeys` so locking the payload also drives autocomplete.
  return { knownKeys: knownKeys ?? allowedKeys ?? [] };
}

function buildKeyConstraints(
  allowedKeys: readonly string[] | undefined,
  requiredKeys: readonly string[] | undefined,
): KeyConstraints {
  const out: KeyConstraints = {};
  if (allowedKeys) out.allowedKeys = allowedKeys;
  if (requiredKeys) out.requiredKeys = requiredKeys;
  return out;
}

/**
 * Full-featured JSON editor — peer to `LoraQueryEditor`. Same theming
 * surface, same imperative handle shape. Use it for filling in query
 * parameter payloads (editable, optionally locked to known keys) or
 * for displaying query results (`readOnly={true}`).
 */
export const LoraJsonEditor = forwardRef<
  LoraJsonEditorHandle,
  LoraJsonEditorProps
>(function LoraJsonEditor(
  {
    value,
    onChange,
    readOnly = false,
    className,
    style,
    theme,
    onDiagnostics,
    onCursorPath,
    onRun,
    knownKeys,
    allowedKeys,
    requiredKeys,
    colorScheme = "auto",
    showLineNumbers = true,
    placeholder,
    minHeight,
    maxHeight,
    indent = 2,
    formatOnPaste = false,
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
  const constraintsComp = useRef(new Compartment());
  const themeComp = useRef(new Compartment());
  const keymapComp = useRef(new Compartment());
  const extrasComp = useRef(new Compartment());
  const lastPopupSigRef = useRef<string>("");
  const lastEmittedRef = useRef<string>(value);

  const indentRef = useRef(indent);
  useEffect(() => {
    indentRef.current = indent;
  }, [indent]);

  const formatOnPasteRef = useRef(formatOnPaste);
  useEffect(() => {
    formatOnPasteRef.current = formatOnPaste;
  }, [formatOnPaste]);

  const prettifyFn = useCallback(async () => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    const next = formatJson(current, indentRef.current);
    if (next === current) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: next },
    });
  }, []);

  const minifyFn = useCallback(async () => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    const next = minifyJson(current);
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

  const onCursorPathRef = useRef(onCursorPath);
  useEffect(() => {
    onCursorPathRef.current = onCursorPath;
  }, [onCursorPath]);
  // Memoised last-emitted path so we don't churn the host with
  // identical arrays.
  const lastPathSigRef = useRef<string>("");

  useLayoutEffect(() => {
    if (!cmHostRef.current) return;
    const initialProviders = buildProviders(knownKeys, allowedKeys);
    const initialConstraints = buildKeyConstraints(allowedKeys, requiredKeys);
    const initialPopupValues = themeToPopupValues(theme);
    lastPopupSigRef.current = JSON.stringify(initialPopupValues);

    // Paste handler — runs at the DOM event level so we can replace
    // the inbound text *before* CodeMirror sees it. Only triggers
    // when the pasted content is, on its own, a complete + valid
    // JSON document; anything else falls through to the default
    // paste behaviour.
    const pasteHandler = EditorView.domEventHandlers({
      paste(event, view) {
        if (!formatOnPasteRef.current) return false;
        const text = event.clipboardData?.getData("text/plain") ?? "";
        if (!text.trim()) return false;
        let parsed: unknown;
        try {
          parsed = JSON.parse(text);
        } catch {
          return false;
        }
        const replacement = JSON.stringify(parsed, null, indentRef.current);
        const sel = view.state.selection.main;
        event.preventDefault();
        view.dispatch({
          changes: { from: sel.from, to: sel.to, insert: replacement },
          selection: { anchor: sel.from + replacement.length },
          userEvent: "input.paste.format",
        });
        return true;
      },
    });

    const view = new EditorView({
      doc: value,
      parent: cmHostRef.current,
      extensions: [
        extensionsComp.current.of(
          jsonExtensions({
            readOnly,
            showLineNumbers,
            ...(placeholder !== undefined && { placeholder }),
          }),
        ),
        themeComp.current.of(jsonPopupTheme(initialPopupValues)),
        providersComp.current.of(loraJsonProviders.of(initialProviders)),
        constraintsComp.current.of(keyConstraintsFacet.of(initialConstraints)),
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
            {
              key: "Alt-Shift-s",
              run: () => {
                const v = viewRef.current;
                if (!v) return false;
                sortKeysCmd(v, indentRef.current);
                return true;
              },
            },
          ]),
        ),
        pasteHandler,
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
          // Recompute cursor path on either a doc change or a
          // selection change, but only emit when the path actually
          // changed.
          if (
            onCursorPathRef.current &&
            (update.docChanged || update.selectionSet)
          ) {
            const pos = update.state.selection.main.head;
            const path = getJsonPath(update.state.doc.toString(), pos);
            const sig = JSON.stringify(path);
            if (sig !== lastPathSigRef.current) {
              lastPathSigRef.current = sig;
              onCursorPathRef.current(path);
            }
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
    const values = themeToPopupValues(theme);
    const sig = JSON.stringify(values);
    if (sig === lastPopupSigRef.current) return;
    lastPopupSigRef.current = sig;
    view.dispatch({
      effects: themeComp.current.reconfigure(jsonPopupTheme(values)),
    });
  }, [theme]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: providersComp.current.reconfigure(
        loraJsonProviders.of(buildProviders(knownKeys, allowedKeys)),
      ),
    });
  }, [knownKeys, allowedKeys]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: constraintsComp.current.reconfigure(
        keyConstraintsFacet.of(buildKeyConstraints(allowedKeys, requiredKeys)),
      ),
    });
  }, [allowedKeys, requiredKeys]);

  useEffect(() => {
    if (!onDiagnostics) return;
    let cancelled = false;
    const view = viewRef.current;
    if (!view) return;
    const doc = view.state.doc.toString();
    if (!doc.trim()) {
      onDiagnostics([]);
      return;
    }
    Promise.resolve()
      .then(() => {
        if (cancelled) return;
        const lint = jsonParseLinter();
        onDiagnostics(lint(view));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [value, onDiagnostics]);

  useImperativeHandle(
    ref,
    () => ({
      view: () => viewRef.current,
      prettify: prettifyFn,
      format: prettifyFn,
      minify: minifyFn,
      sortKeys: () => {
        const v = viewRef.current;
        if (!v) return;
        sortKeysCmd(v, indentRef.current);
      },
      toggleQuotes: () => {
        const v = viewRef.current;
        if (!v) return;
        toggleQuotesCmd(v);
      },
      foldAll: () => {
        const v = viewRef.current;
        if (!v) return;
        foldAllCmd(v);
      },
      unfoldAll: () => {
        const v = viewRef.current;
        if (!v) return;
        unfoldAllCmd(v);
      },
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
        const lint = jsonParseLinter();
        return lint(view);
      },
      getValue: () => viewRef.current?.state.doc.toString() ?? "",
      getJson: () => {
        const view = viewRef.current;
        if (!view) return undefined;
        try {
          return JSON.parse(view.state.doc.toString());
        } catch {
          return undefined;
        }
      },
      getCursorPath: () => {
        const view = viewRef.current;
        if (!view) return [];
        const pos = view.state.selection.main.head;
        return getJsonPath(view.state.doc.toString(), pos);
      },
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
      setJson: (val: unknown) => {
        const view = viewRef.current;
        if (!view) return;
        const text = JSON.stringify(val, null, indentRef.current);
        if (view.state.doc.toString() === text) {
          lastEmittedRef.current = text;
          return;
        }
        lastEmittedRef.current = text;
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: text },
        });
      },
    }),
    [prettifyFn, minifyFn, runFn],
  );

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: extensionsComp.current.reconfigure(
        jsonExtensions({
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
      effects: extrasComp.current.reconfigure(
        toExtensionArray(extraExtensions),
      ),
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
      className={["lora-query", "lora-json", className]
        .filter(Boolean)
        .join(" ")}
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
              aria-label="Format JSON"
              title="Format JSON (⇧⌥F)"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                void prettifyFn();
              }}
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
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
              title={
                copied ? "Copied" : hasSelection ? "Copy selection" : "Copy"
              }
              onMouseDown={(e) => e.preventDefault()}
              onClick={handleCopyClick}
            >
              {copied ? (
                <svg
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  aria-hidden="true"
                >
                  <polyline points="20 6 9 17 4 12" />
                </svg>
              ) : (
                <svg
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  aria-hidden="true"
                >
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
