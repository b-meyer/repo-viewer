import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import type { RepoStatus } from '@/scripts/generated/RepoStatus';
import { MakeCounts, MakeStatus } from '@/tests/fixtures';
import RepoDetail from './RepoDetail.vue';

const NOW = 1_700_000_000_000;

function mountDetail(
  row: RepoStatus,
  extra: { loading?: boolean; detailError?: string | null; openError?: string | null } = {},
) {
  return mount(RepoDetail, {
    props: {
      row,
      loading: extra.loading ?? false,
      detailError: extra.detailError ?? null,
      openError: extra.openError ?? null,
      fetchState: null,
      fetchError: null,
      gitMissing: null,
      now: NOW,
    },
  });
}

/**
 * The button whose label contains `label`.
 */
function button(wrapper: ReturnType<typeof mountDetail>, label: string) {
  const found = wrapper.findAll('button').find((candidate) => candidate.text().includes(label));
  if (found === undefined) throw new Error(`no button labelled ${label}`);
  return found;
}

/**
 * The Re-read button specifically, which is no longer the first one in the drawer.
 */
function reRead(wrapper: ReturnType<typeof mountDetail>) {
  return button(wrapper, 'Re-read');
}

describe('RepoDetail', () => {
  /**
   * The rule the whole design turns on, at the component that would break it most easily: four
   * numbers with nothing to put in them must not read as four zeroes.
   */
  it('renders no bare zero for a row whose counts have not been read', () => {
    const wrapper = mountDetail(MakeStatus({ counts: null, submodules: null }), {
      loading: true,
    });

    expect(wrapper.text()).toContain('counting…');
    expect(wrapper.text()).not.toMatch(/\b0\b/);
  });

  it('shows each column separately once the counts arrive', () => {
    const wrapper = mountDetail(
      MakeStatus({
        counts: MakeCounts({ staged: 1, unstaged: 2, untracked: 3, conflicted: 4 }),
        submodules: [],
      }),
    );

    for (const label of ['Staged', 'Unstaged', 'Untracked', 'Conflicted']) {
      expect(wrapper.text()).toContain(label);
    }
    // Both halves are read, so nothing in the drawer claims work is still happening.
    expect(wrapper.text()).not.toContain('counting…');
  });

  /**
   * A computed zero is a measurement and shows as `0`; that is the opposite of the case above, and
   * the pair is what makes the distinction real rather than stated.
   */
  it('shows a computed zero as a number', () => {
    const wrapper = mountDetail(
      MakeStatus({ counts: MakeCounts({ conflicted: 0 }), submodules: [] }),
    );

    expect(wrapper.text()).toMatch(/Conflicted\s*0/);
  });

  /**
   * The four counts are per-column totals, not a partition of paths — a staged-then-modified file
   * appears in two of them — so no total may be rendered.
   */
  it('never presents the counts as a total', () => {
    const wrapper = mountDetail(
      MakeStatus({
        counts: MakeCounts({ staged: 1, unstaged: 2, untracked: 3, conflicted: 0 }),
        submodules: [],
      }),
    );

    expect(wrapper.text()).not.toContain('6');
    expect(wrapper.text().toLowerCase()).not.toContain('total');
  });

  /**
   * A bare repository has no worktree to diff and no `.gitmodules`, so these can never be computed.
   * `n/a`, never `counting…`: the second would be a claim that work is in progress for something no
   * work will ever be done on.
   */
  it('says n/a rather than counting for a bare repository', () => {
    const wrapper = mountDetail(MakeStatus({ kind: 'bare' }));

    expect(wrapper.text()).toContain('n/a');
    expect(wrapper.text()).not.toContain('counting…');
  });

  it('reports a failed read as unreadable, with the cause', () => {
    const wrapper = mountDetail(MakeStatus({ counts: null }), {
      detailError: 'index unreadable',
    });

    expect(wrapper.text()).toContain('unreadable');
    expect(wrapper.text()).not.toContain('counting…');
    expect(wrapper.html()).toContain('index unreadable');
  });

  /**
   * A read that failed after an earlier one succeeded: the numbers were really measured, so they
   * stay, and the failure appears beside them rather than replacing them with unknowns.
   */
  it('keeps showing counts when a later read failed, and says so', () => {
    const wrapper = mountDetail(MakeStatus({ counts: MakeCounts(), submodules: [] }), {
      detailError: 'index unreadable',
    });

    expect(wrapper.text()).toContain('Staged');
    expect(wrapper.text()).toContain('Re-read failed');
    // The counts are still values, so nothing stands in for them. Asserted on the count itself
    // rather than on the word, because the cause message happens to contain it too.
    expect(wrapper.text()).toMatch(/Staged\s*1/);
  });

  /**
   * **Tier 2's two halves fail independently**, so each section answers for its own field. A shared
   * answer left the succeeding section rendering nothing at all.
   */
  it('describes each half separately when only one of them failed', () => {
    const wrapper = mountDetail(MakeStatus({ counts: MakeCounts(), submodules: null }), {
      detailError: 'cannot read submodules of `C:/work/alpha`',
    });

    expect(wrapper.text()).toMatch(/Staged\s*1/);
    // The submodule half has no value, so it says so rather than rendering an empty section.
    expect(wrapper.text()).toContain('unreadable');
  });

  /**
   * `Some([])` and `None` are different facts: read-and-there-are-none against not-read-yet.
   */
  it('distinguishes an empty submodule list from an unread one', () => {
    const empty = mountDetail(MakeStatus({ counts: MakeCounts(), submodules: [] }));
    expect(empty.text()).toContain('—');
    expect(empty.text()).not.toContain('counting…');

    const unread = mountDetail(MakeStatus({ counts: null, submodules: null }), { loading: true });
    expect(unread.text()).toContain('counting…');
  });

  it('lists a submodule and whether it matches what the parent records', () => {
    const wrapper = mountDetail(
      MakeStatus({
        counts: MakeCounts(),
        submodules: [
          { name: 'sub', path: 'vendor/sub', recordedId: 'a'.repeat(40), headId: 'a'.repeat(40) },
          {
            name: 'moved',
            path: 'vendor/moved',
            recordedId: 'b'.repeat(40),
            headId: 'c'.repeat(40),
          },
        ],
      }),
    );

    expect(wrapper.text()).toContain('vendor/sub');
    expect(wrapper.text()).toContain('in sync');
    expect(wrapper.text()).toContain('moved');
  });

  /**
   * A submodule that was never checked out has no HEAD of its own. That is a real answer about the
   * submodule, not a failed read, so it is `n/a` rather than an error.
   */
  it('says n/a for a submodule that is not checked out', () => {
    const wrapper = mountDetail(
      MakeStatus({
        counts: MakeCounts(),
        submodules: [{ name: 'sub', path: 'vendor/sub', recordedId: 'a'.repeat(40), headId: null }],
      }),
    );

    expect(wrapper.text()).toContain('n/a');
  });

  it('emits refresh when the re-read button is pressed', async () => {
    const wrapper = mountDetail(MakeStatus({ counts: MakeCounts(), submodules: [] }));

    await reRead(wrapper).trigger('click');

    expect(wrapper.emitted('refresh')).toHaveLength(1);
  });

  it('disables the re-read button while a read is in flight', () => {
    const wrapper = mountDetail(MakeStatus({ counts: MakeCounts(), submodules: [] }), {
      loading: true,
    });

    expect(reRead(wrapper).attributes('disabled')).toBeDefined();
  });

  /**
   * The three ways out of the app sit in the drawer beside Re-read, and each reports which one was
   * pressed rather than being wired to a target of its own.
   */
  it('emits the target when an open-in button is pressed', async () => {
    const wrapper = mountDetail(MakeStatus({ counts: MakeCounts(), submodules: [] }));

    await button(wrapper, 'Terminal').trigger('click');

    expect(wrapper.emitted('open')).toEqual([['terminal']]);
  });

  /**
   * A launch failure is not a read failure: it belongs to the button that produced it and must not
   * be reported as a re-read that went wrong.
   */
  it('shows a launch failure without claiming a read failed', () => {
    const wrapper = mountDetail(MakeStatus({ counts: MakeCounts(), submodules: [] }), {
      openError: 'could not run `code`',
    });

    expect(wrapper.text()).toContain('Could not open');
    expect(wrapper.text()).toContain('could not run `code`');
    expect(wrapper.text()).not.toContain('Re-read failed');
  });
});
