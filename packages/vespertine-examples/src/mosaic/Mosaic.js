// The externally updated hierarchical area mosaic (APP-011 R4-R6): a flat memoized list of
// absolutely positioned tiles with pointer events disabled, one container that receives pointer
// events and hit-tests in reverse paint order, keyboard navigation and a details readout.

import { React, h } from "../h.js";
import { palette } from "../theme.js";
import { hitTest, layoutNode } from "./layout.js";
import { ageBucket, freshNav, handleKey, metricFor, navFor, nodeAt, pressTile, sameIndices } from "./nav.js";

export const DETAILS_HEIGHT = 28;

const colors = palette();
const none = { pointerEvents: "none" };

/** Age buckets map to a monotonic ramp of the accent color's alpha (newest strongest). */
const AGE_ALPHA = ["FF", "CC", "99", "66", "33"];

function tileColor(tile, nav, now) {
  if (nav.mode === "age" && tile.node && !tile.node.directory) return `${colors.accent}${AGE_ALPHA[ageBucket(tile.node.mtime, now)]}`;
  if (tile.aggregate > 0) return colors.border;
  return tile.node?.directory ? colors.raised : colors.hover;
}

const Tiles = React.memo(function Tiles({ tiles, mode, now }) {
  return tiles.map((tile, index) =>
    h(
      "div",
      {
        key: index,
        testId: `tile-${index}`,
        style: {
          position: "absolute",
          left: tile.x,
          top: tile.y,
          width: tile.w,
          height: tile.h,
          backgroundColor: tileColor(tile, { mode }, now),
          borderColor: colors.border,
          borderWidth: 1,
          overflow: "hidden",
          ...none,
        },
      },
      tile.header > 0 ? h("text", { style: { fontSize: 11, color: colors.textSecondary, height: tile.header, paddingLeft: 3, ...none } }, tile.node.name) : null,
    ),
  );
});

const outline = (tile, color, testId) =>
  tile
    ? h("div", { testId, style: { position: "absolute", left: tile.x, top: tile.y, width: tile.w, height: tile.h, borderColor: color, borderWidth: 2, ...none } })
    : null;

/**
 * @param {{ store: { subscribe: Function, getSnapshot: Function, startScan: Function },
 *           width?: number, height?: number, now?: number, layout?: object }} props
 */
export function Mosaic({ store, width = 800, height = 600, now = Date.now(), layout }) {
  const snapshot = React.useSyncExternalStore(store.subscribe, store.getSnapshot);
  const [stored, setStored] = React.useState(() => freshNav(snapshot.tree));
  const nav = navFor(stored, snapshot.tree);
  const origin = React.useRef({ x: 0, y: DETAILS_HEIGHT });
  const latest = React.useRef({ nav, snapshot });
  latest.current = { nav, snapshot };

  const areaWidth = width;
  const areaHeight = Math.max(0, height - DETAILS_HEIGHT);
  const node = snapshot.tree ? nodeAt(snapshot.tree, nav.path) : null;
  const metric = React.useMemo(() => metricFor(nav), [nav.mode, nav.apparent]);

  // The tile list is memoized on the displayed node, its path, the area size, the metric and the depth.
  const tiles = React.useMemo(
    () => (node ? layoutNode(node, { x: 0, y: 0, w: areaWidth, h: areaHeight }, metric, { maxDepth: nav.depth, ...layout }) : []),
    [node, nav.path.join("."), areaWidth, areaHeight, metric, nav.depth, layout],
  );

  const tileAt = (event) => {
    const hit = hitTest(tiles, event.x - origin.current.x, event.y - origin.current.y);
    return hit ? hit.tile : null;
  };
  const pathOf = (tile) => (tile && tile.aggregate === 0 ? [...nav.path, ...tile.path] : null);
  const tileForPath = (path) => (path ? tiles.find((tile) => tile.aggregate === 0 && sameIndices([...nav.path, ...tile.path], path)) : null);

  const update = (next) => setStored(next);
  const onPointerDown = (event) => {
    if (event.button !== undefined && event.button !== 0) return;
    update(pressTile(latest.current.nav, snapshot.tree, tileAt(event)));
  };
  const onPointerMove = (event) => {
    const hovered = pathOf(tileAt(event));
    if (sameIndices(hovered, latest.current.nav.hover)) return;
    update({ ...latest.current.nav, hover: hovered });
  };
  const onKeyDown = (event) => {
    const { nav: next, rescan } = handleKey(latest.current.nav, snapshot.tree, event.key);
    if (next !== latest.current.nav) update(next);
    if (rescan) store.startScan({ includeHidden: next.hidden });
  };

  const hoveredNode = nav.hover ? nodeAt(snapshot.tree, nav.hover) : null;
  const selectedNode = nav.selection ? nodeAt(snapshot.tree, nav.selection) : null;
  const detail = hoveredNode ?? selectedNode ?? node;
  const value = detail ? metric(detail) : 0;
  const progress = snapshot.progress;

  return h(
    "div",
    { testId: "mosaic", tabIndex: 0, autoFocus: true, onKeyDown, style: { width, height, flexDirection: "column", backgroundColor: colors.surface } },
    h(
      "div",
      { testId: "mosaic-details", style: { height: DETAILS_HEIGHT, flexShrink: 0, flexDirection: "row", alignItems: "center", gap: 12, paddingLeft: 8 } },
      h("text", { testId: "details-name", style: { fontSize: 12, color: colors.text } }, detail ? detail.name : "no data"),
      h("text", { testId: "details-value", style: { fontSize: 12, color: colors.textSecondary } }, detail ? `${value}` : ""),
      h("text", { testId: "details-mode", style: { fontSize: 12, color: colors.textTertiary } }, `${nav.mode} depth ${nav.depth}`),
      h("text", { testId: "details-path", style: { fontSize: 12, color: colors.textTertiary } }, nav.path.join("/") || "/"),
      h(
        "text",
        { testId: "details-progress", style: { fontSize: 12, color: colors.textTertiary } },
        snapshot.scanning ? `scanning ${progress.files} files ${progress.directories} dirs ${progress.bytes} bytes ${progress.errors} errors` : "idle",
      ),
    ),
    h(
      "div",
      {
        testId: "mosaic-area",
        onPointerDown,
        onPointerMove,
        onLayout: (event) => {
          origin.current = { x: event.x, y: event.y };
        },
        style: { width: areaWidth, height: areaHeight, position: "relative", overflow: "hidden" },
      },
      h(Tiles, { tiles, mode: nav.mode, now }),
      outline(tileForPath(nav.selection), colors.accent, "tile-selection"),
      outline(nav.hover && !sameIndices(nav.hover, nav.selection) ? tileForPath(nav.hover) : null, colors.textSecondary, "tile-hover"),
    ),
  );
}
