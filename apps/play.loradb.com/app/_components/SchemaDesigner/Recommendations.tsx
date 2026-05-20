"use client";

/**
 * Recommendation cards. Reads the local history of recent queries
 * and runs the heuristic engine to surface "you probably want to add
 * this" suggestions. Each card has a primary CTA that pre-fills the
 * matching wizard and a dismiss button that persists in-session.
 */

import { useEffect, useMemo, useState } from "react";
import {
  ActionIcon,
  Badge,
  Button,
  Group,
  Paper,
  Stack,
  Text,
  Tooltip,
} from "@mantine/core";
import {
  IconBolt,
  IconKey,
  IconPrismLight,
  IconRefresh,
  IconX,
} from "@tabler/icons-react";

import { useStore } from "@/lib/state/store";
import { generateRecommendations } from "@/lib/schemaDesign/recommend";
import type {
  ConstraintDef,
  IndexDef,
  Recommendation,
  RecommendationKind,
} from "@/lib/schemaDesign/types";

const EMPTY_INDEXES: IndexDef[] = [];
const EMPTY_CONSTRAINTS: ConstraintDef[] = [];
import * as historyStore from "@/lib/persistence/history";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

const KIND_BADGE: Record<RecommendationKind, { label: string; color: string }> =
  {
    RANGE_INDEX: { label: "RANGE INDEX", color: "blue" },
    TEXT_INDEX: { label: "TEXT INDEX", color: "teal" },
    UNIQUE_CONSTRAINT: { label: "UNIQUE", color: "grape" },
    NOT_NULL_CONSTRAINT: { label: "NOT NULL", color: "grape" },
  };

export function Recommendations() {
  const { tokens } = usePlaygroundTheme();
  const indexes = useStore((s) => s.indexes ?? EMPTY_INDEXES);
  const constraints = useStore((s) => s.constraints ?? EMPTY_CONSTRAINTS);
  const dismissed = useStore((s) => s.dismissedRecs);
  const dismiss = useStore((s) => s.dismissRecommendation);
  const restore = useStore((s) => s.restoreRecommendations);
  const openNewIndex = useStore((s) => s.openNewIndexWizard);
  const openNewConstraint = useStore((s) => s.openNewConstraintWizard);

  const [historyBodies, setHistoryBodies] = useState<
    { body: string; ok: boolean }[]
  >([]);
  const [loading, setLoading] = useState(true);

  const loadHistory = async () => {
    setLoading(true);
    try {
      const list = await historyStore.list(500);
      setHistoryBodies(list.map((e) => ({ body: e.body, ok: e.ok })));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void loadHistory();
  }, []);

  const recs: Recommendation[] = useMemo(() => {
    return generateRecommendations(historyBodies, {
      indexes,
      constraints,
      dismissed: new Set(dismissed),
    });
  }, [historyBodies, indexes, constraints, dismissed]);

  const apply = (rec: Recommendation) => {
    if (rec.kind === "RANGE_INDEX") {
      openNewIndex({
        kind: "RANGE",
        entity: rec.entity,
        label: rec.label,
        property: rec.property,
      });
    } else if (rec.kind === "TEXT_INDEX") {
      openNewIndex({
        kind: "TEXT",
        entity: rec.entity,
        label: rec.label,
        property: rec.property,
      });
    } else if (rec.kind === "UNIQUE_CONSTRAINT") {
      openNewConstraint({
        kind: "UNIQUE",
        entity: rec.entity,
        label: rec.label,
        property: rec.property,
      });
    } else {
      openNewConstraint({
        kind: "NOT_NULL",
        entity: rec.entity,
        label: rec.label,
        property: rec.property,
      });
    }
  };

  return (
    <Stack gap="xs">
      <Group justify="space-between" align="center">
        <Group gap={6}>
          <IconPrismLight size={14} color={tokens.accent.warning} />
          <Text size="sm" fw={600}>
            Recommendations
          </Text>
          {recs.length > 0 && (
            <Badge size="xs" variant="light">
              {recs.length}
            </Badge>
          )}
        </Group>
        <Group gap={4}>
          {dismissed.length > 0 && (
            <Button size="xs" variant="subtle" onClick={restore}>
              Restore dismissed ({dismissed.length})
            </Button>
          )}
          <Tooltip label="Re-scan history" withArrow>
            <ActionIcon
              size="sm"
              variant="subtle"
              onClick={() => void loadHistory()}
              aria-label="Re-scan recommendations"
            >
              <IconRefresh size={12} />
            </ActionIcon>
          </Tooltip>
        </Group>
      </Group>

      {loading ? (
        <Text size="xs" c={tokens.fg.subtle}>
          Scanning your query history…
        </Text>
      ) : recs.length === 0 ? (
        <Text size="xs" c={tokens.fg.subtle}>
          Nothing to suggest yet — keep running queries and we&apos;ll surface
          indexes and constraints worth adding.
        </Text>
      ) : (
        <Stack gap={6}>
          {recs.map((rec) => {
            const badge = KIND_BADGE[rec.kind];
            const isIndex =
              rec.kind === "RANGE_INDEX" || rec.kind === "TEXT_INDEX";
            return (
              <Paper withBorder key={rec.id} p="xs">
                <Group
                  gap={8}
                  wrap="nowrap"
                  justify="space-between"
                  align="flex-start"
                >
                  <Group
                    gap={8}
                    wrap="nowrap"
                    align="flex-start"
                    style={{ flex: 1, minWidth: 0 }}
                  >
                    {isIndex ? (
                      <IconBolt
                        size={14}
                        color={tokens.accent.primary}
                        style={{ marginTop: 2 }}
                      />
                    ) : (
                      <IconKey
                        size={14}
                        color={tokens.accent.warning}
                        style={{ marginTop: 2 }}
                      />
                    )}
                    <Stack gap={2} style={{ flex: 1, minWidth: 0 }}>
                      <Group gap={6} wrap="nowrap">
                        <Badge size="xs" variant="light" color={badge.color}>
                          {badge.label}
                        </Badge>
                        <Text size="sm" fw={500}>
                          {rec.label}.{rec.property}
                        </Text>
                      </Group>
                      <Text size="xs" c={tokens.fg.muted}>
                        {rec.reason}
                      </Text>
                    </Stack>
                  </Group>
                  <Group gap={4} wrap="nowrap">
                    <Button size="xs" onClick={() => apply(rec)}>
                      Add
                    </Button>
                    <Tooltip label="Dismiss" withArrow>
                      <ActionIcon
                        size="sm"
                        variant="subtle"
                        color="gray"
                        onClick={() => dismiss(rec.id)}
                        aria-label={`Dismiss recommendation ${rec.id}`}
                      >
                        <IconX size={12} />
                      </ActionIcon>
                    </Tooltip>
                  </Group>
                </Group>
              </Paper>
            );
          })}
        </Stack>
      )}
    </Stack>
  );
}
