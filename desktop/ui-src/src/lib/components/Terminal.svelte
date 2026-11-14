<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import { commands, events } from "../bindings";
  import type { PodRef } from "../stores/detail.svelte";

  interface Props {
    pod: PodRef;
    /** Container to exec into; `null` lets the API server pick. */
    container: string | null;
    /** The shown dock tab: it gets the focus. Hidden tabs keep their session. */
    active?: boolean;
  }

  let { pod, container, active = true }: Props = $props();

  let host: HTMLDivElement;
  let term: Terminal | null = null;
  let fit: FitAddon | null = null;
  let session: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlisteners: (() => void)[] = [];
  let destroyed = false;
  /** Output that arrived before `cmd_exec_start` told us our session id,
   *  by session: other terminals are open too, so it is not all ours. */
  let early = new Map<string, string[]>();
  let closedEarly = new Set<string>();
  let lastSize = "";

  function sendResize() {
    // A hidden tab has no size; refit when it is shown again.
    if (!term || !fit || !session || host.clientHeight === 0) return;
    fit.fit();
    const size = `${term.cols}x${term.rows}`;
    if (size === lastSize) return;
    lastSize = size;
    commands.cmdExecResize(session, term.cols, term.rows).catch(() => {});
  }

  const closedNote = "\r\n\x1b[90m[session closed]\x1b[0m\r\n";

  $effect(() => {
    if (active && session) term?.focus();
  });

  onMount(async () => {
    const t = new Terminal({
      cursorBlink: true,
      fontSize: 12,
      theme: { background: "#0d1013" },
    });
    const f = new FitAddon();
    t.loadAddon(f);
    t.open(host);
    f.fit();
    term = t;
    fit = f;

    resizeObserver = new ResizeObserver(() => sendResize());
    resizeObserver.observe(host);

    // Output can arrive before cmd_exec_start resolves and `session` is set:
    // keep it by session until we know which one is ours.
    const un = await Promise.all([
      events.execOutput.listen((e) => {
        const { session: id, data } = e.payload;
        if (!term) return;
        if (id === session) term.write(data);
        else if (!session) early.set(id, [...(early.get(id) ?? []), data]);
      }),
      events.execClosed.listen((e) => {
        if (!term) return;
        // Keep `session`: a failure message can arrive just after the close,
        // and the output handler drops messages for a session it no longer
        // knows.
        if (e.payload === session) term.write(closedNote);
        else if (!session) closedEarly.add(e.payload);
      }),
    ]);
    if (destroyed) {
      un.forEach((f) => f());
      return;
    }
    unlisteners = un;

    try {
      lastSize = `${t.cols}x${t.rows}`;
      const id = await commands.cmdExecStart(pod.namespace, pod.name, container, t.cols, t.rows);
      if (destroyed) {
        commands.cmdExecClose(id).catch(() => {});
        return;
      }
      session = id;
      for (const data of early.get(id) ?? []) t.write(data);
      if (closedEarly.has(id)) t.write(closedNote);
      early = new Map();
      closedEarly = new Set();
      sendResize();
      t.onData((data) => {
        commands.cmdExecInput(id, data).catch(() => {});
      });
      if (active) t.focus();
    } catch (e) {
      t.write("\r\n\x1b[31mCould not open terminal: " + e + "\x1b[0m\r\n");
    }
  });

  onDestroy(() => {
    destroyed = true;
    resizeObserver?.disconnect();
    unlisteners.forEach((f) => f());
    if (session) commands.cmdExecClose(session).catch(() => {});
    session = null;
    term?.dispose();
    term = null;
    fit = null;
  });
</script>

<div class="terminal" data-testid="terminal" bind:this={host}></div>

<style>
  .terminal {
    flex: 1;
    min-height: 0;
    padding: 8px 12px;
    background: #0d1013;
    border-top: 1px solid var(--border);
    overflow: hidden;
  }
</style>
