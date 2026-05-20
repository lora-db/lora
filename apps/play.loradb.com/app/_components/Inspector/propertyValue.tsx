"use client";

/**
 * Property-value detection + rendering. A single switch over a
 * detected {@link PropertyKind} drives every row in the node card.
 *
 * Detection is *defensive*: any value we can't classify falls back to
 * a JSON dump so the card never crashes on novel engine output.
 */

import { Anchor, Badge, Code, Group, Stack, Text } from "@mantine/core";
import { IconCheck, IconX } from "@tabler/icons-react";

import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";

export type PropertyKind =
  | "null"
  | "boolean"
  | "integer"
  | "float"
  | "bigint"
  | "string"
  | "url"
  | "email"
  | "datetime"
  | "duration"
  | "point"
  | "array"
  | "object";

export type SemanticGroup =
  | "identifiers"
  | "descriptors"
  | "temporal"
  | "spatial"
  | "other";

const URL_RE = /^(https?:\/\/|www\.)[^\s]+$/i;
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
// Permissive ISO 8601 — date, date-time, optional timezone.
const ISO_DT_RE =
  /^\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:?\d{2})?)?$/;
const TITLE_PROPERTY_HINTS = [
  "name",
  "title",
  "label",
  "displayName",
  "display_name",
  "fullName",
  "username",
  "email",
];

/**
 * Detect what kind of property value we're looking at. The order
 * matters: we test more specific forms before falling back to
 * generic ones (e.g., url before plain string).
 */
export function detectPropertyKind(value: unknown): PropertyKind {
  if (value === null || value === undefined) return "null";
  if (typeof value === "boolean") return "boolean";
  if (typeof value === "bigint") return "bigint";
  if (typeof value === "number") {
    return Number.isInteger(value) ? "integer" : "float";
  }
  if (typeof value === "string") {
    if (URL_RE.test(value)) return "url";
    if (EMAIL_RE.test(value)) return "email";
    if (ISO_DT_RE.test(value)) return "datetime";
    return "string";
  }
  if (Array.isArray(value)) return "array";
  if (typeof value === "object") {
    // Neo4j-style point: { srid, x, y[, z] }
    const o = value as Record<string, unknown>;
    if (
      typeof o.srid === "number" &&
      typeof o.x === "number" &&
      typeof o.y === "number"
    ) {
      return "point";
    }
    // Engine duration: { months, days, seconds, nanoseconds }
    if (
      typeof o.months === "number" &&
      typeof o.days === "number" &&
      typeof o.seconds === "number"
    ) {
      return "duration";
    }
    return "object";
  }
  return "string";
}

/**
 * Pick the most "title-like" property key — the value that should
 * appear in the card header above the id.
 */
export function pickTitleProperty(
  properties: Record<string, unknown>,
): string | null {
  for (const hint of TITLE_PROPERTY_HINTS) {
    if (
      hint in properties &&
      typeof properties[hint] === "string" &&
      (properties[hint] as string).trim().length > 0
    ) {
      return hint;
    }
  }
  return null;
}

/**
 * Bucket a property into one of five semantic groups so the card body
 * can render them in a stable, useful order. The `constrainedKeys`
 * set bubbles indexed/constrained properties up to the top.
 */
export function semanticGroupFor(
  key: string,
  value: unknown,
  constrainedKeys: ReadonlySet<string>,
): SemanticGroup {
  if (constrainedKeys.has(key)) return "identifiers";
  if (key === "id" || /(^id$|_id$|Id$|^uuid$|Uuid$)/.test(key)) {
    return "identifiers";
  }
  const kind = detectPropertyKind(value);
  if (kind === "datetime" || kind === "duration") return "temporal";
  if (kind === "point") return "spatial";
  if (
    TITLE_PROPERTY_HINTS.includes(key) ||
    /^(description|summary|bio|about)$/i.test(key)
  ) {
    return "descriptors";
  }
  return "other";
}

export const GROUP_ORDER: readonly SemanticGroup[] = [
  "identifiers",
  "descriptors",
  "temporal",
  "spatial",
  "other",
];

export const GROUP_LABEL: Record<SemanticGroup, string> = {
  identifiers: "Identifiers",
  descriptors: "Descriptors",
  temporal: "Temporal",
  spatial: "Spatial",
  other: "Other",
};

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

interface PropertyValueProps {
  value: unknown;
  /** Force-expand long strings / nested objects. */
  expanded?: boolean;
}

