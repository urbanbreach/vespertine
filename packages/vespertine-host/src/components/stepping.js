// Keyboard stepping and search ranking shared by menus, selects, comboboxes and pickers
// (CMP-002 R22, R23, R25).

const ARROWS = { up: "up", down: "down", arrowup: "up", arrowdown: "down" };

/**
 * Classifies a key event: `up`, `down`, `home`, `end`, `enter`, `secondary-accept` (ctrl or
 * cmd with enter), `escape`, `backspace` or `other`. Ctrl-N is down and Ctrl-P is up unless
 * `chords` is false (adapters that only bind arrows).
 */
export function classifyKey(event, { chords = true } = {}) {
  const key = String(event.key ?? "").toLowerCase();
  if (key === "enter") return event.ctrl || event.cmd ? "secondary-accept" : "enter";
  if (chords && event.ctrl && !event.alt && !event.cmd && !event.shift) {
    if (key === "n") return "down";
    if (key === "p") return "up";
  }
  if (event.ctrl || event.alt || event.cmd) return "other";
  if (key in ARROWS) return ARROWS[key];
  if (key === "home" || key === "end" || key === "escape" || key === "backspace") return key;
  return "other";
}

/**
 * Steps an active index through `available` (a list of booleans, in mounted order) by one in
 * `direction` (+1 or -1) with wraparound. Without an active row forward enters at the first
 * available row and backward at the last. Unavailable rows are skipped in the direction of
 * travel; a list without available rows yields `null`.
 */
export function stepIndex(available, active, direction) {
  const count = available.length;
  if (count === 0 || !available.some(Boolean)) return null;
  const forward = direction >= 0;
  let index = active === null || active === undefined || active < 0 || active >= count ? null : active;
  if (index === null) index = forward ? -1 : count;
  for (let visited = 0; visited < count; visited++) {
    index = (index + (forward ? 1 : -1) + count) % count;
    if (available[index]) return index;
  }
  return null;
}

/** The first (`"first"`) or last (`"last"`) available index, or `null`. */
export function edgeIndex(available, edge) {
  if (edge === "first") {
    const index = available.findIndex(Boolean);
    return index === -1 ? null : index;
  }
  for (let index = available.length - 1; index >= 0; index--) if (available[index]) return index;
  return null;
}

/**
 * Filters and ranks `labels` by `query`: trimmed and case-insensitive; an empty query keeps
 * every item in input order; a label starting with the query ranks 0, one containing it ranks 1
 * and the rest are dropped; results are ordered by rank, then original index. Returns the
 * original indices.
 */
export function rankMatches(labels, query) {
  const needle = String(query ?? "").trim().toLowerCase();
  if (needle === "") return labels.map((_, index) => index);
  const ranked = [];
  labels.forEach((label, index) => {
    const text = String(label).toLowerCase();
    if (text.startsWith(needle)) ranked.push([0, index]);
    else if (text.includes(needle)) ranked.push([1, index]);
  });
  ranked.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  return ranked.map(([, index]) => index);
}
