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
import { commands, events } from "../lib/bindings";
import type { ResourceKind, ResourceRef, ResourceRow, WatchBatch, WatchState } from "../lib/bindings";
import { defaultCluster } from "./fixtures/cluster";
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
  /** `streamId` of the most recent pod log stream, or null. */
  lastStreamId(): number | null;
  /** Send a `watch-batch`; `id` defaults to the latest watch. */
  emitWatchBatch(batch: Partial<WatchBatch>): Promise<void>;
  /** Send a `watch-state`; `id` defaults to the latest watch. */
  emitWatchState(state: WatchState & { id?: string }): Promise<void>;
  /** Send `pod-log` lines; `streamId` defaults to the latest stream. */
  emitLogLines(lines: string[], streamId?: number): Promise<void>;
  /** Send `pod-log-error`, then (if `done`) `pod-log-done`. */
  emitLogEnd(error: string, done?: boolean, streamId?: number): Promise<void>;
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
const rowKey = (namespace: string, name: string) => (namespace ? `${namespace}/${name}` : name);

export function installFakeBackend(cluster: FakeCluster = defaultCluster()): FakeApi {
  const calls = new Map<CommandName, CallArgs[]>();
  const failures = new Map<CommandName, string[]>();
  const watches: { id: string; kind: ResourceKind; namespace: string | null }[] = [];
  let watchSeq = 0;
  let drainSeq = 0;
  let sessionSeq = 0;
  let streamId: number | null = null;

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

  /** Remove a row and tell the watches of its kind, like the real watch would. */
  function removeRow(kind: ResourceKind, key: string) {
    cluster.rows[kind.id] = rowsOf(kind).filter((r) => r.key !== key);
    for (const w of watches) {
      if (w.kind.id === kind.id) void events.watchBatch.emit({ id: w.id, snapshot: false, applied: [], deleted: [key] });
    }
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
    cmdGetOverview: () => cluster.overview,
    cmdGetMetrics: () => cluster.metrics,

    cmdWatchStart: (resource, namespace) => {
      const kind = kindOf(resource);
      const id = `watch-${++watchSeq}`;
      watches.push({ id, kind, namespace });
      // Like the backend: the full list first, then "live". Sent after the
      // reply so the store knows the id (it also buffers early events).
      setTimeout(() => {
        if (!namespace && cluster.deniedClusterWide.includes(kind.id)) {
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
      return null;
    },

    cmdGetPodDetails: (namespace, name) => {
      const pod = cluster.pods[rowKey(namespace, name)];
      if (!pod) throw `pods "${name}" not found`;
      return pod;
    },
    cmdStreamPodLogs: (_namespace, _name, _container, _previous, _tail, _since, _timestamps, id) => {
      streamId = id;
      return null;
    },
    cmdStopPodLogs: () => null,
    cmdStreamWorkloadLogs: (_kind, _namespace, _name, _options, id) => {
      streamId = id;
      return null;
    },
    cmdStopWorkloadLogs: () => null,
    cmdSaveLogs: (filename) => `/home/tester/Downloads/${filename}`,

    cmdExecStart: () => `exec-${++sessionSeq}`,
    cmdExecInput: () => null,
    cmdExecResize: () => null,
    cmdExecClose: () => null,
    cmdPortforwardStart: (_namespace, _name, port) => ({ session: `pf-${++sessionSeq}`, local_port: 40000 + port }),
    cmdPortforwardStop: () => null,

    cmdGetResourceDetail: (resource, namespace, name) => {
      const { kind, row } = findRow(resource, namespace, name);
      const r = kind.resource;
      return {
        name,
        namespace,
        uid: `uid-${row.key}`,
        api_version: r.group ? `${r.group}/${r.version}` : r.version,
        kind: r.kind,
        created: row.created,
        age: "1h",
        labels: {},
        annotations: {},
        finalizers: [],
        owners: [],
        conditions: [],
        secret_keys: r.kind === "Secret" ? [{ key: "password", bytes: 7 }] : [],
        events: [],
      };
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
  };

  async function handle(cmd: string, args: CallArgs | undefined): Promise<unknown> {
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
    lastStreamId: () => streamId,
    emitWatchBatch: (b) =>
      events.watchBatch.emit({
        id: b.id ?? api.lastWatchId() ?? "",
        snapshot: b.snapshot ?? false,
        applied: b.applied ?? [],
        deleted: b.deleted ?? [],
      }),
    emitWatchState: (s) => events.watchState.emit({ ...s, id: s.id ?? api.lastWatchId() ?? "" }),
    emitLogLines: (lines, id) => events.podLog.emit({ streamId: id ?? streamId ?? -1, data: lines }),
    emitLogEnd: async (error, done = true, id) => {
      const sid = id ?? streamId ?? -1;
      if (error) await events.podLogError.emit({ streamId: sid, data: error });
      if (done) await events.podLogDone.emit({ streamId: sid });
    },
  };
  window.__fake = api;
  return api;
}
