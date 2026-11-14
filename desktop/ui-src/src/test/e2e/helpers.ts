import { expect } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import type { DetailState, Overview_Serialize, ResourceRow, WatchBatch, WatchState } from "../../lib/bindings";
import type { CallArgs, CommandName, FakeApi } from "../fake-backend";
import type { FakeCluster } from "../fixtures/cluster";

/**
 * Open the app on the fake backend. `init` runs in the page before the app
 * mounts: it may change the default cluster or queue `fake.failNext(...)`.
 * It is serialized, so it cannot use variables from the test.
 */
export async function openApp(
  page: Page,
  init?: (cluster: FakeCluster, fake: FakeApi) => void,
  { waitForKinds = true } = {},
): Promise<Fake> {
  if (init) await page.addInitScript(`window.__fakeInit = ${init.toString()};`);
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto("/test.html");
  if (waitForKinds) {
    await expect(page.getByRole("navigation").getByRole("button", { name: "Pods", exact: true })).toBeVisible();
  }
  return new Fake(page, errors);
}

/** Typed wrappers around `window.__fake` (see fake-backend.ts). */
export class Fake {
  constructor(
    readonly page: Page,
    /** Uncaught page errors so far; tests can assert it stays empty. */
    readonly errors: string[],
  ) {}

  calls(cmd: CommandName): Promise<CallArgs[]> {
    return this.page.evaluate((c) => window.__fake.calls(c), cmd);
  }

  async lastCall(cmd: CommandName): Promise<CallArgs> {
    const all = await this.calls(cmd);
    expect(all.length, `${cmd} was called`).toBeGreaterThan(0);
    return all[all.length - 1];
  }

  failNext(cmd: CommandName, error: string): Promise<void> {
    return this.page.evaluate(([c, e]) => window.__fake.failNext(c, e), [cmd, error] as const);
  }

  lastWatchId(): Promise<string | null> {
    return this.page.evaluate(() => window.__fake.lastWatchId());
  }

  lastStreamId(): Promise<number | null> {
    return this.page.evaluate(() => window.__fake.lastStreamId());
  }

  emitWatchBatch(batch: Partial<WatchBatch>): Promise<void> {
    return this.page.evaluate((b) => window.__fake.emitWatchBatch(b), batch);
  }

  emitWatchState(state: WatchState & { id?: string }): Promise<void> {
    return this.page.evaluate((s) => window.__fake.emitWatchState(s), state);
  }

  lastDetailWatchId(): Promise<string | null> {
    return this.page.evaluate(() => window.__fake.lastDetailWatchId());
  }

  activeDetailWatches(): Promise<string[]> {
    return this.page.evaluate(() => window.__fake.activeDetailWatches());
  }

  emitDetail(state: DetailState & { id?: string }): Promise<void> {
    return this.page.evaluate((s) => window.__fake.emitDetail(s), state);
  }

  lastOverviewId(): Promise<number | null> {
    return this.page.evaluate(() => window.__fake.lastOverviewId());
  }

  overviewFeeds(): Promise<number[]> {
    return this.page.evaluate(() => window.__fake.overviewFeeds());
  }

  emitOverview(change: Partial<Overview_Serialize>, id?: number): Promise<void> {
    return this.page.evaluate(([c, i]) => window.__fake.emitOverview(c, i), [change, id] as const);
  }

  emitOverviewState(state: WatchState & { id?: number }): Promise<void> {
    return this.page.evaluate((s) => window.__fake.emitOverviewState(s), state);
  }

  emitLogLines(lines: string[], streamId?: number): Promise<void> {
    return this.page.evaluate(([l, id]) => window.__fake.emitLogLines(l, id), [lines, streamId] as const);
  }

  /** Change the fake cluster in the page. */
  setCluster<A>(fn: (cluster: FakeCluster, arg: A) => void, arg: A): Promise<void> {
    return this.page.evaluate(
      ([src, a]) => {
        const f = new Function(`return (${src})`)() as (c: FakeCluster, a: unknown) => void;
        f(window.__fake.cluster, a);
      },
      [fn.toString(), arg] as const,
    );
  }

  /** A pod row as the backend would send it (cells in the Pods column order). */
  podRow(namespace: string, name: string, status = "Running", restarts = "0"): ResourceRow {
    // name, namespace, ready, status, restarts, node, age, cpu, memory
    const ready = status === "Running" ? "1/1" : "0/1";
    return {
      key: `${namespace}/${name}`,
      name,
      namespace,
      created: Math.floor(Date.now() / 1000) - 60,
      cells: [name, namespace, ready, status, restarts, "node-1", "", "", ""],
    };
  }
}

/** Click a kind in the sidebar. */
export async function openKind(page: Page, label: string) {
  await page.getByRole("navigation").getByRole("button", { name: label, exact: true }).click();
}

/** Data rows of the resource table (not the header). */
export function rows(page: Page): Locator {
  return page.getByTestId("resource-row");
}

/** The data row whose Name cell is `name`. */
export function row(page: Page, name: string): Locator {
  return rows(page).filter({ has: page.getByRole("cell", { name, exact: true }) });
}

/** The text of every cell in a column, top to bottom. */
export async function column(page: Page, label: string): Promise<string[]> {
  const headers = await page.locator(".table .head .th").allTextContents();
  const i = headers.findIndex((h) => h.replace(/[▲▼]/g, "").trim() === label);
  expect(i, `column ${label}`).toBeGreaterThan(0);
  // Cell 0 is the checkbox, like header 0.
  return rows(page).evaluateAll((els, idx) => els.map((el) => el.children[idx]?.textContent?.trim() ?? ""), i);
}
