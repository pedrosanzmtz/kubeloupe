/** "250m" below one core, otherwise cores ("1.5"). */
export function formatCpu(millicores: number): string {
  if (millicores >= 1000) {
    return (millicores / 1000).toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
  }
  return millicores + "m";
}

/** Binary units ("128Mi"). */
export function formatMemory(bytes: number): string {
  const units = ["B", "Ki", "Mi", "Gi", "Ti", "Pi"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  if (unit === 0) return bytes + "B";
  return value.toFixed(2).replace(/0+$/, "").replace(/\.$/, "") + units[unit];
}

/**
 * Age since `created` (unix seconds), recomputed by the UI so rows stay fresh
 * without a backend update. `fallback` is the backend's own text for objects
 * without a creation time.
 */
export function formatAge(created: number, now: number, fallback: string): string {
  if (!created) return fallback || "";
  const secs = Math.max(0, Math.floor(now / 1000 - created));
  if (secs < 60) return secs + "s";
  if (secs < 3600) return Math.floor(secs / 60) + "m";
  if (secs < 86400) return Math.floor(secs / 3600) + "h";
  return Math.floor(secs / 86400) + "d";
}
