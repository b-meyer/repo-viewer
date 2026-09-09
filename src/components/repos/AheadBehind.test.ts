import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import { AHEAD_BEHIND_CAP } from '@/scripts/utils';
import AheadBehind from './AheadBehind.vue';

const NOW = 1_700_000_000_000;
const DAY = 24 * 60 * 60 * 1000;

function mountCell(props: Partial<InstanceType<typeof AheadBehind>['$props']> = {}) {
  return mount(AheadBehind, {
    props: {
      ahead: null,
      behind: null,
      upstream: 'origin/main',
      lastFetchedMs: NOW - DAY,
      now: NOW,
      ...props,
    },
  });
}

describe('AheadBehind', () => {
  it('says there is no upstream, with no counts and nothing to be stale about', () => {
    const wrapper = mountCell({ upstream: null });

    expect(wrapper.text()).toContain('no upstream');
    expect(wrapper.text()).not.toContain('fetched');
  });

  /**
   * Configured upstream, no remote-tracking ref: a distinct state from being in sync.
   */
  it('distinguishes an untracked upstream from being in sync', () => {
    expect(mountCell({ ahead: null, behind: null }).text()).toContain('not tracked');
    expect(mountCell({ ahead: 0, behind: 0 }).text()).toContain('in sync');
  });

  it('renders divergence in both directions', () => {
    const wrapper = mountCell({ ahead: 12, behind: 3 });

    expect(wrapper.text()).toContain('↑12');
    expect(wrapper.text()).toContain('↓3');
  });

  it('marks a capped count as a lower bound rather than an exact one', () => {
    const wrapper = mountCell({ ahead: AHEAD_BEHIND_CAP, behind: 0 });

    expect(wrapper.text()).toContain('↑1000+');
  });

  it('names the never-fetched case loudly', () => {
    const wrapper = mountCell({ ahead: 4, behind: 0, lastFetchedMs: null });

    expect(wrapper.text()).toContain('never fetched');
    expect(wrapper.html()).toContain('text-orange-600');
  });

  /**
   * The four staleness bands must be visually distinct, because the counts above them are only as
   * fresh as the fetch that populated them. A fresh fetch and a year-old one rendering the same
   * colour would make the loudest signal in the app silent.
   *
   * This asserts the _class chosen per band_, which is the logic. It cannot assert that the class
   * resolves to a colour — jsdom applies no Tailwind — so the tokens themselves are verified by
   * their presence in `src/styles/theme.css`.
   */
  it('colours each staleness band differently', () => {
    const tone = (lastFetchedMs: number | null) => {
      const html = mountCell({ ahead: 1, behind: 0, lastFetchedMs }).html();
      return ['text-gray-500', 'text-orange-600', 'text-red-600'].filter((cls) =>
        html.includes(cls),
      );
    };

    expect(tone(NOW - 60_000), 'a fetch minutes old is quiet').toContain('text-gray-500');
    expect(tone(NOW - 10 * DAY), 'past a week is a warning').toContain('text-orange-600');
    expect(tone(NOW - 40 * DAY), 'past a month is loud').toContain('text-red-600');
    expect(tone(null), 'never fetched is a warning').toContain('text-orange-600');

    // And the extremes must not collide: a minutes-old fetch is not red, a year-old one is not
    // quiet. This is the assertion that would have caught the bands all rendering identically.
    expect(tone(NOW - 60_000)).not.toContain('text-red-600');
    expect(tone(NOW - 365 * DAY)).not.toContain('text-gray-500');
  });

  /**
   * The invariant this component exists to enforce: ahead/behind is only as fresh as the fetch it
   * was measured against, so no rendering path shows the counts without the age beside them.
   */
  it('shows the fetch age in every case that shows a count', () => {
    const cases = [
      { ahead: 0, behind: 0 },
      { ahead: 5, behind: 0 },
      { ahead: 0, behind: 5 },
      { ahead: 5, behind: 5 },
      { ahead: AHEAD_BEHIND_CAP, behind: 0 },
      { ahead: null, behind: null },
    ];

    for (const props of cases) {
      const text = mountCell(props).text();
      expect(text, `counts without a fetch age: ${JSON.stringify(props)}`).toMatch(
        /fetched .* ago|never fetched/,
      );
    }
  });
});
