"use client";

/**
 * Three-step wizard for `CREATE … INDEX`. Each step is its own pane;
 * the DDL preview stays sticky on the right so users see the Cypher
 * change live as they make picks.
 *
 * RANGE / TEXT / POINT / LOOKUP are supported. VECTOR + FULLTEXT are
 * surfaced as disabled options with a "coming soon" hint so the user
 * knows the feature is on the roadmap.
 */

import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  Checkbox,
  Group,
  MultiSelect,
  Paper,
  Radio,
  Select,
  Stack,
  Stepper,
  Text,
  TextInput,
  Tooltip,
} from "@mantine/core";
import { IconCircleCheck, IconInfoCircle } from "@tabler/icons-react";

import { useStore } from "@/lib/state/store";
import { createIndex, updateIndex } from "@/lib/actions/schemaDesignActions";
import {
  buildCreateIndexDDL,
  indexDefToDraft,
  suggestIndexName,
} from "@/lib/schemaDesign/ddl";
import { buildIndexUsageExamples } from "@/lib/schemaDesign/examples";
import {
  isSubmittable,
  validateIndexDraft,
  type ValidationIssue,
} from "@/lib/schemaDesign/validate";
import type {
  ConstraintDef,
  EntityKind,
  IndexDef,
  IndexDraft,
  IndexKind,
} from "@/lib/schemaDesign/types";

const EMPTY_INDEXES: IndexDef[] = [];
const EMPTY_CONSTRAINTS: ConstraintDef[] = [];
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { DDLPreview } from "../DDLPreview";
import { UsageExamples } from "../UsageExamples";

interface KindCard {
  kind: IndexKind;
  title: string;
  blurb: string;
  disabled?: boolean;
  disabledReason?: string;
}

const KIND_CARDS: readonly KindCard[] = [
  {
    kind: "RANGE",
    title: "Look up by value",
    blurb:
      "Equality and range comparisons on a property — the everyday workhorse.",
  },
  {
    kind: "TEXT",
    title: "Search inside text",
    blurb: "STARTS WITH / CONTAINS / ENDS WITH on string properties.",
  },
  {
    kind: "POINT",
    title: "Spatial lookup",
    blurb: "Distance-based queries on POINT properties.",
  },
  {
    kind: "LOOKUP",
    title: "All nodes or rels of a type",
    blurb:
      "Token index over labels / relationship types. Usually one per entity is enough.",
  },
  {
    kind: "VECTOR",
    title: "Vector similarity",
    blurb: "Nearest-neighbour over vector embeddings.",
    disabled: true,
    disabledReason: "Coming soon — planner integration still in progress.",
  },
  {
    kind: "FULLTEXT",
    title: "Full-text search",
    blurb: "Multi-property text search across labels.",
  },
];

function defaultDraftFor(kind: IndexKind): IndexDraft {
  return {
    kind,
    entity: "NODE",
    label: "",
    properties: [],
    name: "",
    ifNotExists: true,
  };
}

