// Usage history (#47): geometry for the table sparklines and the detail
// panel charts. Plain SVG, no chart library.

import type { UsageReference, UsageSample } from "./bindings";

/**
 * `points` for an SVG polyline of `values` in a `w`×`h` box: left to right,
 * 0 at the bottom, `max` (default: the largest value) at the top. A single
 * value is drawn flat across the box. Empty for no values.
 */
export function sparkPoints(values: readonly number[], w: number, h: number, max?: number): string {
  if (values.length === 0) return "";
  const top = max ?? Math.max(...values);
  const y = (v: number) => round(top > 0 ? h - (Math.min(v, top) / top) * h : h);
  if (values.length === 1) return `0,${y(values[0])} ${w},${y(values[0])}`;
  const step = w / (values.length - 1);
  return values.map((v, i) => `${round(i * step)},${y(v)}`).join(" ");
}

const round = (n: number) => Math.round(n * 10) / 10;

/**
 * A "nice" top for a chart axis: at least `value` (and 1), rounded up to
 * 1, 2 or 5 times a power of ten in the chart's own unit.
 */
export function niceMax(value: number): number {
  if (!(value > 0)) return 1;
  const pow = 10 ** Math.floor(Math.log10(value));
  for (const m of [1, 2, 5, 10]) if (m * pow >= value) return m * pow;
  return 10 * pow;
}

/** For memory, a top that formats nicely in binary units ("256Mi"). */
export function niceMaxBytes(value: number): number {
  if (!(value > 0)) return 1024 * 1024;
  let unit = 1;
  while (value / unit >= 1024 && unit < 1024 ** 5) unit *= 1024;
  for (const m of [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024]) if (m * unit >= value) return m * unit;
  return 1024 * unit;
}

export type Resource = "cpu" | "memory";

/** A reference line: a request, a limit or allocatable. */
export interface RefLine {
  label: string;
  value: number;
}

/** The reference lines of one resource that are set (0 = not set). */
export function refLines(ref: UsageReference, r: Resource): RefLine[] {
  const lines: RefLine[] =
    r === "cpu"
      ? [
          { label: "request", value: ref.cpu_request },
          { label: "limit", value: ref.cpu_limit },
          { label: "allocatable", value: ref.cpu_allocatable },
        ]
      : [
          { label: "request", value: ref.memory_request },
          { label: "limit", value: ref.memory_limit },
          { label: "allocatable", value: ref.memory_allocatable },
        ];
  return lines.filter((l) => l.value > 0);
}

export const valueOf = (s: UsageSample, r: Resource) => (r === "cpu" ? s.cpu_millicores : s.memory_bytes);

/** A line on a chart. */
export interface ChartLine {
  name: string;
  samples: readonly UsageSample[];
}

/** Box and scales of one chart. */
export interface ChartFrame {
  w: number;
  h: number;
  t0: number;
  t1: number;
  max: number;
}

/**
 * Frame the lines and the reference lines: time from the first to the last
 * sample (at least a minute, so two samples are not a full-width jump), and
 * a value axis that fits both the usage and every reference line.
 */
export function frameFor(
  lines: readonly ChartLine[],
  refs: readonly RefLine[],
  r: Resource,
  w: number,
  h: number,
): ChartFrame {
  let t0 = Infinity;
  let t1 = -Infinity;
  let top = 0;
  for (const l of lines) {
    for (const s of l.samples) {
      t0 = Math.min(t0, s.t);
      t1 = Math.max(t1, s.t);
      top = Math.max(top, valueOf(s, r));
    }
  }
  for (const ref of refs) top = Math.max(top, ref.value);
  if (!Number.isFinite(t0)) t0 = t1 = 0;
  t0 = Math.min(t0, t1 - 60_000);
  // 10% headroom so a line at the top (often a request) stays clear of the label.
  top *= 1.1;
  return { w, h, t0, t1, max: r === "cpu" ? niceMax(top) : niceMaxBytes(top) };
}

/** Y of `value` in `frame`. */
export function yOf(frame: ChartFrame, value: number): number {
  return round(frame.h - (Math.min(value, frame.max) / frame.max) * frame.h);
}

/** `points` of one line in `frame`. */
export function linePoints(frame: ChartFrame, samples: readonly UsageSample[], r: Resource): string {
  const span = frame.t1 - frame.t0 || 1;
  return samples.map((s) => `${round(((s.t - frame.t0) / span) * frame.w)},${yOf(frame, valueOf(s, r))}`).join(" ");
}

/** "10:42", local time, for "collecting since". */
export function clockTime(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}
