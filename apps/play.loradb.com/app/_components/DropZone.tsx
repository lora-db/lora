"use client";

/**
 * Global drag-and-drop overlay for `.lorasnap` imports.
 *
 * Listens on `window` for `dragenter`/`dragover`/`dragleave`/`drop`, shows
 * a full-page accent-tinted overlay while a file is being dragged, and
 * — on drop — imports the file as a new snapshot and immediately loads
 * it into the live DB. The same pipeline the Snapshots panel uses.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { Center, Paper, Stack, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { IconUpload } from "@tabler/icons-react";

import {
  importSnapshotFromFile,
  loadSnapshotById,
} from "@/lib/actions/snapshotActions";
import { hexA } from "@/lib/theme/util";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

const LORASNAP_RE = /\.lorasnap$/i;

function pickLorasnapFile(list: FileList | null | undefined): File | null {
  if (!list) return null;
  for (let i = 0; i < list.length; i++) {
    const f = list.item(i);
    if (f && LORASNAP_RE.test(f.name)) return f;
  }
  return null;
}

export function DropZone() {
  const { tokens } = usePlaygroundTheme();
  const [dragging, setDragging] = useState(false);

  // Track the depth of dragenter/dragleave so we don't flicker when the
  // pointer crosses child element boundaries.
  const depth = useRef(0);

  const handleDrop = useCallback(async (file: File) => {
    const name = file.name.replace(LORASNAP_RE, "") || "snapshot";
    try {
      const record = await importSnapshotFromFile(file, name);
      await loadSnapshotById(record.id);
      notifications.show({
        color: "green",
        title: "Snapshot imported",
        message: `Loaded "${record.name}" (${record.sizeBytes.toLocaleString()} B).`,
      });
    } catch (err) {
      notifications.show({
        color: "red",
        title: "Import failed",
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }, []);

  useEffect(() => {
    if (typeof window === "undefined") return undefined;

    const onEnter = (e: DragEvent) => {
      if (!e.dataTransfer) return;
      // Only treat as a file drag (ignore text/element drags).
      const hasFiles =
        e.dataTransfer.types &&
        Array.from(e.dataTransfer.types).includes("Files");
      if (!hasFiles) return;
      depth.current += 1;
      setDragging(true);
      e.preventDefault();
    };
    const onOver = (e: DragEvent) => {
      if (!e.dataTransfer) return;
      const hasFiles =
        e.dataTransfer.types &&
        Array.from(e.dataTransfer.types).includes("Files");
      if (!hasFiles) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "copy";
    };
    const onLeave = (e: DragEvent) => {
      depth.current = Math.max(0, depth.current - 1);
      if (depth.current === 0) {
        setDragging(false);
      }
      e.preventDefault();
    };
    const onDrop = (e: DragEvent) => {
      e.preventDefault();
      depth.current = 0;
      setDragging(false);
      const file = pickLorasnapFile(e.dataTransfer?.files ?? null);
      if (!file) {
        // Only show a complaint if the user actually dropped *something*.
        if ((e.dataTransfer?.files?.length ?? 0) > 0) {
          notifications.show({
            color: "yellow",
            title: "Unsupported file",
            message: "Drop a .lorasnap snapshot to import.",
          });
        }
        return;
      }
      void handleDrop(file);
    };

    window.addEventListener("dragenter", onEnter);
    window.addEventListener("dragover", onOver);
    window.addEventListener("dragleave", onLeave);
    window.addEventListener("drop", onDrop);
    return () => {
      window.removeEventListener("dragenter", onEnter);
      window.removeEventListener("dragover", onOver);
      window.removeEventListener("dragleave", onLeave);
      window.removeEventListener("drop", onDrop);
    };
  }, [handleDrop]);

  if (!dragging) return null;

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        pointerEvents: "none",
        background: hexA(tokens.accent.primary, 0.18),
        border: `2px dashed ${tokens.accent.primary}`,
        zIndex: 9000,
      }}
    >
      <Center h="100%">
        <Paper
          shadow="md"
          radius="md"
          p="lg"
          withBorder
          style={{
            background: tokens.bg.panel,
            borderColor: tokens.accent.primary,
          }}
        >
          <Stack align="center" gap={8}>
            <IconUpload size={32} color={tokens.accent.primary} />
            <Text size="sm" fw={600} c={tokens.fg.primary}>
              Drop a .lorasnap file to import
            </Text>
            <Text size="xs" c={tokens.fg.muted}>
              The snapshot is saved and loaded automatically.
            </Text>
          </Stack>
        </Paper>
      </Center>
    </div>
  );
}
