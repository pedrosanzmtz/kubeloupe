<script lang="ts">
  import type { RelatedGroup, RelatedItem } from "../bindings";
  import { cellClass, isObjectGroup, itemKey } from "../related";
  import { kindOf, openObject } from "../stores/app.svelte";
  import type { LiveRelated } from "../stores/related.svelte";

  /** The related objects of a detail panel as small tables; a row of an
   *  object opens it. */
  let { related }: { related: LiveRelated } = $props();
  const groups = $derived(related.data?.groups ?? []);

  /** The discovered kind of the group's rows, if they can be opened. */
  function kindFor(g: RelatedGroup) {
    return isObjectGroup(g) ? kindOf(g.api_version, g.kind) : undefined;
  }

  function open(g: RelatedGroup, item: RelatedItem) {
    const kind = kindFor(g);
    if (kind) openObject(kind, kind.resource.namespaced ? item.namespace : "", item.name);
  }
</script>

{#if related.error && !related.data}
  <p class="error-text small">Could not load related objects: {related.error}</p>
{/if}

{#each groups as g (g.title)}
  {@const kind = kindFor(g)}
  <section class="related" aria-label={g.title} data-testid="related-group">
    <h3>{g.title} {#if !g.error}<span class="muted count">({g.items.length})</span>{/if}</h3>
    {#if g.error}
      <p class="error-text small">{g.error}</p>
    {:else if g.items.length === 0}
      <p class="muted small">None.</p>
    {:else}
      <div class="scroll-x">
        <table>
          <thead>
            <tr>
              {#each g.columns as c, i (i)}<th>{c}</th>{/each}
            </tr>
          </thead>
          <tbody>
            {#each g.items as item, i (itemKey(g, item, i))}
              <!-- The whole row is a click target; keyboard users have the
                   name's button. -->
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
              <tr
                class:clickable={!!kind}
                onclick={() => open(g, item)}
                title={kind ? `Open ${g.kind} ${item.name}` : undefined}
              >
                {#each item.cells as cell, c (c)}
                  <td class={cellClass(g.columns[c] ?? "", cell)}>
                    {#if kind && c === g.name_column}
                      <button type="button" class="link" onclick={(e) => (e.stopPropagation(), open(g, item))}
                        >{cell}</button
                      >
                    {:else}
                      {cell}
                    {/if}
                  </td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </section>
{/each}

<style>
  h3 {
    margin: 14px 0 8px;
    font-size: 13px;
  }
  .count {
    font-weight: normal;
  }
  .small {
    font-size: 12px;
    margin: 0 0 6px;
  }
  table {
    font-size: 12px;
  }
  .scroll-x {
    overflow-x: auto;
  }
  tr.clickable {
    cursor: pointer;
  }
  tr.clickable:hover {
    background: var(--bg-hover);
  }
  .link {
    background: transparent;
    border: none;
    color: var(--accent);
    padding: 0;
    font: inherit;
    cursor: pointer;
    text-align: left;
  }
  .link:hover {
    text-decoration: underline;
  }
  .short {
    color: var(--amber);
  }
</style>
