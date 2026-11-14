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
  }

  let { pod, container }: Props = $props();

  let host: HTMLDivElement;
  let term: Terminal | null = null;
  let fit: FitAddon | null = null;
  let session: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlisteners: (() => void)[] = [];
  let destroyed = false;

  function sendResize() {
    if (!term || !fit || !session) return;
    fit.fit();
    commands.cmdExecResize(session, term.cols, term.rows).catch(() => {});
  }

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
    // The pod panel is long: bring the terminal into view (xterm also stops
    // rendering while it is off screen).
    host.scrollIntoView?.({ block: "nearest" });
    term = t;
    fit = f;

    resizeObserver = new ResizeObserver(() => sendResize());
    resizeObserver.observe(host);

    // Output can arrive before cmd_exec_start resolves and `session` is set,
    // so while a terminal is open without a session yet, accept output for it
    // as well.
    const un = await Promise.all([
      events.execOutput.listen((e) => {
        const { session: id, data } = e.payload;
        if (term && (id === session || !session)) term.write(data);
      }),
      events.execClosed.listen((e) => {
        if (term && (e.payload === session || !session)) {
          // Keep `session`: a failure message can arrive just after the close,
          // and the output handler drops messages for a session it no longer
          // knows.
          term.write("\r\n\x1b[90m[session closed]\x1b[0m\r\n");
        }
      }),
    ]);
    if (destroyed) {
      un.forEach((f) => f());
      return;
    }
    unlisteners = un;

    try {
      const id = await commands.cmdExecStart(pod.namespace, pod.name, container, t.cols, t.rows);
      if (destroyed) {
        commands.cmdExecClose(id).catch(() => {});
        return;
      }
      session = id;
      t.onData((data) => {
        commands.cmdExecInput(id, data).catch(() => {});
      });
      t.focus();
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
    height: 240px;
    padding: 8px 12px;
    background: #0d1013;
    border-top: 1px solid var(--border);
    overflow: hidden;
  }
</style>
