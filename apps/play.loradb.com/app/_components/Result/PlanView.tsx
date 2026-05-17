"use client";

/**
 * Surfaces the parser's view of the active editor tab — semantic
 * diagnostics from `analyse()` plus the binding outline from
 * `outline()` (variables, labels, rel-types, parameters).
 *
 * Re-runs whenever the body changes; debounced to ~200ms so each
 * keystroke does not kick off a fresh WASM call. Tolerates parser
 * errors — both calls are wrapped in try/catch and the pane
 * gracefully degrades to an empty state.
 */

import { useEffect, useMemo, useState } from "react";
import {
  Badge,
  Box,
  Center,
  Code,
  Group,
  ScrollArea,
  Stack,
  Text,
  ThemeIcon,
} from "@mantine/core";
import {
  IconAlertCircle,
  IconAlertTriangle,
  IconBox,
  IconHash,
  IconInfoCircle,
  IconTag,
} from "@tabler/icons-react";

import { analyse, outline as outlineFn } from "@loradb/lora-query";
import type {
  Analysis,
  Outline,
  OutlineVariable,
  ParseError,
  VariableKind,
} from "@loradb/lora-query";

import { useActiveTab } from "@/lib/state/selectors";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

const DEBOUNCE_MS = 200;

interface AnalysisBundle {
  analysis: Analysis | null;
  outline: Outline | null;
  loaded: boolean;
}

const EMPTY_BUNDLE: AnalysisBundle = {
  analysis: null,
  outline: null,
  loaded: false,
};

function severityIcon(severity: ParseError["severity"], tokens: ReturnType<typeof usePlaygroundTheme>["tokens"]) {
  if (severity === "error") {
    return (
      <ThemeIcon size="sm" variant="light" color="red">
        <IconAlertCircle size={14} />
      </ThemeIcon>
    );
  }
  if (severity === "warning") {
    return (
      <ThemeIcon size="sm" variant="light" color="yellow">
        <IconAlertTriangle size={14} />
      </ThemeIcon>
    );
  }
  void tokens;
  return (
    <ThemeIcon size="sm" variant="light" color="blue">
      <IconInfoCircle size={14} />
    </ThemeIcon>
  );
}

function VariableRow({ v }: { v: OutlineVariable }) {
  const { tokens } = usePlaygroundTheme();
  const labelStr = v.label ? `:${v.label}` : "";
  return (
    <Group gap="xs" wrap="nowrap" align="center">
      <ThemeIcon size="sm" variant="light" color="grape">
        <IconHash size={14} />
      </ThemeIcon>
      <Code style={{ background: tokens.bg.panel, color: tokens.fg.primary }}>{v.name}</Code>
      {labelStr && (
        <Text size="xs" c={tokens.fg.muted} ff={tokens.font.mono}>
          {labelStr}
        </Text>
      )}
      <Badge size="xs" variant="default" color="gray">
        {v.kind}
      </Badge>
      {v.aliasOf && (
        <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
          alias of {v.aliasOf}
        </Text>
      )}
    </Group>
  );
}

function groupByKind(vars: readonly OutlineVariable[]): Record<VariableKind, OutlineVariable[]> {
  const out: Record<VariableKind, OutlineVariable[]> = {
    node: [],
    relationship: [],
    scalar: [],
    pattern: [],
  };
  for (const v of vars) {
    out[v.kind].push(v);
  }
  return out;
}

