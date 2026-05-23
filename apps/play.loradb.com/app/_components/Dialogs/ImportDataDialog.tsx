"use client";

/**
 * `ImportDataDialog` — wizard-driven row-level data import.
 *
 * Five phases the user moves through:
 *   1. **File** — pick or drop a file; we sniff the head 256 KiB to
 *      detect the format and project an estimated row count.
 *   2. **Target** — choose Node, Relationship, or Custom Cypher.
 *   3. **Mapping** — fill in the appropriate form; see a sample of
 *      the file's rows and (for CSV) override per-column types.
 *   4. **Review** — dry-run the import end-to-end without mutating
 *      state; confirm the generated Cypher + projected row count.
 *   5. **Running** — live progress with bytes/rows/throughput/ETA
 *      and a Cancel button driven by an `AbortController`.
 *
 * Phase 5 ends in either a Done summary (with post-import actions)
 * or an Error summary (with a Back-to-mapping affordance). Snapshots
 * stay on their own panel — this dialog only handles row data.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Divider, Stack, Stepper } from "@mantine/core";
import { modals } from "@mantine/modals";
import {
  IconCheck,
  IconCode,
  IconFileText,
  IconHexagon,
} from "@tabler/icons-react";
import type {
  RowFormat,
  RowImportProgress,
  RowMapping,
  RowParseError,
} from "@loradb/lora-wasm";

import { importStream } from "@/lib/db/client";
import {
  applySmartDefaults,
  buildPreview,
  detectRowFormat,
  effectiveBatchSize,
  renderMappingTemplate,
  wrapStream,
  type MappingKind,
  type PreviewState,
} from "@/lib/importData/rowImport";
import {
  ThroughputTracker,
  type ThroughputReading,
} from "@/lib/util/throughput";
import {
  DonePhase,
  ErrorPhase,
  FileStep,
  Footer,
  MappingStep,
  ReviewStep,
  RunningPhase,
  TargetStep,
} from "./importData/ImportDataSteps";

// ---------------------------------------------------------------------------
// Dialog
// ---------------------------------------------------------------------------

interface ImportDataDialogProps {
  modalId: string;
  initialFile?: File;
}

function ImportDataDialog({ modalId, initialFile }: ImportDataDialogProps) {
  const [phase, setPhase] = useState<"config" | "running" | "done" | "error">(
    "config",
  );
  const [step, setStep] = useState(0); // 0=file, 1=target, 2=mapping, 3=review
  const [file, setFile] = useState<File | null>(initialFile ?? null);
  const [format, setFormat] = useState<RowFormat>("jsonl");
  const [preview, setPreview] = useState<PreviewState | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);

  // Mapping state.
  const [mappingKind, setMappingKind] = useState<MappingKind>("node");
  const [label, setLabel] = useState("Imported");
  const [idColumn, setIdColumn] = useState<string | null>(null);
  const [propertyColumns, setPropertyColumns] = useState<string[]>([]);

  const [relType, setRelType] = useState("RELATED_TO");
  const [startLabel, setStartLabel] = useState("");
  const [startColumn, setStartColumn] = useState<string | null>(null);
  const [startMatchProperty, setStartMatchProperty] = useState("id");
  const [endLabel, setEndLabel] = useState("");
  const [endColumn, setEndColumn] = useState<string | null>(null);
  const [endMatchProperty, setEndMatchProperty] = useState("id");

  const [template, setTemplate] = useState(
    "UNWIND $rows AS r CREATE (:Imported {\n  // fill in properties from r.*\n})",
  );
  const [columnTypes, setColumnTypes] = useState<Record<string, string>>({});
  const [batchSize, setBatchSize] = useState<number | "">(1000);
  const [permissive, setPermissive] = useState(false);

  // Review-step state.
  const [reviewing, setReviewing] = useState(false);
  const [reviewStats, setReviewStats] = useState<{
    rows: number;
    batches: number;
    skipped: number;
  } | null>(null);
  const [reviewError, setReviewError] = useState<string | null>(null);

  // Running-state.
  const [progress, setProgress] = useState<RowImportProgress | null>(null);
  const [throughput, setThroughput] = useState<ThroughputReading | null>(null);
  const [runError, setRunError] = useState<string | null>(null);
  const [runStats, setRunStats] = useState<{
    rows: number;
    batches: number;
    durationMs: number;
    skipped: number;
    errors: RowParseError[];
  } | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const mountedRef = useRef(true);
  const previewRequestRef = useRef(0);
  const reviewRequestRef = useRef(0);

  // Abort any in-flight import when the dialog unmounts — covers the
  // X button, ESC key, outside-click, and the Footer's Close button,
  // none of which route through the explicit Cancel handler.
  useEffect(
    () => () => {
      mountedRef.current = false;
      previewRequestRef.current += 1;
      reviewRequestRef.current += 1;
      abortRef.current?.abort(new DOMException("dialog closed", "AbortError"));
    },
    [],
  );

  const close = useCallback(() => modals.close(modalId), [modalId]);

  // -------------------------------------------------------------------------
  // Sniff the file whenever it changes
  // -------------------------------------------------------------------------
  useEffect(() => {
    const requestId = ++previewRequestRef.current;
    if (!file) {
      setPreview(null);
      setPreviewError(null);
      return;
    }
    const detected = detectRowFormat(file.name);
    if (detected) setFormat(detected);
    void buildPreview(file)
      .then(({ preview, format: f }) => {
        if (!mountedRef.current || requestId !== previewRequestRef.current) {
          return;
        }
        if (f) setFormat(f);
        setPreview(preview);
        setPreviewError(null);
        applySmartDefaults(file.name, preview, {
          setLabel,
          setMappingKind,
          setRelType,
          setIdColumn,
          setStartColumn,
          setEndColumn,
          setStartLabel,
          setEndLabel,
          setPropertyColumns,
          setColumnTypes,
        });
      })
      .catch((err: unknown) => {
        if (!mountedRef.current || requestId !== previewRequestRef.current) {
          return;
        }
        setPreviewError(err instanceof Error ? err.message : String(err));
        setPreview(null);
      });
  }, [file]);

  // -------------------------------------------------------------------------
  // Derived values
  // -------------------------------------------------------------------------
  const mapping: RowMapping | null = useMemo(() => {
    if (mappingKind === "node") {
      if (!label.trim()) return null;
      return {
        kind: "node",
        label: label.trim(),
        id_column: idColumn ?? null,
        id_property: idColumn ?? null,
        properties: propertyColumns.map((c) => ({ source: c, property: c })),
      };
    }
    if (mappingKind === "relationship") {
      if (
        !relType.trim() ||
        !startLabel.trim() ||
        !endLabel.trim() ||
        !startColumn ||
        !endColumn ||
        !startMatchProperty.trim() ||
        !endMatchProperty.trim()
      ) {
        return null;
      }
      return {
        kind: "relationship",
        rel_type: relType.trim(),
        start_label: startLabel.trim(),
        start_column: startColumn,
        start_match_property: startMatchProperty.trim(),
        end_label: endLabel.trim(),
        end_column: endColumn,
        end_match_property: endMatchProperty.trim(),
        properties: propertyColumns.map((c) => ({ source: c, property: c })),
      };
    }
    return null;
  }, [
    mappingKind,
    label,
    idColumn,
    propertyColumns,
    relType,
    startLabel,
    startColumn,
    startMatchProperty,
    endLabel,
    endColumn,
    endMatchProperty,
  ]);

  const previewCypher = useMemo(() => {
    if (mappingKind === "template") return template.trim();
    if (!mapping) return null;
    return renderMappingTemplate(mapping);
  }, [mappingKind, mapping, template]);

  const canAdvanceFromMapping =
    mappingKind === "template" ? template.trim().length > 0 : mapping !== null;

  // -------------------------------------------------------------------------
  // Phase / step transitions
  // -------------------------------------------------------------------------
  const target: RowMapping | string | null = useMemo(() => {
    if (mappingKind === "template") {
      return template.trim().length > 0 ? template : null;
    }
    return mapping;
  }, [mappingKind, template, mapping]);

  const goReview = useCallback(async () => {
    if (!file || !target) return;
    const requestId = ++reviewRequestRef.current;
    setStep(3);
    setReviewing(true);
    setReviewError(null);
    setReviewStats(null);
    try {
      const batch = effectiveBatchSize(batchSize);
      const sourceStream = wrapStream(file, format, columnTypes);
      const stats = await importStream(sourceStream, format, target, {
        batchSize: batch,
        dryRun: true,
        permissive,
      });
      if (!mountedRef.current || requestId !== reviewRequestRef.current) {
        return;
      }
      setReviewStats({
        rows: stats.rows,
        batches: stats.batches,
        skipped: stats.skipped,
      });
    } catch (err) {
      if (!mountedRef.current || requestId !== reviewRequestRef.current) {
        return;
      }
      setReviewError(err instanceof Error ? err.message : String(err));
    } finally {
      if (mountedRef.current && requestId === reviewRequestRef.current) {
        setReviewing(false);
      }
    }
  }, [file, target, batchSize, format, columnTypes, permissive]);

  const startImport = useCallback(async () => {
    if (!file || !target) return;
    setPhase("running");
    setProgress(null);
    setThroughput(null);
    setRunError(null);
    setRunStats(null);
    const controller = new AbortController();
    abortRef.current = controller;
    const tracker = new ThroughputTracker(2_000);
    const startTime = Date.now();
    // The worker emits one progress callback per fed chunk, which for a
    // multi-million-row CSV is thousands of callbacks. Without batching,
    // each one triggers a full React rerender of the dialog and starves
    // the main thread. Buffer the latest reading in refs and flush once
    // per animation frame; the user sees a smooth update, the engine
    // doesn't wait on the UI.
    const latestProgress: { value: RowImportProgress | null } = { value: null };
    let rafHandle: number | null = null;
    let scheduledWithTimeout = false;
    const cancelProgressFlush = () => {
      if (rafHandle === null) return;
      if (scheduledWithTimeout || typeof cancelAnimationFrame === "undefined") {
        clearTimeout(rafHandle);
      } else {
        cancelAnimationFrame(rafHandle);
      }
      rafHandle = null;
      scheduledWithTimeout = false;
    };
    const flush = () => {
      rafHandle = null;
      scheduledWithTimeout = false;
      if (!mountedRef.current) return;
      const p = latestProgress.value;
      if (!p) return;
      setProgress(p);
      setThroughput(tracker.read(file.size));
    };
    try {
      const batch = effectiveBatchSize(batchSize);
      const sourceStream = wrapStream(file, format, columnTypes);
      const stats = await importStream(sourceStream, format, target, {
        batchSize: batch,
        permissive,
        signal: controller.signal,
        onProgress: (p) => {
          tracker.record(p.bytesFed, p.rowsCommitted);
          latestProgress.value = p;
          if (rafHandle === null) {
            if (typeof requestAnimationFrame !== "undefined") {
              rafHandle = requestAnimationFrame(flush);
              scheduledWithTimeout = false;
            } else {
              rafHandle = setTimeout(flush, 16) as unknown as number;
              scheduledWithTimeout = true;
            }
          }
        },
      });
      cancelProgressFlush();
      // Make sure the user sees the final pre-completion reading.
      if (mountedRef.current && latestProgress.value) {
        setProgress(latestProgress.value);
        setThroughput(tracker.read(file.size));
      }
      if (mountedRef.current) {
        setRunStats({
          rows: stats.rows,
          batches: stats.batches,
          durationMs: Date.now() - startTime,
          skipped: stats.skipped,
          errors: stats.errors,
        });
        setPhase("done");
      }
    } catch (err) {
      cancelProgressFlush();
      if (!mountedRef.current) return;
      if ((err as { name?: string } | null)?.name === "AbortError") {
        setRunError("Import cancelled before completion.");
      } else {
        setRunError(err instanceof Error ? err.message : String(err));
      }
      setPhase("error");
    } finally {
      abortRef.current = null;
    }
  }, [file, target, batchSize, format, columnTypes, permissive]);

  const cancelImport = useCallback(() => {
    abortRef.current?.abort(new DOMException("user cancelled", "AbortError"));
  }, []);

  const resetToMapping = useCallback(() => {
    setPhase("config");
    setStep(2);
    setRunError(null);
    setRunStats(null);
    setProgress(null);
  }, []);

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------
  return (
    <Stack gap="md" mih={520}>
      {phase === "config" && (
        <Stepper
          active={step}
          onStepClick={(s) => {
            // Allow back-navigation to any earlier step; forward
            // jumps only on the last validated step.
            if (s <= step) setStep(s);
          }}
          allowNextStepsSelect={false}
          size="sm"
        >
          <Stepper.Step label="File" icon={<IconFileText size={14} />}>
            <FileStep
              file={file}
              format={format}
              preview={preview}
              previewError={previewError}
              onPickFile={setFile}
            />
          </Stepper.Step>
          <Stepper.Step label="Target" icon={<IconHexagon size={14} />}>
            <TargetStep
              mappingKind={mappingKind}
              onPick={(k) => {
                setMappingKind(k);
                setStep(2);
              }}
            />
          </Stepper.Step>
          <Stepper.Step label="Mapping" icon={<IconCode size={14} />}>
            <MappingStep
              mappingKind={mappingKind}
              format={format}
              preview={preview}
              previewCypher={previewCypher}
              label={label}
              setLabel={setLabel}
              idColumn={idColumn}
              setIdColumn={setIdColumn}
              propertyColumns={propertyColumns}
              setPropertyColumns={setPropertyColumns}
              relType={relType}
              setRelType={setRelType}
              startLabel={startLabel}
              setStartLabel={setStartLabel}
              startColumn={startColumn}
              setStartColumn={setStartColumn}
              startMatchProperty={startMatchProperty}
              setStartMatchProperty={setStartMatchProperty}
              endLabel={endLabel}
              setEndLabel={setEndLabel}
              endColumn={endColumn}
              setEndColumn={setEndColumn}
              endMatchProperty={endMatchProperty}
              setEndMatchProperty={setEndMatchProperty}
              template={template}
              setTemplate={setTemplate}
              columnTypes={columnTypes}
              setColumnTypes={setColumnTypes}
            />
          </Stepper.Step>
          <Stepper.Step label="Review" icon={<IconCheck size={14} />}>
            <ReviewStep
              file={file}
              format={format}
              preview={preview}
              previewCypher={previewCypher}
              batchSize={batchSize}
              setBatchSize={setBatchSize}
              permissive={permissive}
              setPermissive={setPermissive}
              reviewing={reviewing}
              reviewStats={reviewStats}
              reviewError={reviewError}
            />
          </Stepper.Step>
        </Stepper>
      )}

      {phase === "running" && (
        <RunningPhase
          file={file}
          progress={progress}
          throughput={throughput}
          previewCypher={previewCypher}
          estimatedRows={preview?.estimatedRows ?? null}
        />
      )}

      {phase === "done" && runStats && (
        <DonePhase
          stats={runStats}
          onClose={close}
          onImportAnother={() => {
            // Reset everything except the format/mappingKind so the
            // next file can re-use the same configuration if the
            // user just wants to import a sibling file.
            setFile(null);
            setStep(0);
            setPreview(null);
            setRunStats(null);
            setProgress(null);
            setReviewStats(null);
            setPhase("config");
          }}
        />
      )}

      {phase === "error" && (
        <ErrorPhase
          message={runError ?? "Unknown error"}
          partialStats={progress}
          onBack={resetToMapping}
        />
      )}

      <Divider />
      <Footer
        phase={phase}
        step={step}
        canAdvanceFromFile={!!file && !!preview && !previewError}
        canAdvanceFromTarget={true}
        canAdvanceFromMapping={canAdvanceFromMapping}
        canRunFromReview={!!file && !!target && !reviewing && !reviewError}
        onBack={() => setStep((s) => Math.max(0, s - 1))}
        onNext={() => {
          if (step === 2) {
            void goReview();
          } else {
            setStep((s) => s + 1);
          }
        }}
        onRun={() => void startImport()}
        onCancel={cancelImport}
        onClose={close}
      />
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Modal entry point
// ---------------------------------------------------------------------------

/** Opens the import-data wizard. */
export function openImportDataDialog(opts: { file?: File } = {}): void {
  const id = "loradb-import-data-dialog";
  modals.open({
    modalId: id,
    title: "Import data",
    size: "xl",
    centered: true,
    children: <ImportDataDialog modalId={id} initialFile={opts.file} />,
  });
}
