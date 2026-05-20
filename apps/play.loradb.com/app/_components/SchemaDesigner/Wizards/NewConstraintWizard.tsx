"use client";

/**
 * Three-step wizard for `CREATE CONSTRAINT`.
 *
 * Step 1 — pick the *intent* (uniqueness, presence, composite key,
 * type predicate). Step 2 — bind the entity, label, properties, and
 * (for PROPERTY_TYPE) the target type. Step 3 — run a pre-flight
 * scan against the live graph and only enable "Create" once the scan
 * comes back clean.
 */

import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  Checkbox,
  Code,
  Group,
  MultiSelect,
  Paper,
  Radio,
  Select,
  Stack,
  Stepper,
  Table,
  Text,
  TextInput,
  Tooltip,
} from "@mantine/core";
import {
  IconAlertTriangle,
  IconArrowUpRight,
  IconCircleCheck,
  IconInfoCircle,
} from "@tabler/icons-react";

import { useStore } from "@/lib/state/store";
import {
  createConstraint,
  updateConstraint,
} from "@/lib/actions/schemaDesignActions";
import { openTabInCell } from "@/lib/actions/tabActions";
import {
  SCALAR_PROPERTY_TYPES,
  buildCreateConstraintDDL,
  constraintDefToDraft,
  suggestConstraintName,
} from "@/lib/schemaDesign/ddl";
import { buildConstraintUsageExamples } from "@/lib/schemaDesign/examples";
import {
  isSubmittable,
  validateConstraintDraft,
  type ValidationIssue,
} from "@/lib/schemaDesign/validate";
import {
  runPreflight,
  type PreflightVerdict,
} from "@/lib/schemaDesign/preflight";
import type {
  ConstraintDef,
  ConstraintDraft,
  ConstraintKind,
  EntityKind,
  IndexDef,
  ScalarPropertyType,
} from "@/lib/schemaDesign/types";

const EMPTY_INDEXES: IndexDef[] = [];
const EMPTY_CONSTRAINTS: ConstraintDef[] = [];
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

import { DDLPreview } from "../DDLPreview";
import { UsageExamples } from "../UsageExamples";

interface KindCard {
  kind: ConstraintKind;
  title: string;
  blurb: string;
  /** Restricts the entity selector when this kind is picked. */
  entity?: EntityKind;
}

const KIND_CARDS: readonly KindCard[] = [
  {
    kind: "UNIQUE",
    title: "No duplicates",
    blurb: "Reject two records sharing the same value (or tuple of values).",
  },
  {
    kind: "NOT_NULL",
    title: "Must be present",
    blurb: "Reject records that don't carry this property.",
  },
  {
    kind: "NODE_KEY",
    title: "Composite node identity",
    blurb:
      "A combination of properties uniquely identifies a node (e.g. country + tax id).",
    entity: "NODE",
  },
  {
    kind: "RELATIONSHIP_KEY",
    title: "Composite rel identity",
    blurb: "A combination of properties uniquely identifies a relationship.",
    entity: "RELATIONSHIP",
  },
  {
    kind: "PROPERTY_TYPE",
    title: "Restrict the value type",
    blurb: "Reject values that aren't of a chosen type (e.g. always STRING).",
  },
];

function defaultDraftFor(
  kind: ConstraintKind,
  entity: EntityKind,
): ConstraintDraft {
  return {
    kind,
    entity,
    label: "",
    properties: [],
    propertyType: "STRING",
    name: "",
    ifNotExists: true,
  };
}

