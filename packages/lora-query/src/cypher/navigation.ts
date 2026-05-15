import { EditorSelection } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { findVariable, getOutline } from "./scope";

/**
 * Locate the word under the cursor and return `[from, to]` if any.
 */
function wordAt(view: EditorView, pos: number): [number, number] | null {
  const line = view.state.doc.lineAt(pos);
  const { from, text } = line;
  const offset = pos - from;
  let start = offset;
  let end = offset;
  while (start > 0 && /\w/.test(text[start - 1] ?? "")) start--;
  while (end < text.length && /\w/.test(text[end] ?? "")) end++;
  if (start === end) return null;
  return [from + start, from + end];
}

/** Move the cursor to the declaration of the variable under it. */
function jumpToDeclaration(view: EditorView): boolean {
  const pos = view.state.selection.main.head;
  const range = wordAt(view, pos);
  if (!range) return false;
  const name = view.state.doc.sliceString(range[0], range[1]);
  const outline = getOutline(view.state);
  const v = findVariable(outline, name);
  if (!v) return false;
  if (v.declStart === range[0]) return false; // already at declaration
  view.dispatch({
    selection: EditorSelection.single(v.declStart, v.declEnd),
    scrollIntoView: true,
  });
  return true;
}

/**
 * Keymap entries: F12 / Mod-Click → jump to declaration. The click
 * handler is registered separately by the editor since CodeMirror
 * keymap doesn't cover pointer events.
 */
export const cypherNavigationKeymap = keymap.of([
  {
    key: "F12",
    run: jumpToDeclaration,
  },
  {
    key: "Mod-d",
    run: jumpToDeclaration,
  },
]);

/**
 * Mod-Click handler: turns the dom into a clickable jump-to-declaration
 * surface when the user holds Cmd/Ctrl.
 */
export const cypherNavigationClick = EditorView.domEventHandlers({
  mousedown(event, view) {
    if (!event.metaKey && !event.ctrlKey) return false;
    const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
    if (pos === null) return false;
    view.dispatch({
      selection: EditorSelection.cursor(pos),
    });
    const handled = jumpToDeclaration(view);
    if (handled) {
      event.preventDefault();
      return true;
    }
    return false;
  },
});

export const cypherNavigation = [cypherNavigationKeymap, cypherNavigationClick];
