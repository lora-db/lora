"use client";

/**
 * Multi-step wizard for `CREATE … INDEX`. Each step is its own pane;
 * the DDL preview stays sticky on the right so users see the Cypher
 * change live as they make picks.
 *
 * Steps:
 *   1. Kind            — pick the index kind
 *   2. Where           — pick entity, label, properties
 *   3. Tune (VECTOR)   — vector-specific: dim / similarity / provider
 *                        / hnsw knobs / quantization. Only present
 *                        when kind === VECTOR.
 *   4. Confirm         — name, IF NOT EXISTS, DDL preview, examples
 *
 * RANGE / TEXT / POINT / LOOKUP / FULLTEXT / VECTOR are all supported.
 */

import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  Checkbox,
  Divider,
  Group,
  MultiSelect,
  NumberInput,
  Paper,
  Radio,
  SegmentedControl,
  Select,
  Slider,
  Stack,
  Stepper,
  Switch,
  Text,
  TextInput,
  Tooltip,
} from "@mantine/core";
import {
  IconBolt,
  IconCircleCheck,
  IconInfoCircle,
  IconRulerMeasure,
  IconWand,
} from "@tabler/icons-react";

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
  VectorIndexOptions,
  VectorIndexProvider,
  VectorQuantization,
  VectorSimilarity,
} from "@/lib/schemaDesign/types";
import { DEFAULT_VECTOR_OPTIONS } from "@/lib/schemaDesign/types";

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
    blurb:
      "k-NN over embeddings. HNSW for fast approximate search, flat for exact small-N. Picks up your default tuning unless you tweak it.",
  },
  {
    kind: "FULLTEXT",
    title: "Full-text search",
    blurb: "Multi-property text search across labels.",
  },
];

function defaultDraftFor(kind: IndexKind): IndexDraft {
  const draft: IndexDraft = {
    kind,
    entity: "NODE",
    label: "",
    properties: [],
    name: "",
    ifNotExists: true,
  };
  if (kind === "VECTOR") {
    draft.vectorOptions = { ...DEFAULT_VECTOR_OPTIONS };
  }
  return draft;
}

const SIMILARITY_OPTIONS: ReadonlyArray<{
  value: VectorSimilarity;
  label: string;
  hint: string;
}> = [
  {
    value: "cosine",
    label: "Cosine",
    hint: "Direction-only — the right pick for most embedding workflows.",
  },
  {
    value: "euclidean",
    label: "Euclidean",
    hint: "L2 distance. Good when magnitudes carry meaning.",
  },
  {
    value: "dot",
    label: "Dot",
    hint: "Raw inner product. Equivalent to cosine on normalised vectors and one reciprocal-sqrt cheaper.",
  },
  {
    value: "manhattan",
    label: "Manhattan",
    hint: "L1 distance. Useful with quantised / binary-ish features.",
  },
];

