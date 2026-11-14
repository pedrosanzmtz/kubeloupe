import { openUrl } from "@tauri-apps/plugin-opener";
import { commands } from "../bindings";
import type { ForwardInfo, ForwardTarget } from "../bindings";
import { forwardLabel, isLocalUrl, parseLocalPort, parseRemotePort } from "../forwards";
import { setNotice } from "./actions.svelte";

/**
 * Port-forwards run in the backend, independent of any panel: closing a
 * panel does not stop them. This store mirrors the backend's list, polled
 * while something that shows it is mounted.
 */
class Forwards {
  /** Replaced on every refresh. */
  list = $state.raw<ForwardInfo[]>([]);
}
export const forwards = new Forwards();

export async function refreshForwards() {
  try {
    forwards.list = await commands.cmdPortforwardList();
  } catch {
    // The list is local to the backend; keep the last one.
  }
}

const POLL_MS = 1000;
let pollers = 0;
let timer: ReturnType<typeof setInterval> | null = null;

/** Keep the list fresh while the caller is mounted; returns the cleanup. */
export function pollForwards(): () => void {
  pollers += 1;
  if (!timer) timer = setInterval(() => void refreshForwards(), POLL_MS);
  void refreshForwards();
  return () => {
    pollers -= 1;
    if (pollers === 0 && timer) {
      clearInterval(timer);
      timer = null;
    }
  };
}

/** Start a forward (or get the one already running). Throws the backend's message. */
export async function startForward(target: ForwardTarget, remote: string, local: number | null): Promise<ForwardInfo> {
  const f = await commands.cmdPortforwardStart(target, remote, local);
  await refreshForwards();
  return f;
}

export async function stopForward(id: string) {
  try {
    await commands.cmdPortforwardStop(id);
  } finally {
    await refreshForwards();
  }
}

export async function openForward(f: ForwardInfo) {
  if (!isLocalUrl(f.url)) {
    setNotice(false, `Not opening ${f.url}: only local URLs can be opened.`);
    return;
  }
  try {
    await openUrl(f.url);
  } catch (e) {
    setNotice(false, `Could not open ${f.url}: ${e}`);
  }
}

export async function copyForwardUrl(f: ForwardInfo) {
  try {
    await navigator.clipboard.writeText(f.url);
    setNotice(true, `Copied ${f.url}`);
  } catch {
    setNotice(false, `Could not copy to the clipboard. The URL is ${f.url}`);
  }
}

/** The "Port forward…" dialog. */
export const forwardDialog = $state({
  target: null as ForwardTarget | null,
  remote: "",
  local: "",
  busy: false,
  error: "",
});

export function openForwardDialog(target: ForwardTarget, remote = "") {
  forwardDialog.target = target;
  forwardDialog.remote = remote;
  forwardDialog.local = "";
  forwardDialog.busy = false;
  forwardDialog.error = "";
}

export function closeForwardDialog() {
  forwardDialog.target = null;
}

export async function confirmForwardDialog() {
  const target = forwardDialog.target;
  if (!target || forwardDialog.busy) return;
  const remote = parseRemotePort(forwardDialog.remote);
  const local = parseLocalPort(forwardDialog.local);
  if (!remote.ok || !local.ok) {
    forwardDialog.error = !remote.ok ? remote.error : !local.ok ? local.error : "";
    return;
  }
  forwardDialog.busy = true;
  forwardDialog.error = "";
  try {
    const f = await startForward(target, remote.value, local.value);
    forwardDialog.target = null;
    setNotice(true, `Forwarding ${f.url} → ${forwardLabel(target)}:${f.remote}`);
  } catch (e) {
    forwardDialog.error = String(e);
  } finally {
    forwardDialog.busy = false;
  }
}
