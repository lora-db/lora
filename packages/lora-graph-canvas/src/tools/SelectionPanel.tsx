import { IconAddConnected, IconDelete, IconDuplicate } from "./icons";

/** Inline icon for "copy". We reuse two stacked squares — similar to
 *  the duplicate icon but distinct enough to read. Keeping these
 *  local since they're only used by the SelectionPanel. */
function IconCopy(p: React.SVGProps<SVGSVGElement>) {
  return (
    <svg
      width={14}
      height={14}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      {...p}
    >
      <rect x="3" y="3" width="8" height="8" rx="1" />
      <path d="M6 5h7v7" />
    </svg>
  );
}

function IconCut(p: React.SVGProps<SVGSVGElement>) {
  return (
    <svg
      width={14}
      height={14}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      {...p}
    >
      <circle cx="5" cy="11" r="2" />
      <circle cx="11" cy="11" r="2" />
      <path d="M6.5 9.5L13 3M5 8L9.5 9.5" />
    </svg>
  );
}

function IconPaste(p: React.SVGProps<SVGSVGElement>) {
  return (
    <svg
      width={14}
      height={14}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      {...p}
    >
      <rect x="3" y="4" width="10" height="10" rx="1" />
      <rect x="5" y="2" width="6" height="3" rx="1" />
    </svg>
  );
}

export interface SelectionPanelProps {
  nodeCount: number;
  linkCount: number;
  hasClipboard: boolean;
  enableClipboard: boolean;
  onDelete(): void;
  onDuplicate(): void;
  onAddConnected(): void;
  onCopy(): void;
  onCut(): void;
  onPaste(): void;
  onClear(): void;
}

interface PanelButtonProps {
  label: string;
  shortcut?: string;
  disabled?: boolean;
  onClick(): void;
  children: React.ReactNode;
}

function PanelButton({
  label,
  shortcut,
  disabled,
  onClick,
  children,
}: PanelButtonProps) {
  return (
    <button
      type="button"
      className="lgc-selpanel-btn"
      aria-label={label}
      title={shortcut ? `${label} (${shortcut})` : label}
      disabled={disabled}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

/** Inline summary that sits in the top-left of the canvas while
 *  anything is selected. Surfaces the count + the most common
 *  selection-scoped actions so the user doesn't have to reach for
 *  the right-side toolbar. Hidden when nothing is selected. */
export function SelectionPanel({
  nodeCount,
  linkCount,
  hasClipboard,
  enableClipboard,
  onDelete,
  onDuplicate,
  onAddConnected,
  onCopy,
  onCut,
  onPaste,
  onClear,
}: SelectionPanelProps) {
  if (nodeCount === 0 && linkCount === 0) {
    // Even with nothing selected, expose paste when there's clipboard
    // content so the user can drop the copy somewhere new.
    if (!enableClipboard || !hasClipboard) return null;
    return (
      <div className="lgc-selpanel" role="region" aria-label="Selection">
        <span className="lgc-selpanel-label">Clipboard ready</span>
        <PanelButton label="Paste" shortcut="⌘V" onClick={onPaste}>
          <IconPaste />
        </PanelButton>
      </div>
    );
  }

  // Build a compact summary: "3 nodes", "2 links", or both.
  const parts: string[] = [];
  if (nodeCount > 0) {
    parts.push(`${nodeCount} ${nodeCount === 1 ? "node" : "nodes"}`);
  }
  if (linkCount > 0) {
    parts.push(`${linkCount} ${linkCount === 1 ? "link" : "links"}`);
  }
  const summary = parts.join(", ");

  return (
    <div className="lgc-selpanel" role="region" aria-label="Selection">
      <span className="lgc-selpanel-label">{summary}</span>
      <div className="lgc-selpanel-divider" />
      <PanelButton label="Delete" shortcut="⌫" onClick={onDelete}>
        <IconDelete width={14} height={14} />
      </PanelButton>
      <PanelButton label="Duplicate" shortcut="⌘D" onClick={onDuplicate}>
        <IconDuplicate width={14} height={14} />
      </PanelButton>
      <PanelButton
        label="Connect to new node"
        shortcut="↵"
        disabled={nodeCount === 0}
        onClick={onAddConnected}
      >
        <IconAddConnected />
      </PanelButton>
      {enableClipboard ? (
        <>
          <PanelButton
            label="Copy"
            shortcut="⌘C"
            disabled={nodeCount === 0}
            onClick={onCopy}
          >
            <IconCopy />
          </PanelButton>
          <PanelButton
            label="Cut"
            shortcut="⌘X"
            disabled={nodeCount === 0}
            onClick={onCut}
          >
            <IconCut />
          </PanelButton>
          <PanelButton
            label="Paste"
            shortcut="⌘V"
            disabled={!hasClipboard}
            onClick={onPaste}
          >
            <IconPaste />
          </PanelButton>
        </>
      ) : null}
      <div className="lgc-selpanel-divider" />
      <PanelButton label="Clear selection" shortcut="Esc" onClick={onClear}>
        ✕
      </PanelButton>
    </div>
  );
}
