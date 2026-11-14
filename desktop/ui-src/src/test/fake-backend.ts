/**
 * Fake Tauri backend for the Playwright app tests (`src/test/e2e`).
 *
 * `installFakeBackend()` routes every `invoke` through `mockIPC` from
 * `@tauri-apps/api/mocks` with `shouldMockEvents`, so the app's
 * `events.*.listen` calls work and the fake can emit events. Commands are
 * served from an in-memory cluster (`fixtures/cluster.ts`).
 *
 * Tests drive it from Playwright through `window.__fake` (see `FakeApi`):
 *
 *     await page.evaluate(() => window.__fake.emitWatchBatch({ deleted: ["demo/web-1"] }));
 *     await page.evaluate(() => window.__fake.calls("cmdStreamPodLogs"));
 *
 * ## Adding a command
 *
 * 1. Regenerate `bindings.ts` as usual. `npm run check` now fails here:
 *    `HANDLERS` must implement every command in `commands`, with its exact
 *    parameters and return type.
 * 2. Add the handler to `HANDLERS` below. It gets the command's arguments
 *    positionally, like the function in `bindings.ts`, and returns the result
 *    (or throws a string, which is how a Rust `Err(String)` arrives). Keep the
 *    data in `FakeCluster` so tests can change it instead of the handler.
 * 3. Write the test in `src/test/e2e/`; `fake.failNext("cmdYourCommand", "...")`
 *    and `fake.calls("cmdYourCommand")` work for every command.
 *
 * ## Adding an event
 *
 * Emit it with `events.<name>.emit(payload)` from `bindings.ts` (typed), and
 * add an `emit…` helper to `FakeApi` if tests need to send it themselves.
 */

import { mockIPC } from "@tauri-apps/api/mocks";
import { applyPatch } from "../lib/settings";
import type { SettingsPatch } from "../lib/settings";
import { commands, events } from "../lib/bindings";
import type {
  ApplyResult,
  ForwardInfo,
  DetailState,
  Overview_Serialize,
  ResourceKind,
  ResourceRef,
  ResourceRow,
  WatchBatch,
  WatchState,
} from "../lib/bindings";
import { defaultCluster, detail, makeRow, NO_POD } from "./fixtures/cluster";
import { NO_RELATED } from "./fixtures/related";
import type { FakeCluster } from "./fixtures/cluster";

type Commands = typeof commands;
export type CommandName = keyof Commands;
type Result<K extends CommandName> = Awaited<ReturnType<Commands[K]>>;
type Handler<K extends CommandName> = (...args: Parameters<Commands[K]>) => Result<K> | Promise<Result<K>>;
/** Every command must have a handler: a new command breaks `npm run check` here. */
type Handlers = { [K in CommandName]: Handler<K> };

/** Raw IPC arguments of one call, keyed like the Rust parameters. */
export type CallArgs = Record<string, unknown>;

/** What tests reach through `window.__fake`. */
export interface FakeApi {
  /** The in-memory cluster; change it freely between steps. */
  cluster: FakeCluster;
  /** Arguments of every call of `cmd` so far, oldest first. */
  calls(cmd: CommandName): CallArgs[];
  /** Make the next call of `cmd` fail with `error` (a Rust `Err(String)`). */
  failNext(cmd: CommandName, error: string): void;
  /** Id of the most recent watch (`cmd_watch_start`), or null. */
  lastWatchId(): string | null;
  /** The watches that were started and not stopped, oldest first. */
  activeWatches(): { id: string; namespace: string | null }[];
  /** `streamId` of the most recent log stream (pod or workload), or null. */
  lastStreamId(): number | null;
  /** Ids of the log streams started and not stopped, oldest first. */
  activeLogStreams(): number[];
  /** Send a `watch-batch`; `id` defaults to the latest watch. */
  emitWatchBatch(batch: Partial<WatchBatch>): Promise<void>;
  /** Send a `watch-state`; `id` defaults to the latest watch. */
  emitWatchState(state: WatchState & { id?: string }): Promise<void>;
  /** Id of the most recent detail watch (`cmd_detail_watch`), or null. */
  lastDetailWatchId(): string | null;
  /** Ids of the detail watches that were started and not stopped. */
  activeDetailWatches(): string[];
  /** Send a `detail-watch`; `id` defaults to the latest detail watch. */
  emitDetail(state: DetailState & { id?: string }): Promise<void>;
  /** Id of the most recent overview feed (`cmd_overview_start`), or null. */
  lastOverviewId(): number | null;
  /** Ids of the overview feeds started and not stopped. */
  overviewFeeds(): number[];
  /** Merge `change` into `cluster.overview` and send it; `id` defaults to the latest feed. */
  emitOverview(change: Partial<Overview_Serialize>, id?: number): Promise<void>;
  /** Send an `overview-state`; `id` defaults to the latest feed. */
  emitOverviewState(state: WatchState & { id?: number }): Promise<void>;
  /** Send `pod-log` lines; `streamId` defaults to the latest stream. */
  emitLogLines(lines: string[], streamId?: number): Promise<void>;
  /** Send `pod-log-error`, then (if `done`) `pod-log-done`. */
  emitLogEnd(error: string, done?: boolean, streamId?: number): Promise<void>;
  /** URLs the app asked `tauri-plugin-opener` to open. */
  opened(): string[];
}

