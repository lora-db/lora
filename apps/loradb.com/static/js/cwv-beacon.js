/*
 * Core Web Vitals beacon — reports LCP, INP, and CLS to Umami as
 * custom events so the same dashboard that tracks pageviews also
 * trends performance per route.
 *
 * Pure inline PerformanceObserver code; no library dependency. The
 * shape of the data is what Google ships in `web-vitals`:
 *
 *   umami.track('cwv-lcp', { value: 1234, rating: 'good',  navType: 'navigate' })
 *   umami.track('cwv-inp', { value:   97, rating: 'good',  navType: 'navigate' })
 *   umami.track('cwv-cls', { value: 0.04, rating: 'good',  navType: 'navigate' })
 *
 * Thresholds match Google's published bands so the `rating` value is
 * directly comparable to CrUX / PageSpeed Insights:
 *   LCP: good ≤ 2500ms,  poor > 4000ms
 *   INP: good ≤ 200ms,   poor > 500ms
 *   CLS: good ≤ 0.1,     poor > 0.25
 *
 * The beacon only fires once per metric per page (final value at
 * page-hidden), so volume stays low. Skips environments where
 * PerformanceObserver, document, or window.umami is missing.
 */
(function () {
  if (typeof PerformanceObserver === "undefined" || typeof document === "undefined") return;

  var navType =
    (performance.getEntriesByType && performance.getEntriesByType("navigation")[0]?.type) ||
    "navigate";

  function rate(metric, v) {
    if (metric === "lcp") return v <= 2500 ? "good" : v <= 4000 ? "needs-improvement" : "poor";
    if (metric === "inp") return v <= 200 ? "good" : v <= 500 ? "needs-improvement" : "poor";
    if (metric === "cls") return v <= 0.1 ? "good" : v <= 0.25 ? "needs-improvement" : "poor";
    return "unknown";
  }

  function send(metric, value) {
    // Umami may not be loaded yet during early-load metrics. Queue
    // and drain on first availability; after that, fire directly.
    var payload = { value: Math.round(value * 1000) / 1000, rating: rate(metric, value), navType: navType };
    var fn = function () {
      try {
        if (window.umami && typeof window.umami.track === "function") {
          window.umami.track("cwv-" + metric, payload);
        }
      } catch (_) {}
    };
    if (window.umami) fn();
    else {
      var t0 = Date.now();
      var iv = setInterval(function () {
        if (window.umami || Date.now() - t0 > 10000) {
          clearInterval(iv);
          fn();
        }
      }, 250);
    }
  }

  // LCP — final value at page-hidden (visibilitychange).
  try {
    var lastLcp = 0;
    var lcpObs = new PerformanceObserver(function (list) {
      var entries = list.getEntries();
      var last = entries[entries.length - 1];
      lastLcp = last.renderTime || last.loadTime || last.startTime || 0;
    });
    lcpObs.observe({ type: "largest-contentful-paint", buffered: true });
    var lcpReported = false;
    var reportLcp = function () {
      if (lcpReported || !lastLcp) return;
      lcpReported = true;
      try { lcpObs.disconnect(); } catch (_) {}
      send("lcp", lastLcp);
    };
    addEventListener("visibilitychange", function () {
      if (document.visibilityState === "hidden") reportLcp();
    });
    addEventListener("pagehide", reportLcp);
  } catch (_) {}

  // CLS — sum of layout-shift values not preceded by user input.
  try {
    var cls = 0;
    var clsObs = new PerformanceObserver(function (list) {
      list.getEntries().forEach(function (e) {
        if (!e.hadRecentInput) cls += e.value;
      });
    });
    clsObs.observe({ type: "layout-shift", buffered: true });
    var clsReported = false;
    var reportCls = function () {
      if (clsReported) return;
      clsReported = true;
      try { clsObs.disconnect(); } catch (_) {}
      send("cls", cls);
    };
    addEventListener("visibilitychange", function () {
      if (document.visibilityState === "hidden") reportCls();
    });
    addEventListener("pagehide", reportCls);
  } catch (_) {}

  // INP — worst interaction-to-next-paint over the page lifetime.
  // `event` entries with duration are the right signal in modern Chromium.
  try {
    var worstInp = 0;
    var inpObs = new PerformanceObserver(function (list) {
      list.getEntries().forEach(function (e) {
        if (e.interactionId && e.duration > worstInp) worstInp = e.duration;
      });
    });
    // durationThreshold tracks anything ≥16ms — keeps observer noise low.
    inpObs.observe({ type: "event", buffered: true, durationThreshold: 16 });
    var inpReported = false;
    var reportInp = function () {
      if (inpReported || !worstInp) return;
      inpReported = true;
      try { inpObs.disconnect(); } catch (_) {}
      send("inp", worstInp);
    };
    addEventListener("visibilitychange", function () {
      if (document.visibilityState === "hidden") reportInp();
    });
    addEventListener("pagehide", reportInp);
  } catch (_) {}
})();
