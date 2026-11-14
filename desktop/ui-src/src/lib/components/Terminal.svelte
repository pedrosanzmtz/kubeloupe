<script module lang="ts">
  import { commands } from "../bindings";
  import type { PodRef } from "../stores/detail.svelte";

  /** A shell in a pod, one on this machine pointed at `namespace`, an
   *  ephemeral debug container, or a root shell on a node. */
  export type TerminalTarget =
    /** `container`: `null` lets the API server pick. */
    | { kind: "pod"; pod: PodRef; container: string | null }
    | { kind: "local"; namespace: string }
    /** Adds a container running `image` that shares `target`'s processes, then attaches. */
    | { kind: "debug"; pod: PodRef; target: string; image: string }
    /** A privileged pod on `node` in `namespace`, deleted when the session ends. */
    | { kind: "nodeShell"; node: string; namespace: string };

  /** The session commands of a target. Both kinds stream through the
   *  `exec-output` and `exec-closed` events. */
  interface Backend {
    /** The session id, and a note to show before the shell's output. */
    start(cols: number, rows: number): Promise<{ id: string; intro: string }>;
    input(id: string, data: string): Promise<unknown>;
    resize(id: string, cols: number, rows: number): Promise<unknown>;
    close(id: string): Promise<unknown>;
    /** The session id `start` runs under when the UI picks it: its output
     *  (progress while a container or pod starts) shows before `start` resolves. */
    pending?: string;
  }

  function randomId(): string {
    if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
    return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  }

  function backendFor(t: TerminalTarget): Backend {
    if (t.kind === "debug" || t.kind === "nodeShell") {
      const pending = `ui-${randomId()}`;
      return {
        pending,
        start: async (cols, rows) => ({
          id:
            t.kind === "debug"
              ? await commands.cmdDebugStart(pending, t.pod.namespace, t.pod.name, t.target, t.image, cols, rows)
              : await commands.cmdNodeShellStart(pending, t.node, t.namespace, cols, rows),
          intro: "",
        }),
        input: commands.cmdExecInput,
        resize: commands.cmdExecResize,
        close: commands.cmdExecClose,
      };
    }
    if (t.kind === "pod") {
      return {
        start: async (cols, rows) => ({
          id: await commands.cmdExecStart(t.pod.namespace, t.pod.name, t.container, cols, rows),
          intro: "",
        }),
        input: commands.cmdExecInput,
        resize: commands.cmdExecResize,
        close: commands.cmdExecClose,
      };
    }
    return {
      start: async (cols, rows) => {
        const s = await commands.cmdLocalShellStart(t.namespace, cols, rows);
        return {
          id: s.session,
          intro:
            `\x1b[90mLocal shell: kubectl uses context ${s.context}, namespace ${s.namespace}.\r\n` +
            "Changing the app's namespace or context does not change this shell.\x1b[0m\r\n",
        };
      },
      input: commands.cmdLocalShellInput,
      resize: commands.cmdLocalShellResize,
      close: commands.cmdLocalShellClose,
    };
  }
</script>

<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import { events } from "../bindings";

  interface Props {
    target: TerminalTarget;
    /** The shown dock tab: it gets the focus. Hidden tabs keep their session. */
    active?: boolean;
  }

  let { target, active = true }: Props = $props();

  // Picked once: a tab's target never changes.
  let backend: Backend | null = null;

  let host: HTMLDivElement;
  let term: Terminal | null = null;
  let fit: FitAddon | null = null;
  let session: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlisteners: (() => void)[] = [];
  let destroyed = false;
  /** Output that arrived before the start command told us our session id,
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
    backend?.resize(session, term.cols, term.rows).catch(() => {});
  }

  const closedNote = "\r\n\x1b[90m[session closed]\x1b[0m\r\n";

  $effect(() => {
    if (active && session) term?.focus();
  });

  onMount(async () => {
    const b = backendFor(target);
    backend = b;
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

    // Output can arrive before the start command resolves and `session` is set:
    // keep it by session until we know which one is ours.
    const un = await Promise.all([
      events.execOutput.listen((e) => {
        const { session: id, data } = e.payload;
        if (!term) return;
        if (id === session || (id === backend?.pending && !session)) term.write(data);
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
      const { id, intro } = await b.start(t.cols, t.rows);
      if (destroyed) {
        b.close(id).catch(() => {});
        return;
      }
      session = id;
      if (intro) t.write(intro);
      for (const data of early.get(id) ?? []) t.write(data);
      if (closedEarly.has(id)) t.write(closedNote);
      early = new Map();
      closedEarly = new Set();
      sendResize();
      t.onData((data) => {
        b.input(id, data).catch(() => {});
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
    if (session) backend?.close(session).catch(() => {});
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
