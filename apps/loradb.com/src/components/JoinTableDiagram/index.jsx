import React from "react";

import styles from "./styles.module.scss";

/**
 * The same many-to-many drawn twice: as three relational tables, and as
 * two nodes joined by one relationship. The join table and the
 * relationship share the accent colour, because one becomes the other.
 *
 * Colours come from the theme tokens, so the figure follows light and
 * dark mode. Used by docs/graphql/many-to-many.md.
 */

const ROW = 24;

function Table({ x, y, width, name, rows, accent }) {
  const height = ROW * (rows.length + 1);
  const header = `M${x} ${y + ROW}V${y + 6}a6 6 0 0 1 6-6h${width - 12}a6 6 0 0 1 6 6v${ROW - 6}z`;
  return (
    <g className={accent ? styles.accent : undefined}>
      <rect
        className={styles.box}
        x={x}
        y={y}
        width={width}
        height={height}
        rx="6"
      />
      <path className={styles.header} d={header} />
      <text className={styles.tableName} x={x + 10} y={y + 16}>
        {name}
      </text>
      {rows.map(([column, tag], i) => {
        const baseline = y + ROW * (i + 1) + 16;
        return (
          <g key={column}>
            <text className={styles.column} x={x + 10} y={baseline}>
              {column}
            </text>
            {tag ? (
              <text
                className={styles.tag}
                x={x + width - 10}
                y={baseline}
                textAnchor="end"
              >
                {tag}
              </text>
            ) : null}
          </g>
        );
      })}
    </g>
  );
}

function Node({ cx, label, sub }) {
  return (
    <g>
      <circle className={styles.node} cx={cx} cy="100" r="46" />
      <text className={styles.nodeLabel} x={cx} y="98" textAnchor="middle">
        {label}
      </text>
      <text className={styles.nodeSub} x={cx} y="116" textAnchor="middle">
        {sub}
      </text>
    </g>
  );
}

export default function JoinTableDiagram() {
  return (
    <figure className={styles.figure}>
      <div className={styles.panels}>
        <div className={styles.panel}>
          <div className={styles.eyebrow}>Relational</div>
          <svg
            viewBox="0 0 360 224"
            role="img"
            aria-label="Three tables: students, courses, and an enrollments join table holding a foreign key to each plus grade and role columns."
          >
            <defs>
              <marker
                id="jtd-dot"
                viewBox="0 0 8 8"
                refX="4"
                refY="4"
                markerWidth="6"
                markerHeight="6"
              >
                <circle className={styles.dot} cx="4" cy="4" r="3" />
              </marker>
            </defs>
            <path
              className={styles.link}
              d="M60 82V132H110"
              markerEnd="url(#jtd-dot)"
            />
            <path
              className={styles.link}
              d="M300 82V156H250"
              markerEnd="url(#jtd-dot)"
            />
            <Table
              x={4}
              y={10}
              width={112}
              name="students"
              rows={[
                ["id", "PK"],
                ["name", null],
              ]}
            />
            <Table
              x={244}
              y={10}
              width={112}
              name="courses"
              rows={[
                ["code", "PK"],
                ["title", null],
              ]}
            />
            <Table
              accent
              x={110}
              y={96}
              width={140}
              name="enrollments"
              rows={[
                ["student_id", "FK"],
                ["course_code", "FK"],
                ["grade", null],
                ["role", null],
              ]}
            />
          </svg>
          <div className={styles.note}>
            Three tables, two foreign keys, two indexes, two joins per query.
          </div>
        </div>

        <div className={styles.panel}>
          <div className={styles.eyebrow}>Graph</div>
          <svg
            viewBox="0 0 360 200"
            role="img"
            aria-label="Two nodes, Student and Course, joined by one ENROLLED_IN relationship that carries grade and role."
          >
            <defs>
              <marker
                id="jtd-arrow"
                viewBox="0 0 10 10"
                refX="9"
                refY="5"
                markerWidth="8"
                markerHeight="8"
                orient="auto"
              >
                <path className={styles.arrowHead} d="M0 0L10 5L0 10z" />
              </marker>
            </defs>
            <path
              className={styles.edge}
              d="M116 100H243"
              markerEnd="url(#jtd-arrow)"
            />
            <rect
              className={styles.edgeLabelBox}
              x="118"
              y="58"
              width="124"
              height="26"
              rx="13"
            />
            <text
              className={styles.edgeLabel}
              x="180"
              y="75.5"
              textAnchor="middle"
            >
              ENROLLED_IN
            </text>
            <text
              className={styles.edgeProps}
              x="180"
              y="126"
              textAnchor="middle"
            >
              grade · role
            </text>
            <Node cx={70} label="Student" sub="id, name" />
            <Node cx={290} label="Course" sub="code, title" />
          </svg>
          <div className={styles.note}>
            Two node types and one relationship that carries the join columns.
          </div>
        </div>
      </div>
      <figcaption className={styles.caption}>
        The join table on the left and the relationship on the right are the
        same thing: a link between one student and one course, with data of its
        own.
      </figcaption>
    </figure>
  );
}
