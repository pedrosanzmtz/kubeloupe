// "+ Create": the dialog that creates objects from YAML.

import { templateFor } from "../templates";
import { app, currentKind } from "./app.svelte";

export const createUi = $state({
  open: false,
  /** The YAML the editor starts with (a template for the current kind). */
  initial: "",
  /** The namespace objects without one go to ("" = "default"). */
  namespace: "",
});

/** Open the dialog with a template for the current kind and namespace. */
export function openCreate() {
  // All namespaces ("") means "default" (the backend does the same).
  createUi.initial = templateFor(currentKind()?.resource ?? null, app.namespace);
  createUi.namespace = app.namespace;
  createUi.open = true;
}

export function closeCreate() {
  createUi.open = false;
}
