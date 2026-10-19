// Navigation state of the mosaic (APP-011 R4, R5): a path of child indices from the root, the
// selection and the hover, all bound to the tree reference they index. State that belongs to a
// different tree is treated as reset, because its indices refer to different data.

export const MODES = Object.freeze(["size", "files", "age"]);
export const MIN_DEPTH = 1;
export const MAX_DEPTH = 6;

export const freshNav = (tree) => ({ tree, path: [], selection: null, hover: null, mode: "size", depth: 3, apparent: false, hidden: true });

/** The nav state valid for `tree`: stored state of another tree reference resets to the root. */
export function navFor(nav, tree) {
  return nav.tree === tree ? nav : { ...freshNav(tree), mode: nav.mode, depth: nav.depth, apparent: nav.apparent, hidden: nav.hidden };
}

export function nodeAt(root, path) {
  let node = root;
  for (const index of path) {
    node = node?.children?.[index];
    if (!node) return null;
  }
  return node;
}

const sameIndices = (a, b) => a === b || (a !== null && b !== null && a.length === b.length && a.every((value, index) => value === b[index]));
export { sameIndices };

/** The value a tile is sized by in a mode. */
export function metricFor(nav) {
  if (nav.mode === "files") return (node) => node.files;
  return nav.apparent ? (node) => node.apparent : (node) => node.allocated;
}

/** The age bucket (0 newest .. 4 oldest) of a modification time. */
export function ageBucket(mtime, now) {
  const days = (now - mtime) / 86_400_000;
  if (days < 1) return 0;
  if (days < 7) return 1;
  if (days < 30) return 2;
  if (days < 365) return 3;
  return 4;
}

/**
 * Applies a press on a tile (or on empty space when `tile` is null). A folder is entered, a file
 * navigates to its folder and is selected, the aggregate tail and empty space clear the selection.
 */
export function pressTile(nav, tree, tile) {
  if (!tile || tile.aggregate > 0) return { ...nav, selection: null };
  const absolute = [...nav.path, ...tile.path];
  if (tile.node.directory) return { ...nav, path: absolute, selection: null, hover: null };
  return { ...nav, path: absolute.slice(0, -1), selection: absolute, hover: null };
}

/** Keyboard handling. Returns the next nav state and whether a rescan was requested. */
export function handleKey(nav, tree, key) {
  const name = String(key).toLowerCase();
  switch (name) {
    case "backspace":
    case "escape":
      if (nav.path.length === 0) return { nav, rescan: false };
      return { nav: { ...nav, selection: nav.path, path: nav.path.slice(0, -1), hover: null }, rescan: false };
    case "enter": {
      if (!nav.selection) return { nav, rescan: false };
      const node = nodeAt(tree, nav.selection);
      if (!node) return { nav, rescan: false };
      return {
        nav: node.directory ? { ...nav, path: nav.selection, selection: null, hover: null } : { ...nav, path: nav.selection.slice(0, -1), hover: null },
        rescan: false,
      };
    }
    case "[":
    case "bracketleft":
      return { nav: { ...nav, depth: Math.max(MIN_DEPTH, nav.depth - 1) }, rescan: false };
    case "]":
    case "bracketright":
      return { nav: { ...nav, depth: Math.min(MAX_DEPTH, nav.depth + 1) }, rescan: false };
    case "t":
      return { nav: { ...nav, mode: MODES[(MODES.indexOf(nav.mode) + 1) % MODES.length] }, rescan: false };
    case "a":
      return { nav: { ...nav, apparent: !nav.apparent }, rescan: false };
    case "h":
      return { nav: { ...nav, hidden: !nav.hidden }, rescan: true };
    case "r":
      return { nav, rescan: true };
    default:
      return { nav, rescan: false };
  }
}