export function PropertyValue({ value, expanded }: PropertyValueProps) {
  const { tokens } = usePlaygroundTheme();
  const kind = detectPropertyKind(value);

  switch (kind) {
    case "null":
      return (
        <Text size="xs" c={tokens.fg.subtle} ff={tokens.font.mono}>
          —
        </Text>
      );
    case "boolean":
      return (
        <Badge
          size="xs"
          variant="light"
          color={value === true ? "green" : "gray"}
          leftSection={
            value === true ? <IconCheck size={10} /> : <IconX size={10} />
          }
        >
          {value === true ? "true" : "false"}
        </Badge>
      );
    case "integer":
    case "float":
      return (
        <Text
          size="xs"
          ff={tokens.font.mono}
          c={tokens.fg.primary}
          style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}
        >
          {kind === "integer"
            ? (value as number).toLocaleString()
            : (value as number).toLocaleString(undefined, {
                maximumFractionDigits: 6,
              })}
        </Text>
      );
    case "bigint":
      return (
        <Text
          size="xs"
          ff={tokens.font.mono}
          c={tokens.fg.primary}
          style={{ textAlign: "right" }}
        >
          {(value as bigint).toString()}n
        </Text>
      );
    case "url":
      return (
        <Anchor
          href={value as string}
          target="_blank"
          rel="noreferrer noopener"
          size="xs"
          style={{ wordBreak: "break-all" }}
        >
          {humanHost(value as string)}
        </Anchor>
      );
    case "email":
      return (
        <Anchor href={`mailto:${value as string}`} size="xs">
          {value as string}
        </Anchor>
      );
    case "datetime":
      return <DatetimeValue iso={value as string} />;
    case "duration":
      return (
        <Text size="xs" ff={tokens.font.mono} c={tokens.fg.primary}>
          {formatDuration(value as DurationLike)}
        </Text>
      );
    case "point":
      return <PointValue point={value as PointLike} />;
    case "array":
      return <ArrayValue values={value as unknown[]} expanded={expanded} />;
    case "object":
      return <ObjectValue value={value as Record<string, unknown>} />;
    case "string":
    default: {
      const s = String(value ?? "");
      return (
        <StringValue text={s} expanded={expanded} mono={tokens.font.mono} />
      );
    }
  }
}

// ---------------------------------------------------------------------------
// Per-kind sub-renderers
// ---------------------------------------------------------------------------

function StringValue({
  text,
  expanded,
  mono,
}: {
  text: string;
  expanded?: boolean;
  mono: string;
}) {
  const { tokens } = usePlaygroundTheme();
  const long = text.length > 80;
  return (
    <Text
      size="xs"
      ff={mono}
      c={tokens.fg.primary}
      lineClamp={expanded || !long ? undefined : 2}
      style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}
    >
      {text}
    </Text>
  );
}

function DatetimeValue({ iso }: { iso: string }) {
  const { tokens } = usePlaygroundTheme();
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return (
      <Text size="xs" ff={tokens.font.mono} c={tokens.fg.primary}>
        {iso}
      </Text>
    );
  }
  return (
    <Stack gap={0}>
      <Text size="xs" c={tokens.fg.primary}>
        {humanRelative(date)}
      </Text>
      <Text size="xs" ff={tokens.font.mono} c={tokens.fg.subtle}>
        {iso}
      </Text>
    </Stack>
  );
}

interface PointLike {
  srid: number;
  x: number;
  y: number;
  z?: number;
}

function PointValue({ point }: { point: PointLike }) {
  const { tokens } = usePlaygroundTheme();
  const z = typeof point.z === "number" ? ` ${point.z}` : "";
  const wkt = `POINT(${point.x} ${point.y}${z})`;
  return (
    <Stack gap={0}>
      <Code style={{ background: tokens.bg.panel, fontSize: 11 }}>{wkt}</Code>
      <Text size="xs" c={tokens.fg.subtle}>
        srid {point.srid}
      </Text>
    </Stack>
  );
}

interface DurationLike {
  months: number;
  days: number;
  seconds: number;
  nanoseconds?: number;
}

