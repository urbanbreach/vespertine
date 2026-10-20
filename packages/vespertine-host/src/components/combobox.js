// The framework-neutral logic of the headless Combobox (CMP-006 R1-R8). The React and Solid
// adapters differ in documented ways, selected with `flavor` ("react" or "solid").

import { rankMatches, stepIndex } from "./stepping.js";

/** String conversion of items defaults to identity (CMP-006 R1). */
export const identityString = (item) => item;

/**
 * The items that pass the query (CMP-006 R2).
 *
 * - A `null` filter disables filtering.
 * - A custom filter function receives the item, the query and the string-conversion callback and
 *   decides per item; React hands it the raw query, Solid the trimmed lowercase one.
 * - The default filter: React trims and lowercases the query, retains substring matches and ranks
 *   prefix matches first, keeping the original order inside each rank (an empty query keeps every
 *   item); Solid retains substring matches in the original order without ranking.
 *
 * @param {readonly any[]} items
 * @param {string} query
 * @param {{ flavor: "react" | "solid", filter?: Function | null, itemToString?: (item: any) => string }} options
 */
export function filterItems(items, query, { flavor, filter, itemToString = identityString }) {
  if (filter === null) return [...items];
  const raw = String(query ?? "");
  if (typeof filter === "function") {
    const given = flavor === "react" ? raw : raw.trim().toLowerCase();
    return items.filter((item) => filter(item, given, itemToString));
  }
  if (flavor === "react") return rankMatches(items.map(itemToString), raw).map((index) => items[index]);
  const needle = raw.trim().toLowerCase();
  return items.filter((item) => String(itemToString(item)).toLowerCase().includes(needle));
}

/** The selection as an array: null and undefined are empty, a single string is one entry. */
export function selectionList(value) {
  if (value === null || value === undefined) return [];
  return Array.isArray(value) ? value : [value];
}

/** Toggles `entry` in a multiple selection, keeping the order of the array (CMP-006 R6). */
export function toggleSelection(value, entry) {
  const list = selectionList(value);
  return list.includes(entry) ? list.filter((other) => other !== entry) : [...list, entry];
}

/**
 * The text of the selected value, or `null` when the placeholder shows (CMP-006 R6). React joins
 * the converted labels with ", " and treats null, an empty string and an empty array as empty;
 * Solid joins the raw selected strings, shows the placeholder only for null and keeps an empty
 * string or an empty array as an empty display.
 */
export function displayText(value, { flavor, itemToString = identityString }) {
  if (value === null || value === undefined) return null;
  if (flavor === "react") {
    if (value === "" || (Array.isArray(value) && value.length === 0)) return null;
    return Array.isArray(value) ? value.map(itemToString).join(", ") : String(itemToString(value));
  }
  return Array.isArray(value) ? value.join(", ") : String(value);
}

/** The first of `strings` that is not in the `disabled` set, or `null`. */
export function firstEnabled(strings, disabled) {
  return strings.find((entry) => !disabled.has(entry)) ?? null;
}

/**
 * The highlight after an edit (CMP-006 R4): the first enabled filtered item when `autoHighlight`
 * is truthy (the public value "always" included), otherwise none.
 */
export function highlightAfterEdit({ autoHighlight, items, query, filter, flavor, itemToString = identityString, disabled }) {
  if (!autoHighlight) return null;
  return firstEnabled(filterItems(items, query, { flavor, filter, itemToString }).map(itemToString), disabled);
}

/**
 * Moves the highlight one step through the filtered strings with wraparound, skipping disabled
 * entries (CMP-006 R5, CMP-002 R23); without a highlight forward starts at the first entry and
 * reverse at the last. Returns the new highlight (the current one when nothing is available).
 *
 * @param {readonly string[]} strings the filtered items as strings
 * @param {Set<string>} disabled
 * @param {string | null} current
 * @param {1 | -1} direction
 */
export function stepHighlight(strings, disabled, current, direction) {
  const at = current === null || current === undefined ? -1 : strings.indexOf(current);
  const next = stepIndex(
    strings.map((entry) => !disabled.has(entry)),
    at === -1 ? null : at,
    direction,
  );
  return next === null ? (current ?? null) : strings[next];
}