const PROVIDER_OPTIONS: ReadonlyArray<{
  value: VectorIndexProvider;
  label: string;
  hint: string;
}> = [
  {
    value: "hnsw",
    label: "HNSW",
    hint: "Approximate k-NN with sub-linear queries. The default at scale.",
  },
  {
    value: "flat",
    label: "Flat",
    hint: "Brute-force scoring. Exact, recommended for <10k vectors or as an oracle.",
  },
];

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

  const isVector = draft.kind === "VECTOR";
  const lastStep = isVector ? 3 : 2;
  // Clamp step when the kind switches between vector / non-vector
  // mid-flow so the user never lands on a step that no longer
  // renders.
  useEffect(() => {
    if (step > lastStep) setStep(lastStep);
  }, [lastStep, step]);

  const updateVectorOptions = (patch: Partial<VectorIndexOptions>) => {
    setDraft((d) => ({
      ...d,
      vectorOptions: {
        ...(d.vectorOptions ?? DEFAULT_VECTOR_OPTIONS),
        ...patch,
      },
    }));
  };

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

          {isVector && (
            <Stepper.Step
              label="Tune"
              description="Vector knobs"
              icon={<IconRulerMeasure size={16} />}
            >
              <VectorTuneStep
                options={draft.vectorOptions ?? DEFAULT_VECTOR_OPTIONS}
                onChange={updateVectorOptions}
              />
            </Stepper.Step>
          )}

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
        {step === lastStep && (
          <Stack gap="xs">
            <DDLPreview ddl={ddl} />
            <Paper withBorder p="xs">
              <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
                What this does
              </Text>
              <Text size="xs" c={tokens.fg.primary} mt={4}>
                {whatThisDoesCopy(draft)}
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
            {step < lastStep ? (
              <Button
                size="xs"
                onClick={() => setStep((s) => Math.min(lastStep, s + 1))}
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

function whatThisDoesCopy(draft: IndexDraft): string {
  switch (draft.kind) {
    case "RANGE":
      return "Speeds up filters with =, <, >, BETWEEN and ORDER BY on the chosen properties.";
    case "TEXT":
      return "Speeds up STARTS WITH, CONTAINS and ENDS WITH on the chosen string properties.";
    case "POINT":
      return "Speeds up point.distance and bounded-area lookups on POINT values.";
    case "FULLTEXT":
      return "Speeds up CALL db.index.fulltext.queryNodes / queryRelationships across the chosen properties.";
    case "VECTOR":
      return draft.vectorOptions?.provider === "hnsw"
        ? "Builds an HNSW graph for sub-linear k-NN over the embedding property. Tune knobs control recall vs query cost."
        : "Scans every vector for every query. Exact results; consider HNSW once you cross ~10k vectors.";
    default:
      return "Speeds up MATCH (n:Label) / MATCH ()-[r:Type]-() scans.";
  }
}

/**
 * Vector-index tuning panel. Ships sensible defaults so a user can
 * click straight to Confirm — every knob has an explainer so the
 * tradeoff is visible before they touch it.
 */
function VectorTuneStep({
  options,
  onChange,
}: {
  options: VectorIndexOptions;
  onChange: (patch: Partial<VectorIndexOptions>) => void;
}) {
  const { tokens } = usePlaygroundTheme();
  const isHnsw = options.provider === "hnsw";
  const int8Allowed = options.similarity === "cosine" && isHnsw;
  const selectedSim = SIMILARITY_OPTIONS.find(
    (o) => o.value === options.similarity,
  );
  const selectedProvider = PROVIDER_OPTIONS.find(
    (o) => o.value === options.provider,
  );

  return (
    <Stack gap="sm" mt="sm">
      <Alert
        color="grape"
        variant="light"
        icon={<IconWand size={14} />}
        styles={{ message: { fontSize: 12 } }}
      >
        Defaults work for most embedding workloads. Tune dimensions to
        match your model (e.g. 384 for MiniLM, 768 for BERT-base,
        1536 for OpenAI text-embedding-3-small).
      </Alert>

      <NumberInput
        label="Embedding dimensions"
        description="Width of every stored vector. Must match the vectors you insert."
        value={options.dimensions}
        onChange={(v) => {
          const next =
            typeof v === "number" && Number.isFinite(v) ? Math.trunc(v) : 0;
          onChange({ dimensions: next });
        }}
        min={1}
        max={4096}
        step={1}
        clampBehavior="strict"
        leftSection={<IconRulerMeasure size={14} />}
      />

      <Stack gap={4}>
        <Text size="sm" fw={500}>
          Similarity function
        </Text>
        <SegmentedControl
          fullWidth
          size="xs"
          value={options.similarity}
          onChange={(v) =>
            onChange({ similarity: v as VectorSimilarity })
          }
          data={SIMILARITY_OPTIONS.map((o) => ({
            value: o.value,
            label: o.label,
          }))}
        />
        {selectedSim && (
          <Text size="xs" c={tokens.fg.muted}>
            {selectedSim.hint}
          </Text>
        )}
      </Stack>

      <Stack gap={4}>
        <Text size="sm" fw={500}>
          Index provider
        </Text>
        <SegmentedControl
          fullWidth
          size="xs"
          value={options.provider}
          onChange={(v) =>
            onChange({ provider: v as VectorIndexProvider })
          }
          data={PROVIDER_OPTIONS.map((o) => ({
            value: o.value,
            label: o.label,
          }))}
        />
        {selectedProvider && (
          <Text size="xs" c={tokens.fg.muted}>
            {selectedProvider.hint}
          </Text>
        )}
      </Stack>

      {isHnsw && (
        <>
          <Divider
            my={4}
            label={
              <Group gap={4}>
                <IconBolt size={12} />
                <Text size="xs" fw={600} tt="uppercase">
                  HNSW tuning
                </Text>
              </Group>
            }
            labelPosition="left"
          />

          <SliderKnob
            label="M (neighbours per layer)"
            description="Higher = better recall, more memory, slower build. 16 is a strong default."
            value={options.hnswM}
            onChange={(v) => onChange({ hnswM: v })}
            min={4}
            max={64}
            step={2}
            marks={[
              { value: 8, label: "8" },
              { value: 16, label: "16" },
              { value: 32, label: "32" },
              { value: 64, label: "64" },
            ]}
          />

          <SliderKnob
            label="efConstruction"
            description="Build-time search width. Higher = slower build, better graph quality."
            value={options.hnswEfConstruction}
            onChange={(v) => onChange({ hnswEfConstruction: v })}
            min={16}
            max={800}
            step={16}
            marks={[
              { value: 100, label: "100" },
              { value: 200, label: "200" },
              { value: 400, label: "400" },
              { value: 800, label: "800" },
            ]}
          />

          <SliderKnob
            label="efSearch"
            description="Query-time search width. Bump this for tighter pre-filtered queries."
            value={options.hnswEfSearch}
            onChange={(v) => onChange({ hnswEfSearch: v })}
            min={16}
            max={800}
            step={16}
            marks={[
              { value: 50, label: "50" },
              { value: 100, label: "100" },
              { value: 200, label: "200" },
              { value: 800, label: "800" },
            ]}
          />

          <Tooltip
            label={
              int8Allowed
                ? "Stores each coordinate as int8 (-128..127). Memory drops ~4× with a small recall cost."
                : "Quantization currently requires cosine similarity with the HNSW provider."
            }
            withArrow
            multiline
            w={260}
          >
            <Switch
              label="int8 quantization (~4× smaller memory)"
              size="sm"
              checked={options.quantization === "int8"}
              disabled={!int8Allowed}
              onChange={(e) =>
                onChange({
                  quantization: e.currentTarget.checked ? "int8" : "none",
                })
              }
            />
          </Tooltip>
        </>
      )}

      <Divider my={4} />

      <Tooltip
        label="Skip the initial backfill at CREATE time. The index marks Populating and the first query fills it on demand."
        withArrow
        multiline
        w={260}
      >
        <Switch
          label="Populate asynchronously (lazy)"
          size="sm"
          checked={options.populateAsync}
          onChange={(e) =>
            onChange({ populateAsync: e.currentTarget.checked })
          }
        />
      </Tooltip>

      <Paper
        withBorder
        p="xs"
        style={{
          background: tokens.bg.overlay,
        }}
      >
        <Text size="xs" c={tokens.fg.muted} tt="uppercase" fw={600}>
          Quick read
        </Text>
        <Text size="xs" c={tokens.fg.primary} mt={4}>
          {quickReadCopy(options)}
        </Text>
      </Paper>
    </Stack>
  );
}

function SliderKnob({
  label,
  description,
  value,
  onChange,
  min,
  max,
  step,
  marks,
}: {
  label: string;
  description: string;
  value: number;
  onChange: (v: number) => void;
  min: number;
  max: number;
  step: number;
  marks: { value: number; label: string }[];
}) {
  const { tokens } = usePlaygroundTheme();
  return (
    <Stack gap={2}>
      <Group justify="space-between" align="baseline">
        <Text size="sm" fw={500}>
          {label}
        </Text>
        <Badge size="sm" variant="light">
          {value}
        </Badge>
      </Group>
      <Slider
        value={value}
        onChange={onChange}
        min={min}
        max={max}
        step={step}
        marks={marks}
        thumbSize={14}
        styles={{ markLabel: { fontSize: 10 } }}
      />
      <Text size="xs" c={tokens.fg.muted}>
        {description}
      </Text>
    </Stack>
  );
}

/**
 * Best-effort summary of the chosen tuning. Avoids hard numbers
 * (every workload is different) and instead points at the qualitative
 * tradeoff each toggle pushes you toward.
 */
function quickReadCopy(opts: VectorIndexOptions): string {
  const fragments: string[] = [];
  fragments.push(`d=${opts.dimensions}`);
  fragments.push(`${opts.similarity}`);
  if (opts.provider === "flat") {
    fragments.push(
      "Flat: exact recall, query cost grows linearly with N.",
    );
  } else {
    fragments.push(
      `HNSW M=${opts.hnswM}, ef build/search=${opts.hnswEfConstruction}/${opts.hnswEfSearch}. Higher ef = higher recall + slower queries.`,
    );
  }
  if (opts.quantization === "int8") {
    fragments.push("int8 storage saves ~4× memory at a small recall cost.");
  }
  if (opts.populateAsync) {
    fragments.push("Async populate: CREATE returns fast, first query backfills.");
  }
  return fragments.join(" · ");
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
