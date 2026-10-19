// The hierarchical area mosaic layout (APP-011 R6): the squarified treemap algorithm of Bruls,
// Huizing and van Wijk, "Squarified Treemaps" (2000), with padding, headers and an aggregate tail.

export const DEFAULT_LAYOUT = Object.freeze({
  outerPadding: 3,
  innerPadding: 1,
  minTile: 5,
  maxChildren: 96,
  rootHeader: 22,
  nestedHeader: 17,
  headerMinWidth: 44,
  headerMinBody: 15,
  maxDepth: 3,
});

const EMPTY = Object.freeze({ x: 0, y: 0, w: 0, h: 0 });

/** The worst aspect ratio of a row with area sum `sum` laid along a side of length `side`. */
function worstRatio(areas, sum, side) {
  if (!(sum > 0) || !(side > 0)) return Infinity;
  const thickness = sum / side;
  let worst = 0;
  for (const area of areas) {
    if (area === 0) continue;
    const other = area / thickness;
    worst = Math.max(worst, Math.max(thickness / other, other / thickness));
  }
  return worst;
}

/**
 * Squarified allocation of `values` (laid out in descending order) inside a width x height area.
 * The rectangles come back in the order of the input values; a nonpositive total or an empty area
 * yields empty rectangles.
 */
export function squarify(values, width, height) {
  const rects = values.map(() => ({ ...EMPTY }));
  const total = values.reduce((sum, value) => sum + (value > 0 ? value : 0), 0);
  if (!(total > 0) || !(width > 0) || !(height > 0)) return rects;
  const scale = (width * height) / total;
  const areas = values.map((value) => (value > 0 ? value * scale : 0));
  // Descending order of allocation, ties by input order.
  const order = areas.map((_, index) => index).sort((a, b) => areas[b] - areas[a] || a - b);

  let free = { x: 0, y: 0, w: width, h: height };
  let cursor = 0;
  while (cursor < order.length) {
    const side = Math.min(free.w, free.h);
    const row = [order[cursor]];
    let rowAreas = [areas[order[cursor]]];
    let sum = rowAreas[0];
    cursor++;
    while (cursor < order.length) {
      const area = areas[order[cursor]];
      const extended = [...rowAreas, area];
      if (worstRatio(extended, sum + area, side) > worstRatio(rowAreas, sum, side)) break;
      row.push(order[cursor]);
      rowAreas = extended;
      sum += area;
      cursor++;
    }
    if (free.w >= free.h) {
      // A vertical strip at the left, its items stacked top to bottom.
      const strip = Math.min(sum / free.h, free.w);
      let y = free.y;
      let remaining = free.h;
      for (const index of row) {
        const itemHeight = strip > 0 ? Math.min(areas[index] / strip, remaining) : 0;
        if (areas[index] > 0) rects[index] = { x: free.x, y, w: strip, h: itemHeight };
        y += itemHeight;
        remaining -= itemHeight;
      }
      free = { x: free.x + strip, y: free.y, w: free.w - strip, h: free.h };
    } else {
      // A horizontal strip at the top, its items placed left to right.
      const strip = Math.min(sum / free.w, free.h);
      let x = free.x;
      let remaining = free.w;
      for (const index of row) {
        const itemWidth = strip > 0 ? Math.min(areas[index] / strip, remaining) : 0;
        if (areas[index] > 0) rects[index] = { x, y: free.y, w: itemWidth, h: strip };
        x += itemWidth;
        remaining -= itemWidth;
      }
      free = { x: free.x, y: free.y + strip, w: free.w, h: free.h - strip };
    }
  }
  return rects;
}

/**
 * Lays a node out into `area` and returns the tiles in parent-before-child (paint) order. A tile
 * is `{ node, x, y, w, h, depth, header, aggregate, path }`: `header` is the header band height (0
 * for none), `aggregate` the number of merged children for the aggregate tail tile (else 0) and
 * `path` the child indices from the laid-out node down to the tile's node (`null` for the tail).
 *
 * @param {{ children?: object[], directory?: boolean }} node
 * @param {{ x: number, y: number, w: number, h: number }} area
 * @param {(node: object) => number} metric the value of a node
 * @param {Partial<typeof DEFAULT_LAYOUT>} [options]
 */
export function layoutNode(node, area, metric, options = {}) {
  const settings = { ...DEFAULT_LAYOUT, ...options };
  const tiles = [];
  place(node, area, metric, settings, 0, tiles);
  return tiles;
}

function place(node, area, metric, settings, depth, tiles, parentPath = []) {
  if (!node.directory || !node.children || node.children.length === 0 || !(area.w > 0) || !(area.h > 0)) return;
  const entries = node.children
    .map((child) => ({ child, value: metric(child) }))
    .filter((entry) => entry.value > 0)
    .sort((a, b) => b.value - a.value);
  const kept = entries.slice(0, settings.maxChildren);
  const rest = entries.slice(settings.maxChildren);
  const items = kept.map((entry) => ({ node: entry.child, value: entry.value, aggregate: 0, index: node.children.indexOf(entry.child) }));
  if (rest.length > 0) items.push({ node: null, value: rest.reduce((sum, entry) => sum + entry.value, 0), aggregate: rest.length, index: -1 });
  const rects = squarify(items.map((item) => item.value), area.w, area.h);
  const padding = depth === 0 ? settings.outerPadding : settings.innerPadding;
  const headerHeight = depth === 0 ? settings.rootHeader : settings.nestedHeader;

  items.forEach((item, index) => {
    const rect = rects[index];
    const inset = {
      x: area.x + rect.x + padding,
      y: area.y + rect.y + padding,
      w: Math.max(0, rect.w - 2 * padding),
      h: Math.max(0, rect.h - 2 * padding),
    };
    if (inset.w < settings.minTile || inset.h < settings.minTile) return;
    if (item.aggregate > 0) {
      tiles.push({ node: null, ...inset, depth, header: 0, aggregate: item.aggregate, path: null });
      return;
    }
    const withHeader =
      item.node.directory === true &&
      item.node.children?.length > 0 &&
      depth + 1 < settings.maxDepth &&
      inset.w >= settings.headerMinWidth &&
      inset.h - headerHeight >= settings.headerMinBody;
    const path = [...parentPath, item.index];
    tiles.push({ node: item.node, ...inset, depth, header: withHeader ? headerHeight : 0, aggregate: 0, path });
    if (withHeader) {
      place(item.node, { x: inset.x, y: inset.y + headerHeight, w: inset.w, h: inset.h - headerHeight }, metric, settings, depth + 1, tiles, path);
    }
  });
}

/**
 * The tile under a point, hit-tested in reverse paint order (the last emitted tile first) with
 * half-open rectangles (`x <= px < x + w`, `y <= py < y + h`). `null` over empty space.
 */
export function hitTest(tiles, px, py) {
  for (let index = tiles.length - 1; index >= 0; index--) {
    const tile = tiles[index];
    if (px >= tile.x && px < tile.x + tile.w && py >= tile.y && py < tile.y + tile.h) return { tile, index };
  }
  return null;
}
