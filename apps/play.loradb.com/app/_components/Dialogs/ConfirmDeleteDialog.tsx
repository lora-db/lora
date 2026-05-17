"use client";

/**
 * `ConfirmDeleteDialog` — Mantine modal that gates node / link removal
 * coming out of `LoraGraphCanvas`. Invoked from `GraphView.tsx` through
 * the canvas's `onBeforeNodeDelete` / `onBeforeLinkDelete` guards, which
 * expect a `Promise<boolean>`.
 *
 * On confirm we issue an atomic Cypher transaction
 * (`DELETE r` + `DETACH DELETE n`) against the in-memory LoraDB via
 * `deleteFromGraph`. The canvas's in-memory view is only updated after
 * the database accepts the change, so view and DB stay in sync. Saving
 * a snapshot is what makes the deletion durable across reloads.
 */

import { useState } from "react";
import {
  Button,
  Code,
  Divider,
  Group,
  List,
  Stack,
  Text,
} from "@mantine/core";
import { modals } from "@mantine/modals";
import type {
  DeletionSource,
  LinkObject,
  NodeObject,
} from "@loradb/lora-graph-canvas";

import { deleteFromGraph } from "@/lib/actions/deleteActions";

interface ConfirmDeleteDialogProps {
  modalId: string;
  nodes: NodeObject[];
  links: LinkObject[];
  source: DeletionSource;
  resolve: (allow: boolean) => void;
}

const MAX_PREVIEW = 8;

function nodeLabel(n: NodeObject): string {
  const label =
    (typeof n.label === "string" && n.label.length > 0 && n.label) ||
    (typeof n.group === "string" && n.group.length > 0 && n.group) ||
    null;
  return label ? `${label} · ${String(n.id)}` : String(n.id);
}

function linkLabel(l: LinkObject): string {
  const type =
    (typeof l.label === "string" && l.label.length > 0 && l.label) ||
    "RELATED_TO";
  const src =
    typeof l.source === "object" && l.source !== null
      ? String((l.source as NodeObject).id)
      : String(l.source);
  const tgt =
    typeof l.target === "object" && l.target !== null
      ? String((l.target as NodeObject).id)
      : String(l.target);
  return `(${src}) -[:${type}]-> (${tgt})`;
}

function buildCypher(nodes: NodeObject[], links: LinkObject[]): string {
  const lines: string[] = [];
  if (links.length > 0) {
    const ids = links
      .map((l) => l.id)
      .filter((id): id is string | number => id !== undefined);
    if (ids.length > 0) {
      lines.push(
        `MATCH ()-[r]-() WHERE id(r) IN [${ids
          .map((id) => JSON.stringify(id))
          .join(", ")}] DELETE r;`,
      );
    }
  }
  if (nodes.length > 0) {
    const ids = nodes.map((n) => JSON.stringify(n.id)).join(", ");
    // DETACH so attached relationships go too — mirrors the canvas's
    // cascade behaviour on node removal.
    lines.push(`MATCH (n) WHERE id(n) IN [${ids}] DETACH DELETE n;`);
  }
  return lines.length > 0
    ? lines.join("\n")
    : "// nothing selected — this guard call is a no-op";
}

