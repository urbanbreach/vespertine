// The synchronized multi-pane editing surface (APP-012): a ruler, a frozen track-header column
// and a two-dimensional clip grid that share one application-owned viewport, with captured
// dragging, snapping, trimming, scrubbing, marquee selection, wheel panning and pointer-anchored
// zoom, and culling of offscreen clips.

import { useWindowSize } from "@vespertine/react";
import { React, h } from "../h.js";
import { palette } from "../theme.js";
import {
  CLIP_INSET,
  FOOTER_HEIGHT,
  HANDLE_WIDTH,
  HEADER_WIDTH,
  RULER_HEIGHT,
  SLIDER_WIDTH,
  rowLayout,
  scaleToSlider,
  tickStep,
  visibleClips,
} from "./geometry.js";
import { generateProject } from "./project.js";
import { SLIDER_LEFT, activePreview, initialState, metricsOf, reduce } from "./state.js";

/** Render counters (clip layer and clip renders), for the memoization acceptance test. */
export const editorStats = { clipLayerRenders: 0, clipRenders: 0 };

const colors = palette();
const none = { pointerEvents: "none" };
const auto = { pointerEvents: "auto" };

const timeLabel = (seconds) => {
  const minutes = Math.floor(seconds / 60);
  const rest = seconds - minutes * 60;
  return `${minutes}:${(rest < 10 ? "0" : "") + (Number.isInteger(rest) ? rest : rest.toFixed(1))}`;
};

/** The pointer handler triple every draggable surface registers on the pressed element itself. */
function pointerHandlers(dispatch, press) {
  return {
    onPointerDown: (event) => {
      if (event.button !== undefined && event.button !== 0) return;
      dispatch(press(event));
    },
    onPointerMove: (event) => dispatch({ type: "move", x: event.x, y: event.y }),
    onPointerUp: (event) => dispatch({ type: "release", x: event.x, y: event.y }),
  };
}

const flags = (event) => ({ x: event.x, y: event.y, shift: event.shift, cmd: event.cmd });

const ClipView = React.memo(function ClipView({ clip, top, height, scale, selected, dragging, handlers }) {
  editorStats.clipRenders++;
  const width = Math.max(1, clip.duration * scale);
  const innerHeight = Math.max(0, height - 2 * CLIP_INSET);
  return h(
    "div",
    {
      testId: `clip-${clip.id}`,
      ...handlers.body,
      style: {
        position: "absolute",
        left: clip.start * scale,
        top: top + CLIP_INSET,
        width,
        height: innerHeight,
        borderRadius: 3,
        borderWidth: selected ? 2 : 1,
        borderColor: selected ? colors.accent : colors.border,
        backgroundColor: dragging ? colors.hover : colors.raised,
        overflow: "hidden",
        ...auto,
      },
    },
    h("text", { style: { fontSize: 11, color: colors.text, ...none }, testId: `clip-label-${clip.id}` }, clip.id),
    h("div", { testId: `trim-start-${clip.id}`, ...handlers.start, style: { position: "absolute", left: 0, top: 0, width: HANDLE_WIDTH, height: innerHeight, backgroundColor: colors.accent, ...auto } }),
    h("div", { testId: `trim-end-${clip.id}`, ...handlers.end, style: { position: "absolute", left: Math.max(0, width - HANDLE_WIDTH), top: 0, width: HANDLE_WIDTH, height: innerHeight, backgroundColor: colors.accent, ...auto } }),
  );
});

/** The mounted clips; memoized so a pure pan with unchanged membership never re-renders it. */
const ClipLayer = React.memo(function ClipLayer({ clips, rowsById, scale, selection, preview, previewClipId, handlersFor }) {
  editorStats.clipLayerRenders++;
  return clips.map((clip) => {
    const shown = clip.id === previewClipId && preview ? { ...clip, trackId: preview.trackId, start: preview.start, duration: preview.duration } : clip;
    const row = rowsById.get(shown.trackId);
    return h(ClipView, {
      key: clip.id,
      clip: shown,
      top: row.top,
      height: row.height,
      scale,
      selected: selection.includes(clip.id),
      dragging: clip.id === previewClipId,
      handlers: handlersFor(clip.id),
    });
  });
});

function Ticks({ duration, scale }) {
  const step = tickStep(scale);
  const ticks = [];
  for (let t = 0; t <= duration + 1e-9; t += step) {
    ticks.push(
      h(
        "div",
        { key: t, testId: `tick-${t}`, style: { position: "absolute", left: t * scale, top: 0, width: 1, height: RULER_HEIGHT, backgroundColor: colors.border, ...none } },
        h("text", { style: { position: "absolute", left: 4, top: 6, fontSize: 10, color: colors.textSecondary, ...none } }, timeLabel(t)),
      ),
    );
  }
  return ticks;
}

/**
 * @param {{ trackCount?: number, durationSeconds?: number, viewportWidth?: number,
 *           viewportHeight?: number, cull?: boolean, seed?: number }} props
 */
