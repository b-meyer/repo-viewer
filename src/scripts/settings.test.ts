import { describe, expect, it } from 'vite-plus/test';
import { DEFAULT_UI_SETTINGS, parseUiSettings } from '@/scripts/settings';

describe('parseUiSettings', () => {
  it('reads back what it was given', () => {
    const saved = {
      chips: ['dirty', 'staleFetch'],
      sortKey: 'read',
      sortDirection: 'desc',
      groupByFolder: true,
    };

    expect(parseUiSettings(saved)).toEqual(saved);
  });

  /**
   * This is the only value crossing the IPC boundary that `ts-rs` does not generate, so it is the
   * only one that arrives unvalidated — and the file is hand-editable, which is the whole reason
   * there is no settings screen. Every one of these is a real thing a hand-edit can produce.
   */
  it.each([
    ['nothing saved', null],
    ['a value of the wrong shape', 42],
    ['a string', 'chips'],
    ['an empty object', {}],
    ['an array', []],
  ])('falls back to the defaults for %s', (_label, value) => {
    expect(parseUiSettings(value)).toEqual(DEFAULT_UI_SETTINGS);
  });

  /**
   * Field by field, not all or nothing: a file with a good sort and a nonsense chip keeps the sort.
   * A user who has hand-edited one key should not lose the rest.
   */
  it('keeps the fields it can read and defaults only the ones it cannot', () => {
    const parsed = parseUiSettings({
      chips: 'dirty',
      sortKey: 'read',
      sortDirection: 'sideways',
      groupByFolder: 'yes',
    });

    expect(parsed.sortKey).toBe('read');
    expect(parsed.chips).toEqual([]);
    expect(parsed.sortDirection).toBe(DEFAULT_UI_SETTINGS.sortDirection);
    expect(parsed.groupByFolder).toBe(DEFAULT_UI_SETTINGS.groupByFolder);
  });

  it('drops an unknown chip and keeps the ones it knows', () => {
    expect(parseUiSettings({ chips: ['dirty', 'haunted', 7, null] }).chips).toEqual(['dirty']);
  });

  /**
   * The defaults are the ordering the store used before there was a sort at all, so a first run
   * looks exactly as it always has.
   */
  it('defaults to an unfiltered table ordered by name', () => {
    expect(DEFAULT_UI_SETTINGS).toEqual({
      chips: [],
      sortKey: 'name',
      sortDirection: 'asc',
      groupByFolder: false,
    });
  });

  /**
   * A parsed object must not share a reference with the defaults, or the first chip a user clicks
   * would edit what every later fallback returns.
   */
  it('never hands back the defaults object itself', () => {
    const parsed = parseUiSettings(null);
    parsed.chips.push('dirty');

    expect(DEFAULT_UI_SETTINGS.chips).toEqual([]);
  });
});
