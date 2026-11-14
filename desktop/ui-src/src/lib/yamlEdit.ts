// Small, dependency-free helpers for the YAML editor: find the line of a
// field path in block-style YAML (what the backend emits), read the
// resourceVersion, and point at the line an API server error names.

/** A field path: map keys and list indices, e.g. `["spec", "containers", 0, "image"]`. */
export type FieldPath = (string | number)[];

interface Entry {
  /** 1-based line number. */
  line: number;
  indent: number;
  /** The line without its indentation. */
  text: string;
}

function entries(yaml: string): Entry[] {
  return yaml
    .split("\n")
    .map((raw, i) => {
      const text = raw.trim();
      return { line: i + 1, indent: raw.length - raw.trimStart().length, text };
    })
    .filter((e) => e.text && !e.text.startsWith("#") && e.text !== "---");
}

const KEY = /^(?:"([^"]*)"|'([^']*)'|([^\s"'#:][^:]*?))\s*:(?:\s|$)/;

/** The key of a `key: value` line, if it is one. */
function keyOf(text: string): string | null {
  const m = KEY.exec(text);
  return m ? (m[1] ?? m[2] ?? m[3]) : null;
}

function isItem(text: string): boolean {
  return text === "-" || text.startsWith("- ");
}

/**
 * The 1-based line of `path` in `yaml`, or of the deepest part of it that
 * exists (so an unknown field points at its parent). `null` if not even the
 * first key is there. Handles block mappings and sequences, including the
 * unindented `key:\n- item` style.
 */
export function lineOfPath(yaml: string, path: FieldPath): number | null {
  const es = entries(yaml);
  let lo = 0;
  let hi = es.length;
  /** Indent of the keys of the mapping being searched. */
  let block = es.length ? es[0].indent : 0;
  let line: number | null = null;
  for (const seg of path) {
    if (lo >= hi) return line;
    if (typeof seg === "number") {
      const first = es[lo];
      if (!isItem(first.text)) return line;
      const s = first.indent;
      let n = -1;
      let start = -1;
      for (let j = lo; j < hi; j++) {
        const e = es[j];
        if (e.indent < s) break;
        if (e.indent === s && isItem(e.text) && ++n === seg) {
          start = j;
          break;
        }
      }
      if (start < 0) return line;
      let end = start + 1;
      while (end < hi && es[end].indent > s) end++;
      line = es[start].line;
      lo = start;
      hi = end;
      block = s + 2;
    } else {
      let found = -1;
      for (let j = lo; j < hi; j++) {
        const e = es[j];
        // The first key of a list item shares the line with its dash.
        const own =
          e.indent === block ? e.text : e.indent + 2 === block && e.text.startsWith("- ") ? e.text.slice(2) : null;
        if (own !== null && keyOf(own) === seg) {
          found = j;
          break;
        }
      }
      if (found < 0) return line;
      const ki = block;
      let end = found + 1;
      // Children: deeper lines, plus list items at the key's own indent.
      while (end < hi && (es[end].indent > ki || (es[end].indent === ki && isItem(es[end].text)))) end++;
      line = es[found].line;
      lo = found + 1;
      hi = end;
      block = lo < hi ? es[lo].indent : ki + 2;
    }
  }
  return line;
}

/** `metadata.resourceVersion` of a YAML object, or "" if it has none. */
export function resourceVersionOf(yaml: string): string {
  const line = lineOfPath(yaml, ["metadata", "resourceVersion"]);
  if (line === null) return "";
  const text = yaml.split("\n")[line - 1].trim();
  if (keyOf(text) !== "resourceVersion") return "";
  return text
    .slice(text.indexOf(":") + 1)
    .trim()
    .replace(/^(['"])(.*)\1$/, "$2");
}

/** `spec.containers[0].image` (or with a leading dot) as a path. */
export function parsePath(s: string): FieldPath {
  const out: FieldPath = [];
  for (const m of s.replace(/^\./, "").matchAll(/([^.[\]]+)|\[(\d+)\]/g)) {
    out.push(m[2] !== undefined ? Number(m[2]) : m[1]);
  }
  return out;
}

/**
 * Field paths an API server error names, in order. Validation errors look
 * like `spec.replicas: Invalid value: -1: ...`, schema errors like
 * `.spec.foo: field not declared in schema`, decoding errors like
 * `unknown field "spec.foo"`.
 */
export function pathsIn(message: string): FieldPath[] {
  // A leading dot marks a path even with one segment (`.spec: ...`);
  // without it, at least two segments, so plain words don't count.
  const seg = String.raw`[A-Za-z_$][\w$-]*`;
  const more = String.raw`(?:\.${seg}|\[\d+\])`;
  const re = new RegExp(String.raw`(?:^|[\s,("])(?:\.(${seg}${more}*)|(${seg}${more}+))(?=:\s|:$|")`, "g");
  return [...message.matchAll(re)].map((m) => parsePath(m[1] ?? m[2]));
}

/**
 * The line an error points at in `yaml`: a parser position (`at line 3
 * column 5`) or the first field path that can be found. `null` if neither.
 */
export function errorLine(message: string, yaml: string): number | null {
  const pos = /\bat line (\d+)/.exec(message);
  if (pos) return Number(pos[1]);
  for (const path of pathsIn(message)) {
    const line = lineOfPath(yaml, path);
    if (line !== null) return line;
  }
  return null;
}
