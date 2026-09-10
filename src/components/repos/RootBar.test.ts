import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import RootBar from './RootBar.vue';

function mountBar(props: Partial<InstanceType<typeof RootBar>['$props']> = {}) {
  return mount(RootBar, {
    props: {
      roots: [],
      scanning: false,
      disabled: false,
      fetchable: 0,
      total: 0,
      fetching: false,
      gitMissing: null,
      ...props,
    },
  });
}

describe('RootBar', () => {
  it('cannot start a scan with no roots configured', () => {
    const scan = mountBar()
      .findAll('button')
      .find((button) => button.text().includes('Scan'));

    expect(scan?.attributes('disabled')).toBeDefined();
  });

  it('offers cancel instead of scan while a scan runs', () => {
    const labels = mountBar({ roots: ['C:/work'], scanning: true })
      .findAll('button')
      .map((button) => button.text());

    expect(labels.some((label) => label.includes('Cancel'))).toBe(true);
    expect(labels.some((label) => label.includes('Scan'))).toBe(false);
  });

  it('lists each root with a way to remove it', async () => {
    const wrapper = mountBar({ roots: ['C:/work', 'C:/other'] });

    expect(wrapper.text()).toContain('C:/work');
    await wrapper.find('[aria-label="Remove C:/work"]').trigger('click');

    expect(wrapper.emitted('remove')?.[0]).toEqual(['C:/work']);
  });

  it('disables the picker when the backend is unreachable', () => {
    const add = mountBar({ disabled: true })
      .findAll('button')
      .find((button) => button.text().includes('Add folder'));

    expect(add?.attributes('disabled')).toBeDefined();
  });

  /**
   * The button fetches what the table is showing, so the count has to be that — a label reading
   * "all" while a chip hides most of the tree would misstate how many network operations one click
   * starts.
   */
  it('says how many rows a fetch would cover', () => {
    const labels = mountBar({ fetchable: 12, total: 300 })
      .findAll('button')
      .map((button) => button.text());

    expect(labels.some((label) => label.includes('Fetch shown (12)'))).toBe(true);
  });

  it('says "all" only when nothing is filtered out', () => {
    const labels = mountBar({ fetchable: 300, total: 300 })
      .findAll('button')
      .map((button) => button.text());

    expect(labels.some((label) => label.includes('Fetch all (300)'))).toBe(true);
  });

  it('offers stop instead of fetch while a fetch runs', () => {
    const wrapper = mountBar({ fetchable: 5, total: 5, fetching: true });
    const labels = wrapper.findAll('button').map((button) => button.text());

    expect(labels.some((label) => label.includes('Stop fetching'))).toBe(true);
    expect(labels.some((label) => label.includes('Fetch all'))).toBe(false);
  });

  /**
   * §10.2: disabled with an explanation, rather than failing at click time.
   */
  it('disables fetch and says why when git is absent', () => {
    const fetch = mountBar({ fetchable: 5, total: 5, gitMissing: 'No usable `git` on PATH.' })
      .findAll('button')
      .find((button) => button.text().includes('Fetch'));

    expect(fetch?.attributes('disabled')).toBeDefined();
    expect(fetch?.attributes('title')).toContain('No usable `git`');
  });

  it('cannot fetch an empty table, and says so', () => {
    const fetch = mountBar({ fetchable: 0, total: 0 })
      .findAll('button')
      .find((button) => button.text().includes('Fetch'));

    expect(fetch?.attributes('disabled')).toBeDefined();
    expect(fetch?.attributes('title')).toContain('no repositories shown');
  });
});
