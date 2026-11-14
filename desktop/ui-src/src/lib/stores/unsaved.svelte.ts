// Unsaved-changes guard. An editor with edits the user has not saved
// registers here; whatever would drop them (closing the panel, opening
// another object, switching view or context) goes through `whenDiscarded`
// or `confirmDiscard`, which ask "Discard changes?" first.

/** Shown by `UnsavedDialog` while a discard waits for an answer. */
export const unsaved = $state({ asking: false });

/** Owner id → what to do when the user discards its edits. */
const owners = new Map<string, () => void>();
let waiting: ((discard: boolean) => void)[] = [];

/** `id` has unsaved edits; `onDiscard` runs if the user discards them. */
export function markUnsaved(id: string, onDiscard: () => void) {
  owners.set(id, onDiscard);
}

/** `id` has nothing unsaved (any more). */
export function markSaved(id: string) {
  owners.delete(id);
}

export function hasUnsaved(): boolean {
  return owners.size > 0;
}

/**
 * Resolves `true` right away if nothing is unsaved, otherwise asks the user
 * and resolves with their answer. On `true` every owner's `onDiscard` has run.
 */
export function confirmDiscard(): Promise<boolean> {
  if (owners.size === 0) return Promise.resolve(true);
  unsaved.asking = true;
  return new Promise((resolve) => waiting.push(resolve));
}

/** The user's answer to the dialog. */
export function answerDiscard(discard: boolean) {
  unsaved.asking = false;
  if (discard) {
    const discards = [...owners.values()];
    owners.clear();
    for (const fn of discards) fn();
  }
  const resolve = waiting;
  waiting = [];
  for (const r of resolve) r(discard);
}

/**
 * Run `fn` now if nothing is unsaved (synchronously, so callers behave as
 * before), or once the user agrees to discard. Never if they keep editing.
 */
export function whenDiscarded(fn: () => void): void {
  if (owners.size === 0) {
    fn();
    return;
  }
  void confirmDiscard().then((ok) => {
    if (ok) fn();
  });
}
