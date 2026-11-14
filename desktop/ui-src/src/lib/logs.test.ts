import { describe, expect, it } from "vitest";
import { LOG_BUFFER_MAX, LogBuffer, POD_COLORS, linePrefix, logMatches, podColor, splitHighlights } from "./logs";

describe("LogBuffer", () => {
  it("appends untagged lines with tag 0", () => {
    const b = new LogBuffer();
    b.push(["a", "b"]);
    expect(b.lines).toEqual(["a", "b"]);
    expect(b.tagIds).toEqual([0, 0]);
  });

  it("interns pod/container tags", () => {
    const b = new LogBuffer();
    b.pushTagged([
      { pod: "p1", container: "c", line: "x" },
      { pod: "p2", container: "c", line: "y" },
      { pod: "p1", container: "c", line: "z" },
    ]);
    expect(b.tagIds).toEqual([1, 2, 1]);
    expect(b.tags[1]).toEqual({ pod: "p1", container: "c" });
    expect(b.tags[2]).toEqual({ pod: "p2", container: "c" });
  });

  it("keeps the cap, trimming in chunks once 10% over", () => {
    const b = new LogBuffer();
    const fill = (n: number, from: number) => Array.from({ length: n }, (_, i) => String(from + i));
    b.push(fill(LOG_BUFFER_MAX, 0));
    b.push(fill(LOG_BUFFER_MAX / 10, LOG_BUFFER_MAX));
    // Exactly at the threshold: nothing dropped yet.
    expect(b.lines).toHaveLength(LOG_BUFFER_MAX + LOG_BUFFER_MAX / 10);
    b.push(["last"]);
    // Over it: back down to the cap, oldest lines gone, tags kept aligned.
    expect(b.lines).toHaveLength(LOG_BUFFER_MAX);
    expect(b.tagIds).toHaveLength(LOG_BUFFER_MAX);
    expect(b.lines[b.lines.length - 1]).toBe("last");
    expect(b.lines[0]).toBe(String(LOG_BUFFER_MAX / 10 + 1));
  });

  it("clear empties the lines but keeps known tags", () => {
    const b = new LogBuffer();
    b.pushTagged([{ pod: "p", container: "c", line: "x" }]);
    b.clear();
    expect(b.lines).toEqual([]);
    expect(b.tagIds).toEqual([]);
    b.pushTagged([{ pod: "p", container: "c", line: "y" }]);
    expect(b.tagIds).toEqual([1]);
  });
});

describe("logMatches", () => {
  it("matches everything with an empty filter", () => {
    expect(logMatches("anything", "")).toBe(true);
  });
  it("is a case-insensitive substring match against a lowercased filter", () => {
    expect(logMatches("GET /Health 200", "health")).toBe(true);
    expect(logMatches("GET /Health 200", "error")).toBe(false);
  });
});

describe("splitHighlights", () => {
  it("returns the whole text unhighlighted without a filter", () => {
    expect(splitHighlights("abc", "")).toEqual([{ text: "abc", hit: false }]);
  });

  it("marks every case-insensitive match and keeps the original case", () => {
    expect(splitHighlights("Error: error", "ERROR")).toEqual([
      { text: "Error", hit: true },
      { text: ": ", hit: false },
      { text: "error", hit: true },
    ]);
  });

  it("joins back to the original text", () => {
    const text = "aaXaaXa";
    const segs = splitHighlights(text, "x");
    expect(segs.map((s) => s.text).join("")).toBe(text);
    expect(segs.filter((s) => s.hit)).toHaveLength(2);
  });

  it("returns no segments for an empty text with a filter", () => {
    expect(splitHighlights("", "x")).toEqual([]);
  });
});

describe("podColor / linePrefix", () => {
  it("is stable and from the palette", () => {
    expect(podColor("web-1")).toBe(podColor("web-1"));
    expect(POD_COLORS).toContain(podColor("web-1"));
  });

  it("matches the CLI's FNV-1a choice", () => {
    // FNV-1a("") = 0x811c9dc5 = 2166136261; 2166136261 % 12 = 1.
    expect(podColor("")).toBe(POD_COLORS[2166136261 % POD_COLORS.length]);
  });

  it("adds the container only when asked", () => {
    expect(linePrefix({ pod: "p", container: "c" }, false)).toBe("[p]");
    expect(linePrefix({ pod: "p", container: "c" }, true)).toBe("[p/c]");
  });
});