export function NewConstraintWizard({ onClose }: { onClose: () => void }) {
  const { tokens } = usePlaygroundTheme();
  const schema = useStore((s) => s.schema);
  const indexes = useStore((s) => s.indexes ?? EMPTY_INDEXES);
  const constraints = useStore((s) => s.constraints ?? EMPTY_CONSTRAINTS);
  const seed = useStore((s) => s.newConstraintSeed);
  const editingDef = useStore((s) => s.editingConstraintDef);
  const editing = editingDef !== null;

  const [step, setStep] = useState(0);
  const [draft, setDraft] = useState<ConstraintDraft>(() =>
    editingDef ? constraintDefToDraft(editingDef) : defaultDraftFor("UNIQUE", "NODE"),
  );
  const [submitting, setSubmitting] = useState(false);
  const [nameTouched, setNameTouched] = useState(editing);
  const [scanning, setScanning] = useState(false);
  const [verdict, setVerdict] = useState<PreflightVerdict | null>(null);

  useEffect(() => {
    if (!seed) return;
    setDraft((d) => {
      const kind = seed.kind ?? d.kind;
      const card = KIND_CARDS.find((c) => c.kind === kind);
      const entity = card?.entity ?? seed.entity ?? d.entity;
      return {
        ...d,
        kind,
        entity,
        label: seed.label ?? d.label,
        properties: seed.property ? [seed.property] : d.properties,
      };
    });
  }, [seed]);

  useEffect(() => {
    if (nameTouched) return;
    setDraft((d) => ({ ...d, name: suggestConstraintName(d) }));
  }, [draft.kind, draft.label, draft.properties, nameTouched]);

  // Reset the preflight verdict whenever the relevant draft fields change.
  const propertiesKey = draft.properties.join(",");
  useEffect(() => {
    setVerdict(null);
  }, [
    draft.kind,
    draft.entity,
    draft.label,
    propertiesKey,
    draft.propertyType,
  ]);

  const labelOptions = useMemo(() => {
    if (draft.entity === "NODE") return schema?.labels ?? [];
    return schema?.relTypes ?? [];
  }, [schema, draft.entity]);

  const propertyOptions = useMemo(() => {
    if (draft.entity === "NODE") {
      return schema?.propertiesByLabel?.[draft.label] ?? [];
    }
    return schema?.propertiesByRelType?.[draft.label] ?? [];
  }, [schema, draft.entity, draft.label]);

  const issues = useMemo(
    () =>
      validateConstraintDraft(
        draft,
        { indexes, constraints },
        { selfName: editingDef?.name },
      ),
    [draft, indexes, constraints, editingDef],
  );
  const submittable = isSubmittable(issues) && verdict !== null && verdict.ok;

  const ddl = useMemo(() => {
    try {
      return buildCreateConstraintDDL(draft);
    } catch {
      return "// fill in the previous steps";
    }
  }, [draft]);

  const usageExamples = useMemo(
    () => buildConstraintUsageExamples(draft),
    [draft],
  );

  const runScan = async () => {
    setScanning(true);
    try {
      const result = await runPreflight(draft);
      setVerdict(result);
    } finally {
      setScanning(false);
    }
  };

  const submit = async () => {
    setSubmitting(true);
    const ok = editingDef
      ? await updateConstraint(editingDef.name, draft)
      : await createConstraint(draft);
    setSubmitting(false);
    if (ok) onClose();
  };

  return (
    <Stack gap="md">
      <Stack gap="md">
        <Stepper active={step} onStepClick={setStep} size="xs" iconSize={20}>
          <Stepper.Step label="Intent" description="What are you preventing?">
            <Stack gap="xs" mt="sm">
              {KIND_CARDS.map((card) => {
                const selected = draft.kind === card.kind;
                return (
                  <Paper
                    key={card.kind}
                    withBorder
                    p="xs"
                    onClick={() => {
                      // Re-clicking the active card shouldn't clear the
                      // entity / label / properties the user already
                      // picked.
                      if (draft.kind === card.kind) return;
                      const entity = card.entity ?? draft.entity;
                      setDraft({
                        ...defaultDraftFor(card.kind, entity),
                        name: draft.name,
                      });
                      setNameTouched(false);
                    }}
                    style={{
                      cursor: "pointer",
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
                        color="grape"
                      >
                        {card.kind.replace("_", " ")}
                      </Badge>
                    </Group>
                  </Paper>
                );
              })}
            </Stack>
          </Stepper.Step>

          <Stepper.Step label="Apply to" description="Pick the schema">
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
                  <Tooltip
                    label="NODE KEY only applies to nodes."
                    disabled={draft.kind !== "NODE_KEY"}
                    withArrow
                  >
                    <Radio
                      value="NODE"
                      label="Nodes"
                      disabled={draft.kind === "RELATIONSHIP_KEY"}
                    />
                  </Tooltip>
                  <Tooltip
                    label="RELATIONSHIP KEY only applies to relationships."
                    disabled={draft.kind !== "RELATIONSHIP_KEY"}
                    withArrow
                  >
                    <Radio
                      value="RELATIONSHIP"
                      label="Relationships"
                      disabled={draft.kind === "NODE_KEY"}
                    />
                  </Tooltip>
                </Group>
              </Radio.Group>

              <Select
                label={draft.entity === "NODE" ? "Label" : "Relationship type"}
                placeholder="Pick from your schema"
                data={labelOptions.map((l) => ({ value: l, label: l }))}
                value={draft.label || null}
                onChange={(v) =>
                  setDraft((d) => ({ ...d, label: v ?? "", properties: [] }))
                }
                searchable
                nothingFoundMessage="Run a query against the data first so we can see this."
              />
              <MultiSelect
                label="Properties"
                placeholder={
                  draft.kind === "NODE_KEY" || draft.kind === "RELATIONSHIP_KEY"
                    ? "Pick two or more — together they form the key"
                    : draft.kind === "UNIQUE"
                      ? "Pick one or more"
                      : "Pick one"
                }
                data={propertyOptions.map((p) => ({ value: p, label: p }))}
                value={draft.properties}
                onChange={(v) => setDraft((d) => ({ ...d, properties: v }))}
                searchable
                nothingFoundMessage="No properties seen for this label yet."
                disabled={!draft.label}
                maxValues={
                  draft.kind === "NOT_NULL" || draft.kind === "PROPERTY_TYPE"
                    ? 1
                    : undefined
                }
              />
              {draft.kind === "PROPERTY_TYPE" && (
                <Select
                  label="Required type"
                  data={SCALAR_PROPERTY_TYPES.map((t) => ({
                    value: t,
                    label: t,
                  }))}
                  value={draft.propertyType}
                  onChange={(v) =>
                    setDraft((d) => ({
                      ...d,
                      propertyType: (v as ScalarPropertyType) ?? "STRING",
                    }))
                  }
                  allowDeselect={false}
                />
              )}
            </Stack>
          </Stepper.Step>

          <Stepper.Step label="Pre-flight" description="Scan & confirm">
            <Stack gap="sm" mt="sm">
              <Text size="sm" c={tokens.fg.muted}>
                Run a non-destructive scan to make sure the existing data
                already satisfies the constraint.
              </Text>
              <Group gap="xs">
                <Button
                  size="xs"
                  variant="default"
                  onClick={() => void runScan()}
                  loading={scanning}
                  disabled={!isSubmittable(issues)}
                >
                  Run pre-flight scan
                </Button>
                {verdict && (verdict.offending > 0 || verdict.capped) && (
                  <Button
                    size="xs"
                    variant="subtle"
                    leftSection={<IconArrowUpRight size={12} />}
                    onClick={() => {
                      openTabInCell({
                        name: "Pre-flight findings",
                        body: verdict.jumpQuery,
                      });
                      onClose();
                    }}
                  >
                    {verdict.capped ? "Run jump query" : "View offending rows"}
                  </Button>
                )}
              </Group>
              {verdict && (
                <Alert
                  color={
                    verdict.ok ? "green" : verdict.capped ? "yellow" : "red"
                  }
                  variant="light"
                  icon={
                    verdict.ok ? (
                      <IconCircleCheck size={14} />
                    ) : (
                      <IconAlertTriangle size={14} />
                    )
                  }
                >
                  {verdict.message}
                </Alert>
              )}
              {verdict && verdict.sample.length > 0 && (
                <Paper withBorder p="xs">
                  <Text
                    size="xs"
                    c={tokens.fg.muted}
                    tt="uppercase"
                    fw={600}
                    mb={4}
                  >
                    Sample offending rows
                  </Text>
                  <Table
                    withColumnBorders
                    withTableBorder
                    striped
                    highlightOnHover
                    fz="xs"
                  >
                    <Table.Thead>
                      <Table.Tr>
                        {Object.keys(verdict.sample[0]!).map((k) => (
                          <Table.Th key={k}>{k}</Table.Th>
                        ))}
                      </Table.Tr>
                    </Table.Thead>
                    <Table.Tbody>
                      {verdict.sample.map((row, i) => (
                        <Table.Tr key={i}>
                          {Object.keys(row).map((k) => (
                            <Table.Td key={k}>
                              <Code style={{ fontSize: 11 }}>
                                {formatCell(row[k])}
                              </Code>
                            </Table.Td>
                          ))}
                        </Table.Tr>
                      ))}
                    </Table.Tbody>
                  </Table>
                </Paper>
              )}
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
                label="Use IF NOT EXISTS"
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
      </Stack>
      {step === 2 && (
        <Stack gap="xs">
          <DDLPreview ddl={ddl} />
          <Paper withBorder p="xs">
            <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
              What this does
            </Text>
            <Text size="xs" c={tokens.fg.primary} mt={4}>
              {whatItDoes(draft.kind)}
            </Text>
          </Paper>
          <UsageExamples
            caption="Example writes affected"
            examples={usageExamples}
          />
          <IssueList issues={issues} />
          {editing && (
            <Alert
              color="yellow"
              variant="light"
              icon={<IconAlertTriangle size={14} />}
            >
              Editing replaces the constraint — “{editingDef!.name}” will be
              dropped and recreated. There is a brief window during the swap
              where the protection is not enforced.
            </Alert>
          )}
          {verdict === null ? (
            <Alert
              color="blue"
              variant="light"
              icon={<IconInfoCircle size={14} />}
            >
              Run the pre-flight scan before {editing ? "saving" : "creating"}{" "}
              the constraint.
            </Alert>
          ) : submittable ? (
            <Alert
              color="green"
              variant="light"
              icon={<IconCircleCheck size={14} />}
            >
              {editing ? "Ready to save." : "Ready to create."}
            </Alert>
          ) : null}
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
              {editing ? "Save changes" : "Create constraint"}
            </Button>
          )}
        </Group>
      </Group>
    </Stack>
  );
}

function whatItDoes(kind: ConstraintKind): string {
  switch (kind) {
    case "UNIQUE":
      return "Future writes are rejected when they would create a duplicate value (or tuple).";
    case "NOT_NULL":
      return "Future writes are rejected when they would leave this property empty.";
    case "NODE_KEY":
    case "RELATIONSHIP_KEY":
      return "The chosen properties together act as a primary key — uniqueness and presence both enforced.";
    case "PROPERTY_TYPE":
      return "Future writes are rejected when this property's value doesn't match the required type.";
  }
}

function formatCell(value: unknown): string {
  if (value === null) return "null";
  if (value === undefined) return "—";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function IssueList({ issues }: { issues: ValidationIssue[] }) {
  if (issues.length === 0) return null;
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
