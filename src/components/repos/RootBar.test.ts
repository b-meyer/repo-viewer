import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import RootBar from './RootBar.vue';

function mountBar(props: Partial<InstanceType<typeof RootBar>['$props']> = {}) {
  return mount(RootBar, { props: { roots: [], scanning: false, disabled: false, ...props } });
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
});
