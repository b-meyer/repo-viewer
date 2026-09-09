import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import type { RepoRow as Row } from '@/stores/repos';
import { MakeDiscovered, MakeStatus } from '@/tests/fixtures';
import RepoRow from './RepoRow.vue';

const NOW = 1_700_000_000_000;

/**
 * Mounts a row inside a table, so the `<tr>` is valid markup.
 */
function mountRow(
  row: Row,
  extra: {
    readError?: string | null;
    tier0Done?: boolean;
    expanded?: boolean;
    loadingDetail?: boolean;
    detailError?: string | null;
    openError?: string | null;
  } = {},
) {
  return mount(RepoRow, {
    props: {
      row,
      now: NOW,
      readError: extra.readError ?? null,
      tier0Done: extra.tier0Done ?? false,
      expanded: extra.expanded ?? false,
      loadingDetail: extra.loadingDetail ?? false,
      detailError: extra.detailError ?? null,
      openError: extra.openError ?? null,
      colspan: 7,
    },
    attachTo: document.createElement('tbody'),
  });
}

/**
 * The button whose label contains `label`.
 */
function button(wrapper: ReturnType<typeof mountRow>, label: string) {
  const found = wrapper.findAll('button').find((candidate) => candidate.text().includes(label));
  if (found === undefined) throw new Error(`no button labelled ${label}`);
  return found;
}

describe('RepoRow', () => {
  /**
   * The rule the whole design turns on. A row nothing has read has no counts, and must not show a
   * number — least of all a zero — for any of them.
   */
  it('renders no bare zero anywhere for a row that has only been discovered', () => {
    const wrapper = mountRow(MakeDiscovered());

    expect(wrapper.text()).not.toMatch(/\b0\b/);
    expect(wrapper.text()).toContain('counting…');
  });

  /**
   * Once Tier 0 has finished, a row still lacking a status is not waiting — it could not be read.
   * Saying `counting…` forever would be the same dishonesty as rendering an unknown as zero.
   */
  it('turns a still-unread row into unreadable once Tier 0 has finished', () => {
    const wrapper = mountRow(MakeDiscovered(), { tier0Done: true, readError: 'HEAD is corrupt' });

    expect(wrapper.text()).toContain('unreadable');
    expect(wrapper.text()).not.toContain('counting…');
    expect(wrapper.html()).toContain('HEAD is corrupt');
  });

  /**
   * A bare repo has no worktree, so its dirty flag can never be computed — not merely not yet.
   */
  it('says n/a rather than counting for a bare repository worktree', () => {
    const wrapper = mountRow(MakeStatus({ kind: 'bare' }));

    expect(wrapper.text()).toContain('n/a');
  });

  it('says counting for a worktree Tier 1 has not reached', () => {
    const wrapper = mountRow(MakeStatus());

    expect(wrapper.text()).toContain('counting…');
  });

  it('distinguishes clean from dirty once Tier 1 has run', () => {
    expect(mountRow(MakeStatus({ dirty: false })).text()).toContain('clean');

    const dirty = mountRow(MakeStatus({ dirty: true, conflicted: 2 }));
    expect(dirty.text()).toContain('dirty');
    expect(dirty.text()).toContain('2 conflicted');
  });

  /**
   * A partial failure keeps every value it did read; losing one field is not worth losing the row.
   */
  it('keeps its values and marks the stash count when the row reports an error', () => {
    const wrapper = mountRow(MakeStatus({ error: 'stash read failed', stashCount: 0 }));

    expect(wrapper.text()).toContain('alpha');
    expect(wrapper.text()).toContain('*');
    expect(wrapper.html()).toContain('stash read failed');
  });

  it('labels a non-ordinary repository kind', () => {
    expect(mountRow(MakeStatus({ kind: 'submodule' })).text()).toContain('submodule');
    expect(mountRow(MakeStatus({ kind: 'normal' })).text()).not.toContain('normal');
  });
  /**
   * A repository Tier 0 could not read is the one a user most wants to go and look at, so it
   * expands like any other row — its drawer is where the failure is explained and where the buttons
   * that open it elsewhere live. Rust validates those against what discovery found rather than
   * against the rows for exactly this case.
   */
  it('expands a row that could not be read, and explains why', () => {
    const wrapper = mountRow(MakeDiscovered({ path: 'C:/work/broken' }), {
      expanded: true,
      tier0Done: true,
      readError: 'HEAD is corrupt',
    });

    expect(wrapper.text()).toContain('HEAD is corrupt');
    expect(wrapper.text()).toContain('Reveal');
  });

  /**
   * And it must not claim a read failed while one is still to come: mid-scan, a row nothing has
   * reached yet is pending, not broken.
   */
  it('says a row is still being read rather than broken, mid-scan', () => {
    const wrapper = mountRow(MakeDiscovered({ path: 'C:/work/pending' }), { expanded: true });

    expect(wrapper.text()).toContain('counting…');
    expect(wrapper.text()).not.toContain('could not be read');
  });

  it('forwards which tool a row was asked to open in', async () => {
    const wrapper = mountRow(MakeStatus({ path: 'C:/work/a' }), { expanded: true });

    await button(wrapper, 'Editor').trigger('click');

    expect(wrapper.emitted('open')).toEqual([['editor']]);
  });
});
