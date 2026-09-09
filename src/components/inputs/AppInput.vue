<template>
  <label class="border-input text-14 bg-gray-25 flex h-28 items-center rounded px-8">
    <i v-if="icon" :class="`bi ${icon} text-12 mr-6 text-gray-500`" />
    <input
      v-model="model"
      type="text"
      class="min-w-0 flex-1 bg-transparent text-gray-900 outline-none placeholder:text-gray-500"
      :placeholder="placeholder"
      :aria-label="label"
      :disabled="disabled"
    />
    <!-- Only rendered when there is something to clear, so it never sits there doing nothing. -->
    <button
      v-if="model !== '' && !disabled"
      type="button"
      class="text-12 ml-6 cursor-pointer text-gray-500 hover:text-gray-900"
      :aria-label="`Clear ${label}`"
      @click="Clear"
    >
      <i class="bi bi-x-circle-fill" />
    </button>
  </label>
</template>

<script setup lang="ts">
/**
 * A single-line text input.
 *
 * Wrapped rather than used raw for the reason every `App*` component exists: the `border-input`
 * utility is a bundle of focus, hover and invalid states that has to be applied together, and a
 * call site reaching for a bare `<input>` gets none of them.
 *
 * `reka-ui` has no primitive for this — a text field has no behaviour to model — so this wraps the
 * element itself. Width comes from the call site's own `class`, which Vue merges onto the root.
 */

/// Setup
const model = defineModel<string>({ required: true });

withDefaults(
  defineProps<{
    /**
     * What the field is for. Becomes the input's `aria-label` and the clear button's, so it is
     * required: this input has no visible label of its own.
     *
     * Not called `ariaLabel`, deliberately — `aria-label` is a real HTML attribute, and a prop of
     * that name is resolved as the attribute instead at the call site.
     */
    label: string;
    /**
     * Hint text shown while the field is empty.
     */
    placeholder?: string;
    /**
     * A bootstrap-icons class shown at the start of the field, e.g. `bi-search`.
     */
    icon?: string;
    /**
     * Whether the field is unavailable.
     */
    disabled?: boolean;
  }>(),
  { placeholder: '', icon: '', disabled: false },
);

/// Methods
/**
 * Empties the field.
 *
 * A method rather than an inline assignment so the template stays declarative, and because clearing
 * is the one interaction here that is not the browser's own.
 */
function Clear(): void {
  model.value = '';
}
</script>
