// The editing surface's state machine (APP-012 R1, R3-R7): one reducer owns the viewport, the
// playhead, the selection, collapsed tracks, the project and the active gesture. Every geometry
// computation of a gesture uses its press-time snapshot plus the total pointer displacement.

import {
  DEFAULT_PLAYHEAD,
  DEFAULT_SCALE,
  HISTORY_LENGTH,
  MOVE_THRESHOLD,
  SLIDER_WIDTH,
  applyWheel,
  clamp,
  layoutMetrics,
  marqueeSelection,
  previewClip,
  sliderToScale,
  timeAt,
} from "./geometry.js";

/** Where the footer zoom slider starts (window x); its thumb position is `scaleToSlider`. */
export const SLIDER_LEFT = 8;

export function initialState({ project, viewportWidth, viewportHeight }) {
  return {
    project,
    viewport: { width: viewportWidth, height: viewportHeight },
    view: { scale: DEFAULT_SCALE, offsetX: 0, offsetY: 0 },
    playhead: DEFAULT_PLAYHEAD,
    selection: [],
    collapsed: new Set(),
    gesture: null,
    history: [],
  };
}

export function metricsOf(state, scale = state.view.scale) {
  return layoutMetrics({
    viewportWidth: state.viewport.width,
    viewportHeight: state.viewport.height,
    tracks: state.project.tracks,
    collapsed: state.collapsed,
    duration: state.project.duration,
    scale,
  });
}

const pushHistory = (history, entry) => [...history, entry].slice(-HISTORY_LENGTH);

const withModifier = (event) => Boolean(event.shift || event.cmd);

function clampView(state, view) {
  const metrics = metricsOf(state, view.scale);
  return {
    scale: view.scale,
    offsetX: clamp(view.offsetX, 0, metrics.maxOffsetX),
    offsetY: clamp(view.offsetY, 0, metrics.maxOffsetY),
  };
}

function beginGesture(state, kind, event, extra = {}) {
  const gesture = {
    kind,
    origin: { x: event.x, y: event.y },
    current: { x: event.x, y: event.y },
    moved: false,
    clip: null,
    scale: state.view.scale,
    offsetX: state.view.offsetX,
    offsetY: state.view.offsetY,
    ...extra,
  };
  const label = gesture.clip ? `dragstart:${kind}:${gesture.clip.id}` : `dragstart:${kind}`;
  return { gesture, history: pushHistory(state.history, label) };
}

function pressClip(state, action) {
  const clip = state.project.clips.find((candidate) => candidate.id === action.clipId);
  if (!clip) return state;
  const kind = action.part === "start" ? "trim-start" : action.part === "end" ? "trim-end" : "move";
  const selected = state.selection.includes(clip.id);
  const selection = withModifier(action)
    ? selected
      ? state.selection.filter((id) => id !== clip.id)
      : [...state.selection, clip.id]
    : [clip.id];
  return { ...state, selection, ...beginGesture(state, kind, action, { clip: { ...clip } }) };
}

function updateGesture(state, action) {
  const { gesture } = state;
  if (!gesture) return state;
  const current = { x: action.x, y: action.y };
  const moved = gesture.moved || Math.abs(current.x - gesture.origin.x) > MOVE_THRESHOLD || Math.abs(current.y - gesture.origin.y) > MOVE_THRESHOLD;
  let next = { ...state, gesture: { ...gesture, current, moved } };
  if (gesture.kind === "scrub") {
    // A scrub follows the current offset and scale.
    next = { ...next, playhead: timeAt(current.x, state.view, metricsOf(state), state.project.duration) };
  } else if (gesture.kind === "zoom") {
    next = { ...next, view: zoomTo(state, current.x) };
  }
  return next;
}

function zoomTo(state, pointerX) {
  const scale = sliderToScale((pointerX - SLIDER_LEFT) / SLIDER_WIDTH);
  return clampView(state, { ...state.view, scale });
}

function finishGesture(state, action) {
  const { gesture } = state;
  if (!gesture) return state;
  const updated = updateGesture(state, action);
  const ended = updated.gesture;
  let next = { ...updated, gesture: null, history: pushHistory(updated.history, `dragend:${ended.kind}`) };
  if (!ended.moved) return next;
  if (ended.kind === "move" || ended.kind === "trim-start" || ended.kind === "trim-end") {
    const metrics = metricsOf(state, ended.scale);
    const preview = previewClip(ended, state.project, state.playhead, metrics.rows);
    next = {
      ...next,
      project: {
        ...state.project,
        clips: state.project.clips.map((clip) =>
          clip.id === ended.clip.id ? { ...clip, trackId: preview.trackId, start: preview.start, duration: preview.duration } : clip,
        ),
      },
    };
  } else if (ended.kind === "marquee") {
    const metrics = metricsOf(state, ended.scale);
    next = {
      ...next,
      selection: marqueeSelection(
        state.project,
        state.collapsed,
        metrics.rows,
        { scale: ended.scale, offsetX: ended.offsetX, offsetY: ended.offsetY },
        metrics,
        { x1: ended.origin.x, y1: ended.origin.y, x2: ended.current.x, y2: ended.current.y },
      ),
    };
  }
  return next;
}

/** @param {ReturnType<typeof initialState>} state */
export function reduce(state, action) {
  switch (action.type) {
    case "viewport": {
      const resized = { ...state, viewport: { width: action.width, height: action.height } };
      return { ...resized, view: clampView(resized, state.view) };
    }
    case "wheel": {
      const metrics = metricsOf(state);
      const view = applyWheel(state.view, metrics, action, state.project.duration);
      return view.scale === state.view.scale && view.offsetX === state.view.offsetX && view.offsetY === state.view.offsetY
        ? state
        : { ...state, view };
    }
    case "toggleCollapse": {
      const collapsed = new Set(state.collapsed);
      if (!collapsed.delete(action.trackId)) collapsed.add(action.trackId);
      const next = { ...state, collapsed };
      return { ...next, view: clampView(next, state.view) };
    }
    case "pressClip":
      return pressClip(state, action);
    case "pressRuler": {
      const playhead = timeAt(action.x, state.view, metricsOf(state), state.project.duration);
      return { ...state, playhead, ...beginGesture(state, "scrub", action) };
    }
    case "pressGrid":
      return {
        ...state,
        selection: withModifier(action) ? state.selection : [],
        ...beginGesture(state, "marquee", action),
      };
    case "pressSlider": {
      const zoomed = { ...state, view: zoomTo(state, action.x) };
      return { ...zoomed, ...beginGesture(zoomed, "zoom", action) };
    }
    case "move":
      return updateGesture(state, action);
    case "release":
      return finishGesture(state, action);
    default:
      return state;
  }
}

/** The previewed geometry of the clip being moved or trimmed, or null. */
export function activePreview(state) {
  const gesture = state.gesture;
  if (!gesture || !gesture.clip || !gesture.moved) return null;
  return previewClip(gesture, state.project, state.playhead, metricsOf(state, gesture.scale).rows);
}