export function NewIndexWizard({ onClose }: { onClose: () => void }) {
  const { tokens } = usePlaygroundTheme();
  const schema = useStore((s) => s.schema);
  const indexes = useStore((s) => s.indexes ?? EMPTY_INDEXES);
  const constraints = useStore((s) => s.constraints ?? EMPTY_CONSTRAINTS);
  const seed = useStore((s) => s.newIndexSeed);
  const editingDef = useStore((s) => s.editingIndexDef);
  const editing = editingDef !== null;

  const [step, setStep] = useState(0);
  const [draft, setDraft] = useState<IndexDraft>(() =>
    editingDef ? indexDefToDraft(editingDef) : defaultDraftFor("RANGE"),
  );
  const [submitting, setSubmitting] = useState(false);
  // When editing, treat the pre-loaded name as user-owned so it isn't
  // overwritten by the auto-suggester on the first prop tweak.
  const [nameTouched, setNameTouched] = useState(editing);

  // Apply the seed once.
  useEffect(() => {
    if (!seed) return;
    setDraft((d) => ({
      ...d,
      kind: seed.kind ?? d.kind,
      entity: seed.entity ?? d.entity,
      label: seed.label ?? d.label,
      properties: seed.property ? [seed.property] : d.properties,
    }));
    if (seed.label) setStep(1);
  }, [seed]);

  // Keep the suggested name in sync until the user manually edits it.
  useEffect(() => {
    if (nameTouched) return;
    setDraft((d) => ({ ...d, name: suggestIndexName(d) }));
  }, [draft.kind, draft.entity, draft.label, draft.properties, nameTouched]);

  const labelOptions = useMemo(() => {
    if (draft.entity === "NODE") return schema?.labels ?? [];
    return schema?.relTypes ?? [];
  }, [schema, draft.entity]);

  const propertyOptions = useMemo(() => {
    if (draft.kind === "LOOKUP") return [];
    if (draft.entity === "NODE") {
      return schema?.propertiesByLabel?.[draft.label] ?? [];
    }
    return schema?.propertiesByRelType?.[draft.label] ?? [];
  }, [schema, draft.entity, draft.kind, draft.label]);

  const issues = useMemo(
    () =>
      validateIndexDraft(
        draft,
        { indexes, constraints },
        { selfName: editingDef?.name },
      ),
    [draft, indexes, constraints, editingDef],
  );
  const submittable = isSubmittable(issues);

  const ddl = useMemo(() => {
    try {
      return buildCreateIndexDDL(draft);
    } catch {
      return "// fill in the previous steps";
    }
  }, [draft]);

  const usageExamples = useMemo(
    () =>
      buildIndexUsageExamples(draft, {
        sampleLabel: schema?.labels?.[0],
        sampleRelType: schema?.relTypes?.[0],
      }),
    [draft, schema],
  );

  const submit = async () => {
    setSubmitting(true);
    const ok = editingDef
      ? await updateIndex(editingDef.name, draft)
      : await createIndex(draft);
    setSubmitting(false);
    if (ok) onClose();
  };

  return (
    <Stack gap="md">
      <Stack gap="md">
        <Stepper active={step} onStepClick={setStep} size="xs" iconSize={20}>
          <Stepper.Step label="Kind" description="What do you need?">
            <Stack gap="xs" mt="sm">
              {KIND_CARDS.map((card) => {
                const selected = draft.kind === card.kind && !card.disabled;
                return (
                  <Tooltip
                    key={card.kind}
                    label={card.disabledReason ?? ""}
                    disabled={!card.disabled}
                    withArrow
                  >
                    <Paper
                      withBorder
                      p="xs"
                      onClick={() => {
                        if (card.disabled) return;
                        // Re-clicking the active card shouldn't clear the
                        // entity / label / properties the user already
                        // picked.
                        if (draft.kind === card.kind) return;
                        setDraft({
                          ...defaultDraftFor(card.kind),
                          name: draft.name,
                        });
                        setNameTouched(false);
                      }}
                      style={{
                        cursor: card.disabled ? "not-allowed" : "pointer",
                        opacity: card.disabled ? 0.55 : 1,
                        borderColor: selected
                          ? tokens.accent.primary
                          : tokens.border.subtle,
                        background: selected
                          ? tokens.bg.overlay
                          : tokens.bg.panel,
                      }}
                    >
                      <Group gap={8} wrap="nowrap">
                        <Stack gap={2} style={{ flex: 1, minWidth: 0 }}>
                          <Text size="sm" fw={600}>
                            {card.title}
                          </Text>
                          <Text size="xs" c={tokens.fg.muted}>
                            {card.blurb}
                          </Text>
                        </Stack>
                        {selected && (
                          <IconCircleCheck
                            size={16}
                            color={tokens.accent.primary}
                          />
                        )}
                        <Badge
                          size="sm"
                          variant={selected ? "filled" : "light"}
                        >
                          {card.kind}
                        </Badge>
                      </Group>
                    </Paper>
                  </Tooltip>
                );
              })}
            </Stack>
          </Stepper.Step>

          <Stepper.Step label="Where" description="Apply to…">
            <Stack gap="sm" mt="sm">
              <Radio.Group
                label="Entity"
                value={draft.entity}
                onChange={(v) =>
                  setDraft((d) => ({
                    ...d,
                    entity: v as EntityKind,
                    label: "",
                    properties: [],
                  }))
                }
              >
                <Group gap="md" mt={4}>
                  <Radio value="NODE" label="Nodes" />
                  <Radio value="RELATIONSHIP" label="Relationships" />
                </Group>
              </Radio.Group>

              {draft.kind !== "LOOKUP" && (
                <>
                  <Select
                    label={
                      draft.entity === "NODE" ? "Label" : "Relationship type"
                    }
                    placeholder="Pick from your schema"
                    data={labelOptions.map((l) => ({ value: l, label: l }))}
                    value={draft.label || null}
                    onChange={(v) =>
                      setDraft((d) => ({
                        ...d,
                        label: v ?? "",
                        properties: [],
                      }))
                    }
                    searchable
                    nothingFoundMessage="Run a query against the data first so we can see this label."
                  />
                  <MultiSelect
                    label="Properties"
                    placeholder="Pick one or more"
                    data={propertyOptions.map((p) => ({ value: p, label: p }))}
                    value={draft.properties}
                    onChange={(v) => setDraft((d) => ({ ...d, properties: v }))}
                    searchable
                    nothingFoundMessage="No properties seen for this label yet."
                    disabled={!draft.label}
                  />
                </>
              )}

              {draft.kind === "LOOKUP" && (
                <Alert
                  icon={<IconInfoCircle size={14} />}
                  color="blue"
                  variant="light"
                >
                  Lookup indexes apply to all{" "}
                  {draft.entity === "NODE" ? "labels" : "relationship types"} —
                  no label or property selection needed.
                </Alert>
              )}
            </Stack>
          </Stepper.Step>

          <Stepper.Step label="Confirm" description="Name & review">
            <Stack gap="sm" mt="sm">
              <TextInput
                label="Name"
                value={draft.name}
                onChange={(e) => {
                  const value = e.currentTarget.value;
                  setNameTouched(true);
                  setDraft((d) => ({ ...d, name: value }));
                }}
                description={
                  nameTouched
                    ? "You're editing the suggested name."
                    : "Auto-generated from your picks — edit if you want."
                }
              />
              <Checkbox
                label="Use IF NOT EXISTS (recommended for beginners)"
                checked={draft.ifNotExists}
                onChange={(e) => {
                  // Read the checked state synchronously — `currentTarget`
                  // is nulled out once React finishes dispatching the
                  // event, so accessing it inside the setState updater
                  // throws when batching defers the closure.
                  const checked = e.currentTarget.checked;
                  setDraft((d) => ({ ...d, ifNotExists: checked }));
                }}
              />
            </Stack>
          </Stepper.Step>
        </Stepper>
        {step === 2 && (
          <Stack gap="xs">
            <DDLPreview ddl={ddl} />
            <Paper withBorder p="xs">
              <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
                What this does
              </Text>
              <Text size="xs" c={tokens.fg.primary} mt={4}>
                {draft.kind === "RANGE"
                  ? "Speeds up filters with =, <, >, BETWEEN and ORDER BY on the chosen properties."
                  : draft.kind === "TEXT"
                    ? "Speeds up STARTS WITH, CONTAINS and ENDS WITH on the chosen string properties."
                    : draft.kind === "POINT"
                      ? "Speeds up point.distance and bounded-area lookups on POINT values."
                      : draft.kind === "FULLTEXT"
                        ? "Speeds up CALL db.index.fulltext.queryNodes / queryRelationships across the chosen properties."
                        : "Speeds up MATCH (n:Label) / MATCH ()-[r:Type]-() scans."}
              </Text>
            </Paper>
            <UsageExamples examples={usageExamples} />
            {editing && (
              <Alert
                color="yellow"
                variant="light"
                icon={<IconInfoCircle size={14} />}
              >
                Editing replaces the index — “{editingDef!.name}” will be
                dropped and recreated. Queries that rely on it will fall back
                to a full scan until the new index is online.
              </Alert>
            )}
            <IssueList issues={issues} editing={editing} />
          </Stack>
        )}
        <Group justify="space-between" mt="md">
          <Button variant="default" size="xs" onClick={onClose}>
            Cancel
          </Button>
          <Group gap="xs">
            {step > 0 && (
              <Button
                variant="default"
                size="xs"
                onClick={() => setStep((s) => Math.max(0, s - 1))}
              >
                Back
              </Button>
            )}
            {step < 2 ? (
              <Button
                size="xs"
                onClick={() => setStep((s) => Math.min(2, s + 1))}
              >
                Next
              </Button>
            ) : (
              <Button
                size="xs"
                color="blue"
                loading={submitting}
                disabled={!submittable}
                onClick={() => void submit()}
              >
                {editing ? "Save changes" : "Create index"}
              </Button>
            )}
          </Group>
        </Group>
      </Stack>
    </Stack>
  );
}

function IssueList({
  issues,
  editing = false,
}: {
  issues: ValidationIssue[];
  editing?: boolean;
}) {
  if (issues.length === 0) {
    return (
      <Alert color="green" variant="light" icon={<IconCircleCheck size={14} />}>
        {editing ? "Ready to save." : "Ready to create."}
      </Alert>
    );
  }
  return (
    <Stack gap={4}>
      {issues.map((issue, i) => (
        <Alert
          key={`${issue.field}-${i}`}
          color={issue.blocking ? "red" : "yellow"}
          variant="light"
        >
          {issue.message}
        </Alert>
      ))}
    </Stack>
  );
}
