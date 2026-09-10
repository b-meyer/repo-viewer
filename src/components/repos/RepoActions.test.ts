import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import RepoActions from './RepoActions.vue';

function mountActions(props: Partial<InstanceType<typeof RepoActions>['$props']> = {}) {
  return mount(RepoActions, {
    props: {
      openError: null,
      fetchState: null,
      fetchError: null,
      gitMissing: null,
      ...props,
    },
  });
}

/**
 * The button whose label contains `label`.
 */
function button(wrapper: ReturnType<typeof mountActions>, label: string) {
  const found = wrapper.findAll('button').find((candidate) => candidate.text().includes(label));
  if (found === undefined) throw new Error(`no button labelled ${label}`);
  return found;
}

describe('RepoActions', () => {
  /**
   * Every row gets these, including one Tier 0 could not read — that row has no ahead/behind at
   * all, so a fetch is one of the few things that might help, and revealing it is how a user finds
   * out why it will not open.
   */
  it('offers every action, including fetch', () => {
    const labels = mountActions()
      .findAll('button')
      .map((each) => each.text());

    expect(labels.some((label) => label.includes('Editor'))).toBe(true);
    expect(labels.some((label) => label.includes('Reveal'))).toBe(true);
    expect(labels.some((label) => label.includes('Fetch'))).toBe(true);
  });

  it('emits fetch when the button is pressed', async () => {
    const wrapper = mountActions();

    await button(wrapper, 'Fetch').trigger('click');

    expect(wrapper.emitted('fetch')).toHaveLength(1);
  });

  /**
   * §10.2: the action disables itself with an explanation rather than failing at click time.
   */
  it('disables fetch and says why when git is absent', () => {
    const wrapper = mountActions({ gitMissing: 'No usable `git` was found on PATH.' });

    const fetch = button(wrapper, 'Fetch');
    expect(fetch.attributes('disabled')).toBeDefined();
    expect(fetch.attributes('title')).toContain('No usable `git`');
  });

  /**
   * Queued is a state worth naming: with a concurrency cap a repository can wait minutes before its
   * process starts, so a spinner for the whole wait would claim work that is not happening.
   */
  it('shows queued and running as different states', () => {
    const queued = mountActions({ fetchState: 'queued' });
    const running = mountActions({ fetchState: 'running' });

    expect(button(queued, 'Queued').attributes('disabled')).toBeDefined();
    expect(running.text()).not.toContain('Queued');
  });

  /**
   * A launch failure and a fetch failure are different operations with different fixes, so they
   * must never be readable as the same thing.
   */
  it('reports a fetch failure without claiming a launch failed', () => {
    const wrapper = mountActions({ fetchError: 'fatal: Authentication failed' });

    expect(wrapper.text()).toContain('Could not fetch');
    expect(wrapper.text()).toContain('fatal: Authentication failed');
    expect(wrapper.text()).not.toContain('Could not open');
  });

  it('shows both failures at once when both happened', () => {
    const wrapper = mountActions({ openError: 'no editor', fetchError: 'no network' });

    expect(wrapper.text()).toContain('Could not open');
    expect(wrapper.text()).toContain('Could not fetch');
  });
});