export function PlanView() {
  const { tokens } = usePlaygroundTheme();
  const tab = useActiveTab();
  const body = tab?.body ?? "";
  const [bundle, setBundle] = useState<AnalysisBundle>(EMPTY_BUNDLE);

  useEffect(() => {
    let cancelled = false;
    if (body.trim().length === 0) {
      setBundle(EMPTY_BUNDLE);
      return () => {
        cancelled = true;
      };
    }
    const timer = setTimeout(() => {
      (async () => {
        let analysis: Analysis | null = null;
        let outline: Outline | null = null;
        try {
          analysis = await analyse(body);
        } catch {
          analysis = null;
        }
        try {
          outline = await outlineFn(body);
        } catch {
          outline = null;
        }
        if (cancelled) return;
        setBundle({ analysis, outline, loaded: true });
      })().catch(() => {
        if (cancelled) return;
        setBundle({ analysis: null, outline: null, loaded: true });
      });
    }, DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [body]);

  const grouped = useMemo(() => {
    if (!bundle.outline) return null;
    return groupByKind(bundle.outline.variables);
  }, [bundle.outline]);

  if (body.trim().length === 0) {
    return (
      <Center h="100%" style={{ background: tokens.bg.editor }}>
        <Text size="xs" c={tokens.fg.subtle}>
          Empty query — nothing to plan.
        </Text>
      </Center>
    );
  }

  const diagnostics = bundle.analysis?.diagnostics ?? [];
  const variables = bundle.outline?.variables ?? [];
  const labels = bundle.outline?.labels ?? [];
  const relTypes = bundle.outline?.relTypes ?? [];
  const parameters = bundle.outline?.parameters ?? [];

  return (
    <ScrollArea
      style={{
        flex: 1,
        minHeight: 0,
        background: tokens.bg.editor,
      }}
      type="auto"
    >
      <Box p="md">
        <Stack gap="md">
          <Group gap="xs">
            <Badge variant="default" color="gray" size="sm">
              {variables.length} variable{variables.length === 1 ? "" : "s"}
            </Badge>
            <Badge variant="default" color="gray" size="sm">
              {labels.length} label{labels.length === 1 ? "" : "s"}
            </Badge>
            <Badge variant="default" color="gray" size="sm">
              {relTypes.length} rel-type{relTypes.length === 1 ? "" : "s"}
            </Badge>
            <Badge variant="default" color="gray" size="sm">
              {parameters.length} param{parameters.length === 1 ? "" : "s"}
            </Badge>
            <Badge variant="default" color="gray" size="sm">
              {diagnostics.length} diagnostic{diagnostics.length === 1 ? "" : "s"}
            </Badge>
          </Group>

          {diagnostics.length > 0 && (
            <Stack gap="xs">
              <Text size="sm" fw={600} c={tokens.fg.primary}>
                Diagnostics
              </Text>
              {diagnostics.map((d, i) => (
                <Group key={`d-${i}`} align="flex-start" gap="xs" wrap="nowrap">
                  {severityIcon(d.severity, tokens)}
                  <Stack gap={2} style={{ flex: 1, minWidth: 0 }}>
                    <Text size="sm" c={tokens.fg.primary}>
                      {d.message}
                    </Text>
                    <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
                      {d.line > 0 ? `${d.line}:${d.column}` : "—"}
                    </Text>
                  </Stack>
                </Group>
              ))}
            </Stack>
          )}

          {labels.length > 0 && (
            <Stack gap="xs">
              <Text size="sm" fw={600} c={tokens.fg.primary}>
                Labels
              </Text>
              <Group gap={6} wrap="wrap">
                {labels.map((l) => (
                  <Badge key={l} variant="light" color="blue" size="sm">
                    {l}
                  </Badge>
                ))}
              </Group>
            </Stack>
          )}

          {relTypes.length > 0 && (
            <Stack gap="xs">
              <Text size="sm" fw={600} c={tokens.fg.primary}>
                Relationship types
              </Text>
              <Group gap={6} wrap="wrap">
                {relTypes.map((r) => (
                  <Badge key={r} variant="light" color="orange" size="sm">
                    :{r}
                  </Badge>
                ))}
              </Group>
            </Stack>
          )}

          {parameters.length > 0 && (
            <Stack gap="xs">
              <Text size="sm" fw={600} c={tokens.fg.primary}>
                Parameters
              </Text>
              <Group gap={6} wrap="wrap">
                {parameters.map((p) => (
                  <Group key={p} gap={4}>
                    <ThemeIcon size="sm" variant="light" color="teal">
                      <IconTag size={14} />
                    </ThemeIcon>
                    <Code style={{ background: tokens.bg.panel }}>${p}</Code>
                  </Group>
                ))}
              </Group>
            </Stack>
          )}

          {variables.length > 0 && grouped && (
            <Stack gap="xs">
              <Text size="sm" fw={600} c={tokens.fg.primary}>
                Outline
              </Text>
              {(Object.keys(grouped) as VariableKind[]).map((kind) => {
                const list = grouped[kind];
                if (list.length === 0) return null;
                return (
                  <Stack key={kind} gap={4}>
                    <Group gap="xs">
                      <ThemeIcon size="sm" variant="light" color="gray">
                        <IconBox size={14} />
                      </ThemeIcon>
                      <Text size="xs" c={tokens.fg.muted} tt="uppercase">
                        {kind}
                      </Text>
                    </Group>
                    <Stack gap={2} pl="md">
                      {list.map((v) => (
                        <VariableRow key={`${v.name}-${v.declStart}`} v={v} />
                      ))}
                    </Stack>
                  </Stack>
                );
              })}
            </Stack>
          )}

          {bundle.loaded &&
            diagnostics.length === 0 &&
            variables.length === 0 &&
            labels.length === 0 &&
            relTypes.length === 0 &&
            parameters.length === 0 && (
              <Text size="xs" c={tokens.fg.subtle}>
                No structural information.
              </Text>
            )}
        </Stack>
      </Box>
    </ScrollArea>
  );
}