function formatDuration(d: DurationLike): string {
  const parts: string[] = [];
  if (d.months) parts.push(`${d.months}mo`);
  if (d.days) parts.push(`${d.days}d`);
  if (d.seconds) {
    const h = Math.floor(d.seconds / 3600);
    const m = Math.floor((d.seconds % 3600) / 60);
    const s = d.seconds % 60;
    if (h) parts.push(`${h}h`);
    if (m) parts.push(`${m}m`);
    if (s) parts.push(`${s}s`);
  }
  if (parts.length === 0) return "0s";
  return parts.join(" ");
}

function ArrayValue({
  values,
  expanded,
}: {
  values: unknown[];
  expanded?: boolean;
}) {
  const { tokens } = usePlaygroundTheme();
  if (values.length === 0) {
    return (
      <Text size="xs" c={tokens.fg.subtle}>
        empty list
      </Text>
    );
  }
  if (!expanded && values.length > 3) {
    return (
      <Group gap={6} wrap="wrap">
        {values.slice(0, 3).map((v, i) => (
          <Badge
            key={i}
            size="sm"
            variant="light"
            color="gray"
            radius="sm"
            style={{ fontFamily: tokens.font.mono, textTransform: "none" }}
          >
            {compactValue(v)}
          </Badge>
        ))}
        <Text size="xs" c={tokens.fg.subtle}>
          +{values.length - 3} more
        </Text>
      </Group>
    );
  }
  return (
    <Group gap={6} wrap="wrap">
      {values.map((v, i) => (
        <Badge
          key={i}
          size="sm"
          variant="light"
          color="gray"
          radius="sm"
          style={{ fontFamily: tokens.font.mono, textTransform: "none" }}
        >
          {compactValue(v)}
        </Badge>
      ))}
    </Group>
  );
}

function ObjectValue({ value }: { value: Record<string, unknown> }) {
  const { tokens } = usePlaygroundTheme();
  const text = (() => {
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  })();
  return (
    <Code
      block
      style={{
        background: tokens.bg.panel,
        color: tokens.fg.primary,
        fontFamily: tokens.font.mono,
        fontSize: 11,
        whiteSpace: "pre-wrap",
        wordBreak: "break-word",
      }}
    >
      {text}
    </Code>
  );
}

function compactValue(v: unknown): string {
  if (v === null || v === undefined) return "—";
  if (typeof v === "string") return v.length > 24 ? `${v.slice(0, 24)}…` : v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  try {
    return JSON.stringify(v);
  } catch {
    return String(v);
  }
}

function humanHost(url: string): string {
  try {
    const u = new URL(url.startsWith("www.") ? `https://${url}` : url);
    return u.hostname + (u.pathname === "/" ? "" : u.pathname);
  } catch {
    return url;
  }
}

function humanRelative(d: Date): string {
  const diff = d.getTime() - Date.now();
  const abs = Math.abs(diff);
  const seconds = Math.round(abs / 1000);
  const minutes = Math.round(seconds / 60);
  const hours = Math.round(minutes / 60);
  const days = Math.round(hours / 24);
  const months = Math.round(days / 30);
  const years = Math.round(days / 365);
  const past = diff < 0;
  const ago = (n: number, unit: string) =>
    past ? `${n} ${unit} ago` : `in ${n} ${unit}`;
  if (seconds < 45) return past ? "just now" : "in a moment";
  if (minutes < 45) return ago(minutes, minutes === 1 ? "minute" : "minutes");
  if (hours < 22) return ago(hours, hours === 1 ? "hour" : "hours");
  if (days < 26) return ago(days, days === 1 ? "day" : "days");
  if (months < 11) return ago(months, months === 1 ? "month" : "months");
  return ago(years, years === 1 ? "year" : "years");
}

/**
 * Plain-text rendering used for "copy value" actions and diff output.
 * Mirrors the visual rendering closely enough that "copy" feels
 * predictable to the user.
 */
export function renderValueText(value: unknown): string {
  const kind = detectPropertyKind(value);
  switch (kind) {
    case "null":
      return "null";
    case "boolean":
    case "integer":
    case "float":
    case "string":
    case "url":
    case "email":
    case "datetime":
      return String(value);
    case "bigint":
      return `${(value as bigint).toString()}n`;
    case "point": {
      const p = value as PointLike;
      const z = typeof p.z === "number" ? ` ${p.z}` : "";
      return `POINT(${p.x} ${p.y}${z})`;
    }
    case "duration":
      return formatDuration(value as DurationLike);
    case "array":
    case "object":
      try {
        return JSON.stringify(value, null, 2);
      } catch {
        return String(value);
      }
  }
}
