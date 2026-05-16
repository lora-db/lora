// Minimal tween primitive — interpolates an object's numeric fields
// from their starting values to a target over a duration, calling
// `onUpdate` each frame with the current state. Callers must drive
// `Group.update()` from their own animation loop (we don't poll
// requestAnimationFrame internally, matching the upstream API).
//
// LORA: replaces `@tweenjs/tween.js` (MIT). Upstream covers the full
// tween menagerie (chaining, repeat/yoyo, dozens of easings, group
// scheduling). The kapsule shell only uses `Tween().to().easing().
// onUpdate().onComplete().start()` plus `Group.add/remove/update`
// and `Easing.Quadratic.Out`, so this slim replacement is enough.

type Numbers = Record<string, number>;
type EasingFn = (k: number) => number;

export const Easing = {
  Linear: { None: (k: number): number => k },
  Quadratic: {
    In: (k: number): number => k * k,
    Out: (k: number): number => k * (2 - k),
    InOut: (k: number): number =>
      (k *= 2) < 1 ? 0.5 * k * k : -0.5 * (--k * (k - 2) - 1),
  },
} as const;

export class Group {
  // Stored as Tween<Numbers> internally but accepts any Tween subtype
  // — the upstream library doesn't enforce homogeneity either, and
  // we lose nothing by relaxing.
  #tweens = new Set<Tween<Numbers>>();
  add<T extends Numbers>(t: Tween<T>): void {
    this.#tweens.add(t as unknown as Tween<Numbers>);
  }
  remove<T extends Numbers>(t: Tween<T>): void {
    this.#tweens.delete(t as unknown as Tween<Numbers>);
  }
  /** Step every active tween. Should be called once per animation
   *  frame; finished tweens self-remove via their `onComplete`. */
  update(time: number = performance.now()): void {
    for (const t of this.#tweens) t.update(time);
  }
}

export class Tween<T extends Numbers> {
  #from: T;
  #to: Partial<T> = {};
  #duration = 1000;
  #easing: EasingFn = Easing.Linear.None;
  #onUpdate: ((obj: T) => void) | null = null;
  #onComplete: ((this: Tween<T>, obj: T) => void) | null = null;
  #startTime = 0;
  #running = false;

  constructor(initial: T) {
    // Clone — the caller often hands us a transient object that
    // they keep mutating. Cloning here means our baseline is stable
    // for the lifetime of the tween.
    this.#from = { ...initial };
  }

  to(target: Partial<T>, duration: number): this {
    this.#to = target;
    this.#duration = duration;
    return this;
  }

  easing(fn: EasingFn): this {
    this.#easing = fn;
    return this;
  }

  onUpdate(cb: (obj: T) => void): this {
    this.#onUpdate = cb;
    return this;
  }

  onComplete(cb: (this: Tween<T>, obj: T) => void): this {
    this.#onComplete = cb;
    return this;
  }

  start(time: number = performance.now()): this {
    this.#startTime = time;
    this.#running = true;
    return this;
  }

  update(time: number): boolean {
    if (!this.#running) return false;
    const elapsed = time - this.#startTime;
    const t = Math.min(1, elapsed / this.#duration);
    const k = this.#easing(t);

    const cur = { ...this.#from };
    for (const key in this.#to) {
      const start = this.#from[key] as number;
      const end = this.#to[key] as number;
      (cur as Numbers)[key] = start + (end - start) * k;
    }
    this.#onUpdate?.(cur);

    if (t >= 1) {
      this.#running = false;
      this.#onComplete?.call(this, cur);
    }
    return this.#running;
  }
}
