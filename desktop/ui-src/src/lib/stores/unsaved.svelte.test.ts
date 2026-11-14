import { afterEach, describe, expect, it, vi } from "vitest";
import { answerDiscard, confirmDiscard, hasUnsaved, markSaved, markUnsaved, unsaved, whenDiscarded } from "./unsaved.svelte";

afterEach(() => {
  markSaved("a");
  markSaved("b");
  answerDiscard(false);
});

describe("unsaved guard", () => {
  it("runs at once, synchronously, when nothing is unsaved", () => {
    const fn = vi.fn();
    whenDiscarded(fn);
    expect(fn).toHaveBeenCalledOnce();
    expect(unsaved.asking).toBe(false);
  });

  it("asks first, and runs after a discard", async () => {
    const dropped = vi.fn();
    markUnsaved("a", dropped);
    const fn = vi.fn();
    whenDiscarded(fn);
    expect(unsaved.asking).toBe(true);
    expect(fn).not.toHaveBeenCalled();

    answerDiscard(true);
    await Promise.resolve();
    expect(fn).toHaveBeenCalledOnce();
    expect(dropped).toHaveBeenCalledOnce();
    expect(hasUnsaved()).toBe(false);
    expect(unsaved.asking).toBe(false);
  });

  it("does nothing when the user keeps editing", async () => {
    const dropped = vi.fn();
    markUnsaved("a", dropped);
    const answer = confirmDiscard();
    answerDiscard(false);
    expect(await answer).toBe(false);
    expect(dropped).not.toHaveBeenCalled();
    expect(hasUnsaved()).toBe(true);
  });

  it("forgets owners that saved", async () => {
    markUnsaved("a", () => {});
    markUnsaved("b", () => {});
    markSaved("a");
    markSaved("b");
    expect(await confirmDiscard()).toBe(true);
    expect(unsaved.asking).toBe(false);
  });
});
