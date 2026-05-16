// Hover tooltip — positions an absolute-positioned div near the
// pointer over a host element. Content can be a string (HTML) or an
// HTMLElement; `null`/empty content hides the tooltip.
//
// LORA: internalised from `float-tooltip` (MIT, © Vasco Asturiano).
// The upstream additionally supports React/Preact-renderable content
// via `preact`; we only use the string/HTMLElement paths, so the
// preact dependency is dropped. Uses native pointer events instead
// of `d3-selection` to avoid the runtime dep.

import "./float-tooltip.css";

export type TooltipContent = string | HTMLElement | false | null;

export interface TooltipOptions {
  style?: Partial<CSSStyleDeclaration>;
}

export default class Tooltip {
  #tooltipEl: HTMLDivElement;
  #content: TooltipContent = false;
  #offsetX: number | string | null = null;
  #offsetY: number | string | null = null;
  #mouseInside = false;

  constructor(host: HTMLElement, options: TooltipOptions = {}) {
    // The host needs to be a positioned ancestor so the absolute
    // tooltip anchors against it (CSS rule: an absolute element is
    // positioned relative to the nearest positioned ancestor).
    if (getComputedStyle(host).position === "static") {
      host.style.position = "relative";
    }

    this.#tooltipEl = document.createElement("div");
    this.#tooltipEl.className = "float-tooltip-kap";
    // Default to off-screen + hidden so the first paint doesn't
    // flash anywhere unexpected.
    this.#tooltipEl.style.left = "-10000px";
    this.#tooltipEl.style.display = "none";
    if (options.style) {
      for (const [k, v] of Object.entries(options.style)) {
        if (v !== undefined) {
          (this.#tooltipEl.style as unknown as Record<string, string>)[k] =
            String(v);
        }
      }
    }
    host.appendChild(this.#tooltipEl);

    // We use pointermove (rather than mousemove) so touch hovers
    // also drive the tooltip on devices that simulate pointer
    // events. The earlier d3-selection-based implementation used a
    // unique namespaced event so unrelated handlers wouldn't clash;
    // since we own the element here, simple .addEventListener is
    // enough.
    host.addEventListener("pointermove", (ev) => {
      this.#mouseInside = true;
      const rect = host.getBoundingClientRect();
      const x = ev.clientX - rect.left;
      const y = ev.clientY - rect.top;
      const canvasWidth = host.offsetWidth;
      const canvasHeight = host.offsetHeight;

      // Horizontal: when no offset is set, scale the translate
      // toward the cursor side so the bubble doesn't run off the
      // edge of the canvas. Numeric offsets shift around centred.
      const tx =
        this.#offsetX === null || this.#offsetX === undefined
          ? `-${(x / canvasWidth) * 100}%`
          : typeof this.#offsetX === "number"
            ? `calc(-50% + ${this.#offsetX}px)`
            : this.#offsetX;

      // Vertical: auto flips above the cursor if it's near the
      // bottom edge; otherwise sits below.
      const ty =
        this.#offsetY === null || this.#offsetY === undefined
          ? canvasHeight > 130 && canvasHeight - y < 100
            ? "calc(-100% - 6px)"
            : "21px"
          : typeof this.#offsetY === "number"
            ? this.#offsetY < 0
              ? `calc(-100% - ${Math.abs(this.#offsetY)}px)`
              : `${this.#offsetY}px`
            : this.#offsetY;

      this.#tooltipEl.style.left = `${x}px`;
      this.#tooltipEl.style.top = `${y}px`;
      this.#tooltipEl.style.transform = `translate(${tx},${ty})`;
      if (this.#content) this.#tooltipEl.style.display = "inline";
    });

    host.addEventListener("pointerover", () => {
      this.#mouseInside = true;
      if (this.#content) this.#tooltipEl.style.display = "inline";
    });
    host.addEventListener("pointerout", () => {
      this.#mouseInside = false;
      this.#tooltipEl.style.display = "none";
    });
  }

  content(): TooltipContent;
  content(value: TooltipContent): this;
  content(value?: TooltipContent): TooltipContent | this {
    if (arguments.length === 0) return this.#content;
    this.#content = value ?? false;
    this.#render();
    return this;
  }

  offsetX(): number | string | null;
  offsetX(value: number | string | null): this;
  offsetX(value?: number | string | null): number | string | null | this {
    if (arguments.length === 0) return this.#offsetX;
    this.#offsetX = value ?? null;
    return this;
  }

  offsetY(): number | string | null;
  offsetY(value: number | string | null): this;
  offsetY(value?: number | string | null): number | string | null | this {
    if (arguments.length === 0) return this.#offsetY;
    this.#offsetY = value ?? null;
    return this;
  }

  #render(): void {
    const visible = !!this.#content && this.#mouseInside;
    this.#tooltipEl.style.display = visible ? "inline" : "none";
    if (!this.#content) {
      this.#tooltipEl.textContent = "";
      return;
    }
    if (this.#content instanceof HTMLElement) {
      this.#tooltipEl.textContent = "";
      this.#tooltipEl.appendChild(this.#content);
    } else if (typeof this.#content === "string") {
      this.#tooltipEl.innerHTML = this.#content;
    } else {
      this.#tooltipEl.style.display = "none";
      // eslint-disable-next-line no-console
      console.warn("Tooltip content is invalid, skipping.", this.#content);
    }
  }
}