declare global {
  interface Window {
    __fake: FakeApi;
    /** Set by a Playwright init script to adjust the fake before the app mounts. */
    __fakeInit?: (cluster: FakeCluster, fake: FakeApi) => void;
  }
}

const toCamel = (cmd: string) => cmd.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()) as CommandName;
const sameResource = (a: ResourceRef, b: ResourceRef) =>
  a.group === b.group && a.version === b.version && a.kind === b.kind && a.plural === b.plural;
/** A JSON copy, like an event payload over real IPC. */
const copy = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
const rowKey = (namespace: string, name: string) => (namespace ? `${namespace}/${name}` : name);
/** Where the fake keeps the settings across reloads (the real app: a file). */
export const SETTINGS_KEY = "kubeloupe-fake-settings";

export function installFakeBackend(cluster: FakeCluster = defaultCluster()): FakeApi {
  const calls = new Map<CommandName, CallArgs[]>();
  const failures = new Map<CommandName, string[]>();
  const watches: { id: string; kind: ResourceKind; namespace: string | null }[] = [];
  /** Detail watches by id, with the `kindId|rowKey` they follow. */
  const detailWatches = new Map<string, string>();
  let watchSeq = 0;
  let detailSeq = 0;
  let drainSeq = 0;
  let sessionSeq = 0;
  let streamId: number | null = null;
  /** Running log streams, like the backend's map (with its cap). */
  const logStreams = new Set<number>();
  const MAX_LOG_STREAMS = 10;
  const startStream = (id: number) => {
    logStreams.delete(id);
    if (logStreams.size >= MAX_LOG_STREAMS) {
      throw `Too many log streams: ${MAX_LOG_STREAMS} are already open. Close a log tab to open another.`;
    }
    logStreams.add(id);
    streamId = id;
    return null;
  };
  /** Running overview feeds, and the id of the latest one. */
  const overviewFeeds = new Set<number>();
  let lastOverview: number | null = null;
  try {
    const saved = sessionStorage.getItem(SETTINGS_KEY);
    if (saved) cluster.settings = JSON.parse(saved) as FakeCluster["settings"];
  } catch {
    // Keep the defaults.
  }

  function kindOf(resource: ResourceRef): ResourceKind {
    const kind = cluster.kinds.find((k) => sameResource(k.resource, resource));
    if (!kind) throw `the server could not find the requested resource (${resource.plural})`;
    return kind;
  }

  function rowsOf(kind: ResourceKind): ResourceRow[] {
    return (cluster.rows[kind.id] ??= []);
  }

  function findRow(resource: ResourceRef, namespace: string, name: string): { kind: ResourceKind; row: ResourceRow } {
    const kind = kindOf(resource);
    const row = rowsOf(kind).find((r) => r.key === rowKey(namespace, name));
    if (!row) throw `${kind.resource.plural} "${name}" not found`;
    return { kind, row };
  }

  /** Remove a row and tell the watches of its kind (and the detail watches
   *  of the object), like the real watches would. */
  function removeRow(kind: ResourceKind, key: string) {
    cluster.rows[kind.id] = rowsOf(kind).filter((r) => r.key !== key);
    for (const w of watches) {
      if (w.kind.id === kind.id) void events.watchBatch.emit({ id: w.id, snapshot: false, applied: [], deleted: [key] });
    }
    for (const [id, of] of detailWatches) {
      if (of === `${kind.id}|${key}`) void events.detailWatch.emit({ id, state: "deleted" });
    }
  }

  function detailOf(resource: ResourceRef, namespace: string, name: string) {
    const { kind, row } = findRow(resource, namespace, name);
    const known = cluster.details[`${kind.id}|${row.key}`];
    if (known) return known;
    return detail(kind, namespace, name, {
      uid: `uid-${row.key}`,
      created: row.created,
      secret_keys: kind.resource.kind === "Secret" ? [{ key: "password", bytes: 7 }] : [],
      pod: NO_POD,
    });
  }

  function defaultYaml(kind: ResourceKind, row: ResourceRow): string {
    const r = kind.resource;
    const lines = [
      `apiVersion: ${r.group ? `${r.group}/${r.version}` : r.version}`,
      `kind: ${r.kind}`,
      "metadata:",
      `  name: ${row.name}`,
    ];
    if (row.namespace) lines.push(`  namespace: ${row.namespace}`);
    if (r.kind === "Secret") lines.push("type: Opaque", "data:", "  password: <hidden>");
    return lines.join("\n") + "\n";
  }

  /** Add (or replace) a row and tell the watches of its kind. */
  function addRow(kind: ResourceKind, row: ResourceRow) {
    cluster.rows[kind.id] = [...rowsOf(kind).filter((r) => r.key !== row.key), row];
    for (const w of watches) {
      if (w.kind.id === kind.id && (!w.namespace || w.namespace === row.namespace)) {
        void events.watchBatch.emit({ id: w.id, snapshot: false, applied: [row], deleted: [] });
      }
    }
  }

  /**
   * A small stand-in for `apply_manifests`: documents split at `---`, the
   * identity read with regexes (block style, as the templates are written).
   */
  function applyManifests(yaml: string, namespace: string, dryRun: boolean, skipInvalid: boolean): ApplyResult[] {
    const field = (doc: string, re: RegExp) => re.exec(doc)?.[1]?.trim() ?? "";
    const docs: { line: number; text: string }[] = [];
    let start = 1;
    let cur: string[] = [];
    yaml.split("\n").forEach((l, i) => {
      if (/^---(\s|$)/.test(l)) {
        docs.push({ line: start, text: cur.join("\n") });
        cur = [];
        start = i + 2;
      } else cur.push(l);
    });
    docs.push({ line: start, text: cur.join("\n") });
    const planned = docs
      .filter((d) => d.text.split("\n").some((l) => l.trim() && !l.trim().startsWith("#")))
      .map((d, i) => {
        const apiVersion = field(d.text, /^apiVersion:(.*)$/m);
        const kindName = field(d.text, /^kind:(.*)$/m);
        const name = field(d.text, /^ {2}name:(.*)$/m);
        const kind = cluster.kinds.find(
          (k) =>
            k.resource.kind === kindName &&
            (k.resource.group ? `${k.resource.group}/${k.resource.version}` : k.resource.version) === apiVersion,
        );
        const ns = kind?.resource.namespaced ? field(d.text, /^ {2}namespace:(.*)$/m) || namespace || "default" : "";
        const exists = kind ? rowsOf(kind).some((r) => r.key === rowKey(ns, name)) : false;
        const error = !kind
          ? `the cluster does not serve ${kindName} in ${apiVersion}`
          : (cluster.applyErrors[name] ?? "");
        const result: ApplyResult = {
          document: i + 1,
          line: d.line,
          api_version: apiVersion,
          kind: kindName,
          name,
          namespace: ns,
          status: error ? "failed" : exists ? "unchanged" : "created",
          message: error,
          takeovers: [],
        };
        return { result, kind };
      });
    const failed = planned.filter((p) => p.result.status === "failed").length;
    if (dryRun) return planned.map((p) => p.result);
    if (failed && !skipInvalid) {
      return planned.map((p) =>
        p.result.status === "failed"
          ? p.result
          : { ...p.result, status: "skipped" as const, message: `not applied: ${failed} document(s) failed validation` },
      );
    }
    for (const p of planned) {
      if (p.kind && p.result.status === "created") addRow(p.kind, makeRow(cluster.kinds, p.kind.id, p.result.namespace, p.result.name, {}, Math.floor(Date.now() / 1000)));
    }
    return planned.map((p) => p.result);
  }

  /** Set `metadata.resourceVersion` the way the server reports it (after name / namespace). */
  function withResourceVersion(yaml: string, rv: number): string {
    const line = `  resourceVersion: "${rv}"\n`;
    if (/^ {2}resourceVersion: .*\n/m.test(yaml)) return yaml.replace(/^ {2}resourceVersion: .*\n/m, line);
    return yaml.replace(/^(metadata:\n {2}name: .*\n(?: {2}namespace: .*\n)?)/m, `$1${line}`);
  }

  const HANDLERS: Handlers = {
    cmdListContexts: () => cluster.contexts,
    cmdSetContext: (name) => {
      cluster.contexts = cluster.contexts.map((c) => ({ ...c, current: c.name === name }));
      // Like the backend: a context switch stops every log stream.
      logStreams.clear();
      return null;
    },
    cmdListNamespaces: () => cluster.namespaces,
    cmdDefaultNamespace: () => cluster.defaultNamespace,
    cmdDiscover: () => cluster.kinds,
    cmdCheckAccess: (kinds, namespace) =>
      kinds.map((r) => {
        const id = kindOf(r).id;
        const clusterWide = r.namespaced && namespace === null;
        const allowed = !cluster.denied.includes(id) && !(clusterWide && cluster.deniedClusterWide.includes(id));
        return { id, allowed };
      }),
    cmdOverviewStart: (id) => {
      overviewFeeds.add(id);
      lastOverview = id;
      // Like the backend: the first summary, then "live".
      setTimeout(() => {
        if (!overviewFeeds.has(id)) return;
        void events.overviewUpdate.emit({ id, overview: copy(cluster.overview) });
        void events.overviewState.emit({ id, state: "live" });
      }, 0);
      return null;
    },
    cmdOverviewStop: (id) => {
      overviewFeeds.delete(id);
      return null;
    },
    cmdGetMetrics: (namespace, what) => {
      const m = cluster.metrics;
      return what === "pods"
        ? { available: m.available, pods: m.pods.filter((p) => !namespace || p.namespace === namespace), nodes: [] }
        : { available: m.available, pods: [], nodes: m.nodes };
    },

    cmdWatchStart: (resource, namespace) => {
      const kind = kindOf(resource);
      const id = `watch-${++watchSeq}`;
      watches.push({ id, kind, namespace });
      // Like the backend: the full list first, then "live". Sent after the
      // reply so the store knows the id (it also buffers early events).
      setTimeout(() => {
        const denied = namespace
          ? cluster.deniedNamespaces.includes(namespace)
          : cluster.deniedClusterWide.includes(kind.id);
        if (denied) {
          // Like the backend: a 403 ends the watch with one "forbidden".
          void events.watchState.emit({ id, state: "forbidden", error: `${kind.resource.plural} is forbidden` });
          return;
        }
        const applied = rowsOf(kind).filter((r) => !namespace || r.namespace === namespace);
        void events.watchBatch.emit({ id, snapshot: true, applied, deleted: [] });
        void events.watchState.emit({ id, state: "live" });
      }, 0);
      return id;
    },
    cmdWatchStop: (id) => {
      const i = watches.findIndex((w) => w.id === id);
      if (i >= 0) watches.splice(i, 1);
      detailWatches.delete(id);
      return null;
    },
    cmdDetailWatch: (resource, namespace, name) => {
      const { kind, row } = findRow(resource, namespace, name);
      const id = `detail-${++detailSeq}`;
      detailWatches.set(id, `${kind.id}|${row.key}`);
      return id;
    },
    cmdStreamPodLogs: (_namespace, _name, _container, _previous, _tail, _since, _timestamps, id) => startStream(id),
    cmdStreamWorkloadLogs: (_kind, _namespace, _name, _options, id) => startStream(id),
    cmdStopLogs: (id) => {
      logStreams.delete(id);
      return null;
    },
    cmdSaveLogs: (filename) => `/home/tester/Downloads/${filename}`,

    cmdExecStart: () => `exec-${++sessionSeq}`,
    cmdExecInput: () => null,
    cmdExecResize: () => null,
    cmdExecClose: () => null,
    // A local shell at the selected context; `namespace` as asked.
    cmdLocalShellStart: (namespace) => ({
      session: `local-${++sessionSeq}`,
      context: cluster.contexts.find((c) => c.current)?.name ?? "",
      namespace: namespace ?? "default",
    }),
    cmdLocalShellInput: () => null,
    cmdLocalShellResize: () => null,
    cmdLocalShellClose: () => null,
    // Like the backend: no local port means the remote one if >= 1024, else 40000 + it.
    cmdPortforwardStart: (target, remote, local) => {
      const same = (f: ForwardInfo) =>
        f.target.kind === target.kind && f.target.namespace === target.namespace && f.target.name === target.name;
      const existing = local === null ? cluster.forwards.find((f) => same(f) && f.remote === remote) : undefined;
      if (existing) return existing;
      const remotePort = /^\d+$/.test(remote) ? Number(remote) : 8080;
      const port = local ?? (remotePort >= 1024 ? remotePort : 40000 + remotePort);
      const localPort = port === 0 ? 45000 + sessionSeq : port;
      if (local !== null && cluster.forwards.some((f) => f.local_port === localPort)) {
        throw `local port ${localPort} is already in use: pick another one, or leave it empty for a free one`;
      }
      const f: ForwardInfo = {
        id: `pf-${++sessionSeq}`,
        target,
        remote,
        remote_port: remotePort,
        local_port: localPort,
        url: `http://127.0.0.1:${localPort}`,
        started_at: Date.now(),
        status: { pod: target.kind === "pod" ? target.name : `${target.name}-1`, connections: 0, total_connections: 0, last_error: "" },
      };
      cluster.forwards.push(f);
      return f;
    },
    cmdPortforwardStop: (id) => {
      cluster.forwards = cluster.forwards.filter((f) => f.id !== id);
      return null;
    },
    // A copy, like real IPC: the app must see a new list after the tests change this one.
    cmdPortforwardList: () => JSON.parse(JSON.stringify(cluster.forwards)) as ForwardInfo[],
    cmdServicePorts: (namespace, name) => cluster.servicePorts[`${namespace}/${name}`] ?? [],

    cmdGetResourceDetail: (resource, namespace, name) => detailOf(resource, namespace, name),
    cmdGetRelated: (resource, namespace, name) => {
      const { kind, row } = findRow(resource, namespace, name);
      // A copy, like real IPC: tests change the fixture in place.
      return structuredClone(cluster.related[`${kind.id}|${row.key}`] ?? NO_RELATED);
    },
    cmdGetResourceYaml: (resource, namespace, name, reveal) => {
      const { kind, row } = findRow(resource, namespace, name);
      const key = `${kind.id}|${row.key}`;
      const yaml = (reveal && cluster.revealedYaml[key]) || cluster.yaml[key] || defaultYaml(kind, row);
      return withResourceVersion(yaml, cluster.resourceVersions[key] ?? 1);
    },
    cmdApplyResourceYaml: (resource, namespace, name, yaml, resourceVersion, dryRun) => {
      const { kind, row } = findRow(resource, namespace, name);
      const key = `${kind.id}|${row.key}`;
      const current = String(cluster.resourceVersions[key] ?? 1);
      if (resourceVersion && resourceVersion !== current) {
        return { outcome: "conflict", resource_version: current };
      }
      // A dry run reports what the save would take over, without using it up.
      if (dryRun) return { outcome: "applied", takeovers: cluster.nextTakeovers };
      cluster.yaml[key] = yaml;
      cluster.resourceVersions[key] = Number(current) + 1;
      const took = cluster.nextTakeovers;
      cluster.nextTakeovers = [];
      return { outcome: "applied", takeovers: took };
    },
    cmdApplyManifests: (yaml, namespace, dryRun, skipInvalid) => applyManifests(yaml, namespace, dryRun, skipInvalid),
    cmdDeleteResource: (resource, namespace, name) => {
      const { kind, row } = findRow(resource, namespace, name);
      removeRow(kind, row.key);
      return null;
    },
    cmdRunAction: (resource, namespace, name, action, replicas) => {
      const { kind, row } = findRow(resource, namespace, name);
      const label = `${kind.resource.kind.toLowerCase()} ${row.key}`;
      switch (action) {
        case "delete":
        case "forceDelete":
          removeRow(kind, row.key);
          return `Deleted ${label}`;
        case "scale":
          return `Scaled ${label} to ${replicas}`;
        default:
          return `${action} ${label}: done`;
      }
    },
    cmdBulkDelete: (resource, items) => {
      const kind = kindOf(resource);
      return items.map(({ namespace, name }) => {
        const key = rowKey(namespace, name);
        const error = cluster.bulkErrors[key] ?? "";
        if (!error) removeRow(kind, key);
        return { key, error };
      });
    },
    cmdDrainNode: (node) => {
      const drainId = `drain-${++drainSeq}`;
      setTimeout(() => {
        void events.drainProgress.emit({ drainId, event: { type: "cordoned", node } });
        void events.drainFinished.emit({
          drainId,
          error: "",
          summary: { evicted: 0, failed: 0, skipped: 0, still_terminating: 0 },
        });
      }, 0);
      return drainId;
    },
    cmdCancelDrain: () => null,

    cmdGetSettings: () => copy(cluster.settings),
    cmdUpdateSettings: (patch) => {
      // The app sends whole values (`SettingsPatch`), like the backend expects.
      cluster.settings = applyPatch(cluster.settings, patch as SettingsPatch);
      sessionStorage.setItem(SETTINGS_KEY, JSON.stringify(cluster.settings));
      return copy(cluster.settings);
    },
  };

  const opened: string[] = [];

  async function handle(cmd: string, args: CallArgs | undefined): Promise<unknown> {
    // tauri-plugin-opener's `openUrl` (Open in browser).
    if (cmd === "plugin:opener|open_url") {
      opened.push(String(args?.url));
      return null;
    }
    const name = toCamel(cmd);
    const handler = HANDLERS[name] as ((...a: unknown[]) => unknown) | undefined;
    if (!handler) throw new Error(`fake backend: no handler for ${cmd}`);
    let list = calls.get(name);
    if (!list) calls.set(name, (list = []));
    // A JSON copy, like real IPC (and app state may hold Svelte proxies, which
    // structuredClone rejects).
    list.push(JSON.parse(JSON.stringify(args ?? {})) as CallArgs);
    const fail = failures.get(name)?.shift();
    if (fail !== undefined) throw fail;
    // `bindings.ts` builds the argument object in parameter order.
    return handler(...Object.values(args ?? {}));
  }

  mockIPC((cmd, args) => handle(cmd, args as CallArgs | undefined), { shouldMockEvents: true });

  const api: FakeApi = {
    cluster,
    calls: (cmd) => calls.get(cmd) ?? [],
    failNext: (cmd, error) => {
      const list = failures.get(cmd) ?? [];
      list.push(error);
      failures.set(cmd, list);
    },
    lastWatchId: () => (watchSeq ? `watch-${watchSeq}` : null),
    activeWatches: () => watches.map((w) => ({ id: w.id, namespace: w.namespace })),
    lastStreamId: () => streamId,
    activeLogStreams: () => [...logStreams],
    emitWatchBatch: (b) =>
      events.watchBatch.emit({
        id: b.id ?? api.lastWatchId() ?? "",
        snapshot: b.snapshot ?? false,
        applied: b.applied ?? [],
        deleted: b.deleted ?? [],
      }),
    emitWatchState: (s) => events.watchState.emit({ ...s, id: s.id ?? api.lastWatchId() ?? "" }),
    lastDetailWatchId: () => (detailSeq ? `detail-${detailSeq}` : null),
    activeDetailWatches: () => [...detailWatches.keys()],
    emitDetail: (s) => events.detailWatch.emit({ ...s, id: s.id ?? api.lastDetailWatchId() ?? "" }),
    lastOverviewId: () => lastOverview,
    overviewFeeds: () => [...overviewFeeds],
    emitOverview: (change, id) => {
      Object.assign(cluster.overview, change);
      return events.overviewUpdate.emit({ id: id ?? lastOverview ?? -1, overview: copy(cluster.overview) });
    },
    emitOverviewState: (s) => events.overviewState.emit({ ...s, id: s.id ?? lastOverview ?? -1 }),
    emitLogLines: (lines, id) => events.podLog.emit({ streamId: id ?? streamId ?? -1, data: lines }),
    opened: () => [...opened],
    emitLogEnd: async (error, done = true, id) => {
      const sid = id ?? streamId ?? -1;
      if (error) await events.podLogError.emit({ streamId: sid, data: error });
      if (done) await events.podLogDone.emit({ streamId: sid });
    },
  };
  window.__fake = api;
  return api;
}