function ConfirmDeleteDialog({
  modalId,
  nodes,
  links,
  source,
  resolve,
}: ConfirmDeleteDialogProps) {
  const totalNodes = nodes.length;
  const totalLinks = links.length;
  const summary = (() => {
    const bits: string[] = [];
    if (totalNodes > 0) {
      bits.push(`${totalNodes} node${totalNodes === 1 ? "" : "s"}`);
    }
    if (totalLinks > 0) {
      bits.push(
        `${totalLinks} relationship${totalLinks === 1 ? "" : "s"}`,
      );
    }
    return bits.length > 0 ? bits.join(" and ") : "nothing";
  })();

  const [busy, setBusy] = useState(false);

  const cancel = (): void => {
    resolve(false);
    modals.close(modalId);
  };

  const confirm = async (): Promise<void> => {
    setBusy(true);
    const outcome = await deleteFromGraph(nodes, links);
    if (!outcome.ok) {
      setBusy(false);
      // Keep the dialog open so the user can retry or cancel — the
      // notification surfaced the underlying error.
      return;
    }
    resolve(true);
    modals.close(modalId);
  };

  const nodePreview = nodes.slice(0, MAX_PREVIEW);
  const nodeExtra = totalNodes - nodePreview.length;
  const linkPreview = links.slice(0, MAX_PREVIEW);
  const linkExtra = totalLinks - linkPreview.length;

  return (
    <Stack gap="sm">
      <Text size="sm">
        Remove {summary} from the database? Triggered by{" "}
        <Code>{source}</Code>.
      </Text>

      {totalNodes > 0 && (
        <Stack gap={4}>
          <Text size="xs" c="dimmed" tt="uppercase" fw={600}>
            Nodes
          </Text>
          <List size="xs" spacing={2} withPadding>
            {nodePreview.map((n) => (
              <List.Item key={String(n.id)}>
                <Text size="xs" ff="monospace">
                  {nodeLabel(n)}
                </Text>
              </List.Item>
            ))}
            {nodeExtra > 0 && (
              <List.Item>
                <Text size="xs" c="dimmed">
                  …and {nodeExtra} more
                </Text>
              </List.Item>
            )}
          </List>
        </Stack>
      )}

      {totalLinks > 0 && (
        <Stack gap={4}>
          <Text size="xs" c="dimmed" tt="uppercase" fw={600}>
            Relationships
          </Text>
          <List size="xs" spacing={2} withPadding>
            {linkPreview.map((l, i) => (
              <List.Item key={l.id !== undefined ? String(l.id) : `lp-${i}`}>
                <Text size="xs" ff="monospace">
                  {linkLabel(l)}
                </Text>
              </List.Item>
            ))}
            {linkExtra > 0 && (
              <List.Item>
                <Text size="xs" c="dimmed">
                  …and {linkExtra} more
                </Text>
              </List.Item>
            )}
          </List>
        </Stack>
      )}

      <Divider />

      <Stack gap={4}>
        <Text size="xs" c="dimmed" tt="uppercase" fw={600}>
          Equivalent Cypher
        </Text>
        <Code block style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
          {buildCypher(nodes, links)}
        </Code>
      </Stack>

      <Text size="xs" c="dimmed">
        This runs against the in-memory LoraDB session. Save a snapshot
        afterwards to persist the deletion across reloads.
      </Text>

      <Group justify="flex-end" gap="xs" mt="xs">
        <Button
          variant="default"
          size="xs"
          onClick={cancel}
          disabled={busy}
          data-autofocus
        >
          Cancel
        </Button>
        <Button
          size="xs"
          color="red"
          loading={busy}
          onClick={() => {
            void confirm();
          }}
        >
          Remove
        </Button>
      </Group>
    </Stack>
  );
}

interface PendingBatch {
  nodes: NodeObject[];
  links: LinkObject[];
  source: DeletionSource;
  resolves: Array<(allow: boolean) => void>;
}

// Mixed-selection deletes arrive as two concurrent guard calls
// (`onBeforeNodeDelete` + `onBeforeLinkDelete`) from the canvas in the
// same microtask. Opening two modals with the same id would let the
// second replace the first, firing the first's `onClose` and resolving
// the node-delete guard with `false` — so only the links would be
// removed. We batch calls within a microtask into a single combined
// modal whose result resolves every queued guard with the same value.
let pendingBatch: PendingBatch | null = null;

function presentBatch(batch: PendingBatch): void {
  const id = "loradb-confirm-delete-dialog";
  let settled = false;
  const settle = (allow: boolean): void => {
    if (settled) return;
    settled = true;
    for (const resolve of batch.resolves) resolve(allow);
  };
  modals.open({
    modalId: id,
    title: "Remove from graph?",
    centered: true,
    onClose: () => {
      settle(false);
    },
    children: (
      <ConfirmDeleteDialog
        modalId={id}
        nodes={batch.nodes}
        links={batch.links}
        source={batch.source}
        resolve={settle}
      />
    ),
  });
}

/** Opens the confirm-delete modal and returns a promise that resolves
 *  to `true` when the user confirms removal, `false` otherwise. The
 *  promise also resolves `false` if the modal is dismissed without an
 *  explicit choice (clicking outside, pressing Esc, etc.). */
export function openConfirmDeleteDialog(opts: {
  nodes: NodeObject[];
  links: LinkObject[];
  source: DeletionSource;
}): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    if (pendingBatch !== null) {
      pendingBatch.nodes.push(...opts.nodes);
      pendingBatch.links.push(...opts.links);
      pendingBatch.resolves.push(resolve);
      return;
    }
    const batch: PendingBatch = {
      nodes: [...opts.nodes],
      links: [...opts.links],
      source: opts.source,
      resolves: [resolve],
    };
    pendingBatch = batch;
    queueMicrotask(() => {
      pendingBatch = null;
      presentBatch(batch);
    });
  });
}
