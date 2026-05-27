import React from "react";
import clsx from "clsx";
import Head from "@docusaurus/Head";

import styles from "./styles.module.scss";

/**
 * HowTo block + schema.org HowTo JSON-LD.
 *
 * Renders a header (eyebrow label, title, description, optional time
 * badge) plus a vertical timeline of numbered steps, and emits a single
 * HowTo JSON-LD blob that names each step. Use for tutorials, install
 * paths, and cookbook recipes — anywhere a reader is following ordered
 * instructions with a measurable outcome.
 *
 *   <HowTo
 *     name="Install LoraDB in Python"
 *     description="Install the lora-python wheel and run a one-line query."
 *     totalTime="PT2M"
 *     steps={[
 *       { name: "Install", text: "Run `pip install lora-python`." },
 *       { name: "Verify", text: "Run `python -c \"import lora\"`." },
 *     ]}
 *   />
 *
 * Step `text` is the primary description (required). Backticked
 * fragments in `text` render as inline <code>. Optional fields per step:
 * `url` (deep link to the step on this page), `image` (URL).
 *
 * totalTime, if present, must be an ISO 8601 duration (PT2M = 2 minutes,
 * PT1H30M = 1 hour 30 minutes, etc.) and is surfaced as a small badge in
 * the header.
 *
 * One <HowTo> per page is the safe upper bound — multiple HowTo blocks
 * on a single URL is allowed by the spec but commonly downranked.
 */

const ISO_DURATION = /^PT(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?$/;

function formatDuration(iso) {
  if (typeof iso !== "string") return null;
  const match = ISO_DURATION.exec(iso);
  if (!match) return null;
  const [, h, m, s] = match;
  const parts = [];
  if (h) parts.push(`${h} hr`);
  if (m) parts.push(`${m} min`);
  if (s && !h && !m) parts.push(`${s} sec`);
  if (parts.length === 0) return null;
  return `~${parts.join(" ")}`;
}

function renderInlineCode(text) {
  if (typeof text !== "string") return text;
  if (!text.includes("`")) return text;
  return text
    .split(/(`[^`]+`)/g)
    .map((part, i) =>
      part.startsWith("`") && part.endsWith("`") && part.length > 1 ? (
        <code key={i}>{part.slice(1, -1)}</code>
      ) : (
        <React.Fragment key={i}>{part}</React.Fragment>
      ),
    );
}

function ClockIcon() {
  return (
    <svg
      width="13"
      height="13"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5l3 2" />
    </svg>
  );
}

export default function HowTo({
  name,
  description,
  totalTime,
  steps,
  className,
  kind = "How-to",
}) {
  if (!Array.isArray(steps) || steps.length === 0) {
    return null;
  }

  const schema = {
    "@context": "https://schema.org",
    "@type": "HowTo",
    name,
    description,
    step: steps.map((step, idx) => {
      const node = {
        "@type": "HowToStep",
        position: idx + 1,
        name: step.name || `Step ${idx + 1}`,
        text: step.text,
      };
      if (step.url) node.url = step.url;
      if (step.image) node.image = step.image;
      return node;
    }),
  };
  if (totalTime) {
    schema.totalTime = totalTime;
  }

  const timeLabel = formatDuration(totalTime);

  return (
    <>
      <Head>
        <script type="application/ld+json">{JSON.stringify(schema)}</script>
      </Head>
      <section
        className={clsx(styles.howto, className)}
        aria-label={`${kind}: ${name}`}
      >
        <header className={styles.header}>
          <div className={styles.headerMeta}>
            <span className={styles.eyebrow}>{kind}</span>
            {timeLabel && (
              <span
                className={styles.time}
                aria-label={`Estimated time ${timeLabel}`}
              >
                <ClockIcon />
                {timeLabel}
              </span>
            )}
          </div>
          <h3 className={styles.title}>{name}</h3>
          {description && <p className={styles.description}>{description}</p>}
        </header>
        <ol className={styles.steps}>
          {steps.map((step, idx) => (
            <li key={idx} className={styles.step}>
              <div className={styles.marker} aria-hidden="true">
                <span className={styles.number}>{idx + 1}</span>
              </div>
              <div className={styles.body}>
                {step.name && (
                  <div className={styles.stepName}>{step.name}</div>
                )}
                <div className={styles.stepText}>
                  {renderInlineCode(step.text)}
                </div>
                {step.url && (
                  <a
                    className={styles.jump}
                    href={step.url}
                    aria-label={`Jump to ${step.name || `step ${idx + 1}`}`}
                  >
                    Jump to section
                    <span className={styles.jumpArrow} aria-hidden="true">
                      →
                    </span>
                  </a>
                )}
              </div>
            </li>
          ))}
        </ol>
      </section>
    </>
  );
}