export function Editor({ trackCount, durationSeconds, viewportWidth, viewportHeight, cull = true, seed = 1 }) {
  // Explicit sizes keep tests independent of the window; otherwise the window size is observed.
  const windowSize = useWindowSize();
  const width = viewportWidth ?? windowSize.width;
  const height = viewportHeight ?? windowSize.height;
  const [state, dispatch] = React.useReducer(
    reduce,
    undefined,
    () => initialState({ project: generateProject({ seed, trackCount, durationSeconds }), viewportWidth: width, viewportHeight: height }),
  );
  React.useEffect(() => {
    if (state.viewport.width !== width || state.viewport.height !== height) dispatch({ type: "viewport", width, height });
  }, [width, height, state.viewport.width, state.viewport.height]);

  const { view, project, collapsed, selection, gesture } = state;
  const metrics = metricsOf(state);
  const rowsById = React.useMemo(() => new Map(metrics.rows.map((row) => [row.track.id, row])), [collapsed, project.tracks]);

  // Gesture handlers keep stable identities for the lifetime of the component.
  const handlerCache = React.useRef(new Map());
  const stable = React.useRef(null);
  if (stable.current === null) {
    stable.current = {
      ruler: pointerHandlers(dispatch, (event) => ({ type: "pressRuler", ...flags(event) })),
      grid: pointerHandlers(dispatch, (event) => ({ type: "pressGrid", ...flags(event) })),
      slider: pointerHandlers(dispatch, (event) => ({ type: "pressSlider", ...flags(event) })),
      handlersFor(clipId) {
        let entry = handlerCache.current.get(clipId);
        if (!entry) {
          const part = (name) => pointerHandlers(dispatch, (event) => ({ type: "pressClip", clipId, part: name, ...flags(event) }));
          entry = { body: part("body"), start: part("start"), end: part("end") };
          handlerCache.current.set(clipId, entry);
        }
        return entry;
      },
    };
  }
  const { ruler, grid, slider, handlersFor } = stable.current;
  const onWheel = React.useCallback((event) => dispatch({ type: "wheel", ...event }), []);

  // The mounted clips change identity only when their membership changes.
  const visible = visibleClips(project, collapsed, metrics.rows, view, metrics, cull);
  const signature = visible.map((clip) => clip.id).join(",");
  const membershipRef = React.useRef({ signature: null, clips: null, project: null });
  if (membershipRef.current.signature !== signature || membershipRef.current.project !== project) {
    membershipRef.current = { signature, clips: visible, project };
  }
  const mounted = membershipRef.current.clips;

  const preview = activePreview(state);
  const previewClipId = gesture?.clip && gesture.moved ? gesture.clip.id : null;
  const selectedClip = selection.length > 0 ? project.clips.find((clip) => clip.id === selection[selection.length - 1]) : null;
  const guideX = preview?.guide != null ? preview.guide * view.scale : null;
  const { contentHeight } = rowLayout(project.tracks, collapsed);
  const marquee = gesture?.kind === "marquee" && gesture.moved ? gesture : null;
  const thumb = scaleToSlider(view.scale) * SLIDER_WIDTH;

  const fixed = (left, top, w, hh, extra) => ({ position: "absolute", left, top, width: w, height: hh, overflow: "hidden", ...extra });

  return h(
    "div",
    { testId: "editor", style: { position: "relative", width, height, backgroundColor: colors.surface, overflow: "hidden" } },
    // Readouts for tests and screen readers.
    h(
      "div",
      { testId: "readouts", style: { position: "absolute", left: 8, top: 4, flexDirection: "column", ...none } },
      h("text", { testId: "readout-offset", style: { fontSize: 11, color: colors.textSecondary } }, `offset ${Math.round(view.offsetX)},${Math.round(view.offsetY)}`),
      h("text", { testId: "readout-scale", style: { fontSize: 11, color: colors.textSecondary } }, `scale ${view.scale.toFixed(2)}`),
      h("text", { testId: "readout-playhead", style: { fontSize: 11, color: colors.textSecondary } }, `playhead ${state.playhead.toFixed(2)}`),
      h("text", { testId: "readout-mounted", style: { fontSize: 11, color: colors.textSecondary } }, `clips ${mounted.length}/${project.clips.length}`),
      h(
        "text",
        { testId: "readout-selection", style: { fontSize: 11, color: colors.textSecondary } },
        selectedClip ? `selected ${selectedClip.id} ${selectedClip.trackId} ${selectedClip.start.toFixed(2)} ${selectedClip.duration.toFixed(2)}` : "selected none",
      ),
      h("text", { testId: "readout-history", style: { fontSize: 11, color: colors.textSecondary } }, `history ${state.history.join(" ")}`),
    ),
    // An ordinary two-axis native scroller beside the editing surface.
    h(
      "div",
      { testId: "plain-scroller", style: fixed(Math.max(260, width - 260), 8, 240, 100, { overflow: "scroll", backgroundColor: colors.raised, borderRadius: 4 }) },
      h("div", { style: { width: 480, height: 300, backgroundColor: colors.sidebar } }, h("text", { style: { fontSize: 11, color: colors.textTertiary } }, "native scroller")),
    ),
    // Ruler.
    h("div", { testId: "ruler-corner", style: fixed(0, metrics.gridTop - RULER_HEIGHT, HEADER_WIDTH, RULER_HEIGHT, { backgroundColor: colors.sidebar }) }),
    h(
      "div",
      { testId: "ruler", ...ruler, onWheel, style: fixed(metrics.gridLeft, metrics.gridTop - RULER_HEIGHT, metrics.gridWidth, RULER_HEIGHT, { backgroundColor: colors.sidebar }) },
      h("div", { testId: "ruler-ticks", style: { position: "absolute", left: -view.offsetX, top: 0, width: metrics.contentWidth, height: RULER_HEIGHT, ...none } }, h(Ticks, { duration: project.duration, scale: view.scale })),
      h("div", { testId: "playhead-ruler", style: { position: "absolute", left: state.playhead * view.scale - view.offsetX, top: 0, width: 1, height: RULER_HEIGHT, backgroundColor: colors.accent, ...none } }),
    ),
    // Track headers: frozen column translated by the shared vertical offset.
    h(
      "div",
      { testId: "headers", onWheel, style: fixed(0, metrics.gridTop, HEADER_WIDTH, metrics.gridHeight, { backgroundColor: colors.sidebar }) },
      h(
        "div",
        { testId: "headers-content", style: { position: "absolute", left: 0, top: -view.offsetY, width: HEADER_WIDTH, height: contentHeight, ...none } },
        metrics.rows.map((row) =>
          h(
            "div",
            { key: row.track.id, testId: `header-${row.track.id}`, style: { position: "absolute", left: 0, top: row.top, width: HEADER_WIDTH, height: row.height, flexDirection: "row", gap: 8, paddingLeft: 8, borderColor: colors.border, borderWidth: 1 } },
            h(
              "div",
              {
                testId: `collapse-${row.track.id}`,
                role: "button",
                "aria-label": `Toggle ${row.track.id}`,
                onClick: () => dispatch({ type: "toggleCollapse", trackId: row.track.id }),
                style: { width: 14, height: 14, backgroundColor: colors.raised, borderRadius: 2, ...auto },
              },
            ),
            h("text", { style: { fontSize: 12, color: colors.text, ...none } }, row.track.id),
          ),
        ),
      ),
    ),
    // Clip grid: the translating wrapper has pointer hits disabled, so empty grid stays reachable.
    h(
      "div",
      { testId: "grid", ...grid, onWheel, style: fixed(metrics.gridLeft, metrics.gridTop, metrics.gridWidth, metrics.gridHeight, { backgroundColor: colors.surface }) },
      h(
        "div",
        { testId: "grid-content", style: { position: "absolute", left: -view.offsetX, top: -view.offsetY, width: metrics.contentWidth, height: contentHeight, ...none } },
        h(ClipLayer, { clips: mounted, rowsById, scale: view.scale, selection, preview, previewClipId, handlersFor }),
        guideX === null ? null : h("div", { testId: "snap-guide", style: { position: "absolute", left: guideX, top: 0, width: 1, height: contentHeight, backgroundColor: colors.guide, ...none } }),
        h("div", { testId: "playhead", style: { position: "absolute", left: state.playhead * view.scale, top: 0, width: 1, height: contentHeight, backgroundColor: colors.accent, ...none } }),
      ),
      marquee
        ? h("div", {
            testId: "marquee",
            style: {
              position: "absolute",
              left: Math.min(marquee.origin.x, marquee.current.x) - metrics.gridLeft,
              top: Math.min(marquee.origin.y, marquee.current.y) - metrics.gridTop,
              width: Math.abs(marquee.current.x - marquee.origin.x),
              height: Math.abs(marquee.current.y - marquee.origin.y),
              borderColor: colors.accent,
              borderWidth: 1,
              ...none,
            },
          })
        : null,
    ),
    // Footer with the exponential zoom slider.
    h(
      "div",
      { testId: "footer", style: fixed(0, metrics.gridTop + metrics.gridHeight, width, FOOTER_HEIGHT, { backgroundColor: colors.sidebar }) },
      h(
        "div",
        { testId: "zoom-slider", ...slider, style: { position: "absolute", left: SLIDER_LEFT, top: 12, width: SLIDER_WIDTH, height: 10, backgroundColor: colors.raised, borderRadius: 5 } },
        h("div", { testId: "zoom-thumb", style: { position: "absolute", left: thumb - 4, top: -2, width: 8, height: 14, backgroundColor: colors.accent, borderRadius: 4, ...none } }),
      ),
    ),
  );
}
