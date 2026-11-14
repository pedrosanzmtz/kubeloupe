import { describe, expect, it } from "vitest";
import { bindingFor, formatBinding, KEYMAP } from "./keys";
import type { KeyEventLike } from "./keys";

const press = (key: string, mods: Partial<KeyEventLike> = {}): KeyEventLike => ({
  key,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});
const command = (e: KeyEventLike, mac = false) => bindingFor(e, mac)?.binding.command ?? null;

describe("bindingFor", () => {
  it("uses Ctrl elsewhere and Cmd on macOS", () => {
    expect(command(press("k", { ctrlKey: true }))).toBe("palette");
    expect(command(press("k", { metaKey: true }))).toBeNull();
    expect(command(press("k", { metaKey: true }), true)).toBe("palette");
    expect(command(press("k", { ctrlKey: true }), true)).toBeNull();
    expect(command(press("P", { ctrlKey: true }))).toBe("palette");
  });

  it("tells plain letters from their modified forms", () => {
    expect(command(press("k"))).toBe("rowUp");
    expect(command(press("j"))).toBe("rowDown");
    expect(command(press("J", { shiftKey: true }))).toBeNull();
    expect(command(press("j", { altKey: true }))).toBeNull();
  });

  it("matches punctuation typed with Shift", () => {
    expect(command(press("?", { shiftKey: true }))).toBe("help");
    expect(command(press("/"))).toBe("focusFilter");
  });

  it("needs Shift for Shift+F10 and accepts the Menu key", () => {
    expect(command(press("F10", { shiftKey: true }))).toBe("rowMenu");
    expect(command(press("F10"))).toBeNull();
    expect(command(press("ContextMenu"))).toBe("rowMenu");
  });

  it("gives the tab index for Ctrl+1…9", () => {
    const hit = bindingFor(press("3", { ctrlKey: true }), false);
    expect(hit?.binding.command).toBe("detailTab");
    expect(hit?.index).toBe(2);
    expect(command(press("3"))).toBeNull();
  });

  it("takes Cmd+Backspace as Delete on macOS only", () => {
    expect(command(press("Delete"))).toBe("deleteRows");
    expect(command(press("Backspace", { metaKey: true }), true)).toBe("deleteRows");
    expect(command(press("Backspace", { ctrlKey: true }))).toBeNull();
  });

  it("toggles the dock with Ctrl+` on every platform", () => {
    expect(command(press("`", { ctrlKey: true }))).toBe("toggleDock");
    expect(command(press("`", { ctrlKey: true }), true)).toBe("toggleDock");
    // Cmd+` switches windows on macOS.
    expect(command(press("`", { metaKey: true }), true)).toBeNull();
    expect(command(press("`"))).toBeNull();
  });

  it("opens a local terminal with Ctrl+Shift+` (the event's key is ~)", () => {
    expect(command(press("~", { ctrlKey: true, shiftKey: true }))).toBe("newLocalShell");
    expect(command(press("~", { ctrlKey: true, shiftKey: true }), true)).toBe("newLocalShell");
    expect(command(press("~", { shiftKey: true }))).toBeNull();
  });

  it("leaves Space with Shift alone", () => {
    expect(command(press(" "))).toBe("toggleRow");
    expect(command(press(" ", { shiftKey: true }))).toBeNull();
  });
});

describe("formatBinding", () => {
  const of = (c: string) => KEYMAP.find((b) => b.command === c)!;
  it("writes the keys the way the platform does", () => {
    expect(formatBinding(of("palette"), false)).toEqual(["Ctrl+K", "Ctrl+P"]);
    expect(formatBinding(of("palette"), true)).toEqual(["⌘K", "⌘P"]);
    expect(formatBinding(of("rowDown"), false)).toEqual(["↓", "j"]);
    expect(formatBinding(of("rowMenu"), false)).toEqual(["Shift+F10", "Menu"]);
    expect(formatBinding(of("detailTab"), false)).toEqual(["Ctrl+1…9"]);
    expect(formatBinding(of("deleteRows"), false)).toEqual(["Delete"]);
    expect(formatBinding(of("deleteRows"), true)).toEqual(["Delete", "⌘Backspace"]);
    expect(formatBinding(of("toggleRow"), false)).toEqual(["Space"]);
    expect(formatBinding(of("toggleDock"), false)).toEqual(["Ctrl+`"]);
    expect(formatBinding(of("toggleDock"), true)).toEqual(["⌃`"]);
  });

  it("covers every binding with a label", () => {
    for (const b of KEYMAP) {
      expect(b.label).not.toBe("");
      expect(formatBinding(b, false).length).toBeGreaterThan(0);
    }
  });
});
