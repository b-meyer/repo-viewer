import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import AppInput from './AppInput.vue';

function mountInput(value = '', extra: { disabled?: boolean } = {}) {
  return mount(AppInput, {
    props: {
      modelValue: value,
      label: 'Search repositories',
      placeholder: 'Search…',
      icon: 'bi-search',
      disabled: extra.disabled ?? false,
    },
  });
}

describe('AppInput', () => {
  /**
   * The field has no visible label, so the accessible one is the only name it has.
   */
  it('labels the field for assistive technology', () => {
    const wrapper = mountInput();

    expect(wrapper.find('input').attributes('aria-label')).toBe('Search repositories');
    expect(wrapper.find('input').attributes('placeholder')).toBe('Search…');
  });

  it('emits what was typed', async () => {
    const wrapper = mountInput();

    await wrapper.find('input').setValue('alpha');

    expect(wrapper.emitted('update:modelValue')).toEqual([['alpha']]);
  });

  /**
   * A clear button with nothing to clear is a control that does nothing, so it is not drawn until
   * there is something in the field.
   */
  it('offers a clear button only when there is something to clear', () => {
    expect(mountInput('').findAll('button')).toHaveLength(0);
    expect(mountInput('alpha').findAll('button')).toHaveLength(1);
  });

  it('empties the field when cleared', async () => {
    const wrapper = mountInput('alpha');

    await wrapper.find('button').trigger('click');

    expect(wrapper.emitted('update:modelValue')).toEqual([['']]);
  });

  /**
   * Disabled means every part of the control, including the one that would clear it.
   */
  it('disables the field and hides the clear button', () => {
    const wrapper = mountInput('alpha', { disabled: true });

    expect(wrapper.find('input').attributes('disabled')).toBeDefined();
    expect(wrapper.findAll('button')).toHaveLength(0);
  });
});
