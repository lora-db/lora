// Swizzled `BlogSidebar/Content` so each year group is further sub-grouped by
// month. The stock component only renders a year heading; we keep that and add
// a month sub-heading inside it so long archives are easier to scan.
import React, { memo } from "react";
import { useThemeConfig } from "@docusaurus/theme-common";
import { groupBlogSidebarItemsByYear } from "@docusaurus/plugin-content-blog/client";
import Heading from "@theme/Heading";

import styles from "./styles.module.css";

const MONTH_NAMES = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

function groupItemsByMonth(items) {
  const groups = [];
  const indexByMonth = new Map();
  for (const item of items) {
    const date = item.date instanceof Date ? item.date : new Date(item.date);
    const monthIndex = date.getUTCMonth();
    if (!indexByMonth.has(monthIndex)) {
      indexByMonth.set(monthIndex, groups.length);
      groups.push([monthIndex, []]);
    }
    groups[indexByMonth.get(monthIndex)][1].push(item);
  }
  return groups;
}

function BlogSidebarYearGroup({ year, yearGroupHeadingClassName, children }) {
  return (
    <div role="group">
      <Heading as="h3" className={yearGroupHeadingClassName}>
        {year}
      </Heading>
      {children}
    </div>
  );
}

function BlogSidebarMonthGroup({ monthLabel, count, children }) {
  return (
    <div role="group" className={styles.monthGroup}>
      <Heading as="h4" className={styles.monthGroupHeading}>
        <span className={styles.monthGroupLabel}>{monthLabel}</span>
        <span className={styles.monthGroupRule} aria-hidden="true" />
        <span className={styles.monthGroupCount}>{count}</span>
      </Heading>
      <div className={styles.monthGroupItems}>{children}</div>
    </div>
  );
}

function BlogSidebarContent({
  items,
  yearGroupHeadingClassName,
  ListComponent,
}) {
  const themeConfig = useThemeConfig();
  if (themeConfig.blog.sidebar.groupByYear) {
    const itemsByYear = groupBlogSidebarItemsByYear(items);
    return (
      <>
        {itemsByYear.map(([year, yearItems]) => (
          <BlogSidebarYearGroup
            key={year}
            year={year}
            yearGroupHeadingClassName={yearGroupHeadingClassName}
          >
            {groupItemsByMonth(yearItems).map(([monthIndex, monthItems]) => (
              <BlogSidebarMonthGroup
                key={monthIndex}
                monthLabel={MONTH_NAMES[monthIndex]}
                count={monthItems.length}
              >
                <ListComponent items={monthItems} />
              </BlogSidebarMonthGroup>
            ))}
          </BlogSidebarYearGroup>
        ))}
      </>
    );
  }
  return <ListComponent items={items} />;
}

export default memo(BlogSidebarContent);
