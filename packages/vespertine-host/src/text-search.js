// The framework-neutral text-search helper and controller shared by the React and Solid
// adapters (APP-010 R4-R6; matching rules of TXT-009 R4, R5, R13).
//
// `findRanges` searches text groups itself (for finders that drive an explicit-range
// declaration); the controller keeps the find cursor of a native `highlight` declaration whose
// matches the native side counts and reports through `onHighlight`.

const WORD_CHARACTER = /[\p{Alphabetic}\p{N}_]/u;

function codePointBefore(text, index) {
  if (index <= 0) return null;
  const low = text.charCodeAt(index - 1);
  if (low >= 0xdc00 && low <= 0xdfff && index >= 2) {
    const high = text.charCodeAt(index - 2);
    if (high >= 0xd800 && high <= 0xdbff) return text.slice(index - 2, index);
  }
  return text[index - 1];
}

function codePointAt(text, index) {
  if (index >= text.length) return null;
  return String.fromCodePoint(text.codePointAt(index));
}

const isWordCharacter = (character) => character !== null && WORD_CHARACTER.test(character);

/**
 * The lowercased copy of `text` (Unicode default lowercasing per character, not case folding) and
 * the map from each UTF-16 unit of the copy (plus one entry past the end) to the UTF-16 offset of
 * the original character it came from.
 */
function lowercaseWithMap(text) {
  const map = [];
  let lowered = "";
  for (let index = 0; index < text.length; ) {
    const character = String.fromCodePoint(text.codePointAt(index));
    const lower = character.toLowerCase();
    for (let unit = 0; unit < lower.length; unit++) map.push(index);
    lowered += lower;
    index += character.length;
  }
  map.push(text.length);
  return { lowered, map };
}

function matchGroup(text, query, caseSensitive, wholeWord) {
  const matches = [];
  if (query === "" || text === "") return matches;
  let haystack = text;
  let needle = query;
  let map = null;
  if (!caseSensitive) {
    const lowered = lowercaseWithMap(text);
    haystack = lowered.lowered;
    map = lowered.map;
    needle = lowercaseWithMap(query).lowered;
    if (needle === "") return matches;
  }
  let position = 0;
  while (position <= haystack.length - needle.length) {
    const found = haystack.indexOf(needle, position);
    if (found === -1) break;
    const start = map ? map[found] : found;
    const end = map ? map[found + needle.length] : found + needle.length;
    if (start >= end) {
      position = found + 1;
      continue;
    }
    if (wholeWord && (isWordCharacter(codePointBefore(text, start)) || isWordCharacter(codePointAt(text, end)))) {
      position = found + 1;
      continue;
    }
    matches.push([start, end]);
    position = found + needle.length;
  }
  return matches;
}

/**
 * Finds `query` in `texts` (one string, or an ordered list of groups) and returns the matches as
 * `[start, end)` pairs of UTF-16 code units in document order. The document is the groups joined
 * by one newline; matches never cross a group boundary. Matching is case-insensitive by default
 * (Unicode default lowercasing, offsets mapped back to the original text); with `wholeWord` the
 * characters around a match must not be alphanumeric or underscore. Matches do not overlap.
 *
 * @param {string | string[]} texts
 * @param {string} query
 * @param {{ caseSensitive?: boolean, wholeWord?: boolean }} [options]
 * @returns {[number, number][]}
 */
export function findRanges(texts, query, { caseSensitive = false, wholeWord = false } = {}) {
  const groups = Array.isArray(texts) ? texts : [texts];
  const ranges = [];
  let offset = 0;
  for (const group of groups) {
    for (const [start, end] of matchGroup(group, query ?? "", caseSensitive, wholeWord)) {
      ranges.push([offset + start, offset + end]);
    }
    offset += group.length + 1;
  }
  return ranges;
}

/**
 * A find cursor over `total` matches: `next` and `previous` wrap around, the index clamps when
 * the match count shrinks, and `reset` returns to the first match.
 */
export function createSearchCursor(total = 0) {
  let count = Math.max(0, total);
  let index = 0;
  return {
    get total() {
      return count;
    },
    get index() {
      return index;
    },
    setTotal(next) {
      count = Math.max(0, next);
      index = count === 0 ? 0 : Math.min(index, count - 1);
    },
    next() {
      index = count === 0 ? 0 : (index + 1) % count;
      return index;
    },
    previous() {
      index = count === 0 ? 0 : (index - 1 + count) % count;
      return index;
    },
    goTo(target) {
      index = count === 0 ? 0 : Math.min(Math.max(Math.trunc(Number(target)) || 0, 0), count - 1);
      return index;
    },
    reset() {
      index = 0;
    },
  };
}

const DECLARATION_FIELDS = ["caseSensitive", "wholeWord", "matchIndexOffset", "color", "activeColor", "radius"];

/**
 * One text-search controller (APP-010 R4-R6). It is retained per hook or primitive instance:
 * the native side reports the match count through `props.onHighlight`, navigation moves the
 * active index (wrapping) and every change bumps a revision that subscribers observe.
 *
 * Options of `snapshot`: `query` plus the declaration fields `caseSensitive`, `wholeWord`,
 * `matchIndexOffset`, `color`, `activeColor` and `radius`. A new query resets the active index.
 */
export function createTextSearchController() {
  const cursor = createSearchCursor();
  const listeners = new Set();
  let revision = 0;
  let lastQuery = null;
  let props = null;
  let propsKey = "";

  const changed = () => {
    revision++;
    for (const listener of [...listeners]) listener();
  };

  function onHighlight(event) {
    const count = Number(event?.matchCount);
    if (!Number.isFinite(count) || count === cursor.total) return;
    cursor.setTotal(count);
    changed();
  }

  const move = (step) => {
    const before = cursor.index;
    step();
    if (cursor.index !== before) changed();
  };

  return {
    get revision() {
      return revision;
    },
    /** The number of subscribed revision listeners. */
    get listenerCount() {
      return listeners.size;
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    getRevision: () => revision,
    next: () => move(() => cursor.next()),
    previous: () => move(() => cursor.previous()),
    goTo: (index) => move(() => cursor.goTo(index)),
    /** The result for `options` and the controller's current state. */
    snapshot(options = {}) {
      const query = options.query ?? "";
      if (lastQuery !== null && lastQuery !== query) cursor.reset();
      lastQuery = query;
      const highlight = { query, activeIndex: cursor.index };
      for (const field of DECLARATION_FIELDS) {
        if (options[field] !== undefined) highlight[field] = options[field];
      }
      const key = JSON.stringify(highlight);
      if (key !== propsKey) {
        propsKey = key;
        props = { highlight, onHighlight };
      }
      return {
        props,
        total: cursor.total,
        active: cursor.index,
        next: this.next,
        previous: this.previous,
        goTo: this.goTo,
        controller: this,
      };
    },
  };
}
