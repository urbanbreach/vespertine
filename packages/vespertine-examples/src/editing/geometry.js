// Pure geometry of the editing surface (APP-012 R2, R3, R5-R8): layout metrics, wheel panning
// and zoom, snapping, trimming, marquee hit testing, culling and the zoom slider mapping.

export const HEADER_WIDTH = 220;
export const RULER_HEIGHT = 30;
export const FOOTER_HEIGHT = 34;
export const ROW_HEIGHTS = Object.freeze({ normal: 34, caption: 30, audio: 56, collapsed: 18 });
export const CLIP_INSET = 3;
export const HANDLE_WIDTH = 7;
export const MOVE_THRESHOLD = 3;
export const SNAP_DISTANCE = 6;
export const MIN_CLIP_DURATION = 0.4;
export const HISTORY_LENGTH = 8;
export const SLIDER_WIDTH = 120;
export const MIN_SCALE = 2;
export const MAX_SCALE = 400;
export const DEFAULT_SCALE = 24;
export const DEFAULT_PLAYHEAD = 3.5;
export const TICK_STEPS = Object.freeze([0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300]);
export const TICK_MIN_SPACING = 90;

export const clamp = (value, low, high) => Math.min(Math.max(value, low), high);

/** The height of a track's row (collapsed rows are compact). */
export function rowHeight(track, collapsed) {
  if (collapsed.has(track.id)) return ROW_HEIGHTS.collapsed;
  if (track.kind === "caption") return ROW_HEIGHTS.caption;
  if (track.kind === "audio") return ROW_HEIGHTS.audio;
  return ROW_HEIGHTS.normal;
}

/** Row tops and heights in content space, in track order. */
export function rowLayout(tracks, collapsed) {
  const rows = [];
  let top = 0;
  for (const track of tracks) {
    const height = rowHeight(track, collapsed);
    rows.push({ track, top, height });
    top += height;
  }
  return { rows, contentHeight: top };
}

/**
 * The panel geometry for a viewport (grid origin arithmetic is exact because the panel is pinned
 * to the bottom of the viewport).
 */
export function layoutMetrics({ viewportWidth, viewportHeight, tracks, collapsed, duration, scale }) {
  const { rows, contentHeight } = rowLayout(tracks, collapsed);
  const uncollapsed = rows.filter((row) => !collapsed.has(row.track.id)).reduce((sum, row) => sum + row.height, 0);
  const bodyHeight = Math.max(120, Math.min(uncollapsed, Math.round(0.42 * viewportHeight)));
  const gridLeft = HEADER_WIDTH;
  const gridTop = viewportHeight - (RULER_HEIGHT + bodyHeight + FOOTER_HEIGHT) + RULER_HEIGHT;
  const gridWidth = Math.max(0, viewportWidth - HEADER_WIDTH);
  const gridHeight = bodyHeight;
  const contentWidth = duration * scale;
  return {
    rows,
    bodyHeight,
    gridLeft,
    gridTop,
    gridWidth,
    gridHeight,
    contentWidth,
    contentHeight,
    maxOffsetX: Math.max(0, contentWidth - gridWidth),
    maxOffsetY: Math.max(0, contentHeight - gridHeight),
  };
}

/** The first tick step whose on-screen spacing reaches 90 px (300 s when none does). */
export function tickStep(scale) {
  return TICK_STEPS.find((step) => step * scale >= TICK_MIN_SPACING) ?? 300;
}

/**
 * Applies a wheel event to the viewport: pan (subtract the deltas), Shift pans horizontally with
 * the negative vertical delta, Ctrl or the command key zooms about the pointer.
 *
 * @param {{ scale: number, offsetX: number, offsetY: number }} view
 * @param {object} metrics `layoutMetrics` result for the current view
 * @param {{ deltaX?: number, deltaY?: number, x?: number, shift?: boolean, ctrl?: boolean, cmd?: boolean }} event
 * @param {number} duration
 */
export function applyWheel(view, metrics, event, duration) {
  const deltaX = event.deltaX ?? 0;
  const deltaY = event.deltaY ?? 0;
  if (event.ctrl || event.cmd) {
    const pointerX = event.x ?? metrics.gridLeft;
    const time = (pointerX - metrics.gridLeft + view.offsetX) / view.scale;
    const scale = clamp(view.scale * Math.exp(0.005 * deltaY), MIN_SCALE, MAX_SCALE);
    const offsetX = clamp(time * scale - (pointerX - metrics.gridLeft), 0, Math.max(0, duration * scale - metrics.gridWidth));
    return { scale, offsetX, offsetY: view.offsetY };
  }
  if (event.shift) {
    return { ...view, offsetX: clamp(view.offsetX - deltaY, 0, metrics.maxOffsetX) };
  }
  return {
    ...view,
    offsetX: clamp(view.offsetX - deltaX, 0, metrics.maxOffsetX),
    offsetY: clamp(view.offsetY - deltaY, 0, metrics.maxOffsetY),
  };
}

/** The zoom slider's exponential mapping: position ratio to scale and back. */
export const sliderToScale = (ratio) => MIN_SCALE * (MAX_SCALE / MIN_SCALE) ** clamp(ratio, 0, 1);
export const scaleToSlider = (scale) => Math.log(scale / MIN_SCALE) / Math.log(MAX_SCALE / MIN_SCALE);

/** The playhead time for a window x on the ruler or grid at the given view. */
export function timeAt(windowX, view, metrics, duration) {
  return clamp((windowX - metrics.gridLeft + view.offsetX) / view.scale, 0, duration);
}

/**
 * Snaps `time` to the nearest candidate within 6 logical pixels inclusive; among equidistant
 * candidates the later one wins. Returns `{ time, guide }` with `guide` null when nothing snapped.
 */
export function snapTime(time, candidates, scale) {
  let best = SNAP_DISTANCE;
  let snapped = null;
  for (const candidate of candidates) {
    const distance = Math.abs(candidate - time) * scale;
    if (distance <= best) {
      best = distance;
      snapped = candidate;
    }
  }
  return snapped === null ? { time, guide: null, snapped: false } : { time: snapped, guide: snapped, snapped: true };
}

/** Snap candidates: 0, the project end, the playhead, then the edges of every other clip on the track. */
export function snapCandidates(project, trackId, excludedClipId, playhead) {
  const candidates = [0, project.duration, playhead];
  for (const clip of project.clips) {
    if (clip.trackId !== trackId || clip.id === excludedClipId) continue;
    candidates.push(clip.start, clip.start + clip.duration);
  }
  return candidates;
}

/** The row (track) whose content-space vertical range holds `y` (clamped to the first and last). */
export function trackAtContentY(rows, y) {
  if (rows.length === 0) return null;
  if (y < rows[0].top) return rows[0].track;
  for (const row of rows) if (y >= row.top && y < row.top + row.height) return row.track;
  return rows[rows.length - 1].track;
}

/**
 * The previewed clip of a move/trim gesture: a pure function of the press-time snapshot, the total
 * pointer displacement, the project and the playhead. `guide` is hidden whenever a clamp
 * overrode the snapped position.
 *
 * @param {{ kind: string, clip: object, scale: number, offsetX: number, offsetY: number,
 *           origin: {x:number,y:number}, current: {x:number,y:number} }} gesture
 * @returns {{ trackId: string, start: number, duration: number, guide: number | null }}
 */
export function previewClip(gesture, project, playhead, rows) {
  const { clip, scale } = gesture;
  const dx = gesture.current.x - gesture.origin.x;
  const dy = gesture.current.y - gesture.origin.y;
  const total = project.duration;

  if (gesture.kind === "move") {
    const target = rows.find((row) => row.track.id === clip.trackId);
    const sourceCenter = (target ? target.top + target.height / 2 : 0) + dy;
    const trackId = (trackAtContentY(rows, sourceCenter) ?? { id: clip.trackId }).id;
    const maxStart = Math.max(0, total - clip.duration);
    const wanted = clamp(clip.start + dx / scale, 0, maxStart);
    const candidates = snapCandidates(project, trackId, clip.id, playhead);
    const startSnap = snapTime(wanted, candidates, scale);
    const endSnap = snapTime(wanted + clip.duration, candidates, scale);
    const startCorrection = Math.abs(startSnap.time - wanted);
    const endCorrection = Math.abs(endSnap.time - (wanted + clip.duration));
    let start = wanted;
    let guide = null;
    if (startSnap.snapped && (!endSnap.snapped || startCorrection <= endCorrection)) {
      start = startSnap.time;
      guide = startSnap.guide;
    } else if (endSnap.snapped) {
      start = endSnap.time - clip.duration;
      guide = endSnap.guide;
    }
    const clamped = clamp(start, 0, maxStart);
    return { trackId, start: clamped, duration: clip.duration, guide: clamped === start ? guide : null };
  }

  const candidates = snapCandidates(project, clip.trackId, clip.id, playhead);
  if (gesture.kind === "trim-start") {
    const end = clip.start + clip.duration;
    const snap = snapTime(clip.start + dx / scale, candidates, scale);
    const start = clamp(snap.time, 0, end - MIN_CLIP_DURATION);
    return { trackId: clip.trackId, start, duration: end - start, guide: start === snap.time ? snap.guide : null };
  }
  const end = clip.start + clip.duration;
  const snap = snapTime(end + dx / scale, candidates, scale);
  const clampedEnd = clamp(snap.time, clip.start + MIN_CLIP_DURATION, total);
  return {
    trackId: clip.trackId,
    start: clip.start,
    duration: clampedEnd - clip.start,
    guide: clampedEnd === snap.time ? snap.guide : null,
  };
}

/** The painted rectangle of a clip in window coordinates at a given view (collapsed tracks are not painted). */
export function paintedRect(clip, rows, view, metrics) {
  const row = rows.find((candidate) => candidate.track.id === clip.trackId);
  if (!row) return null;
  return {
    left: metrics.gridLeft + clip.start * view.scale - view.offsetX,
    width: clip.duration * view.scale,
    top: metrics.gridTop + row.top - view.offsetY,
    height: row.height,
  };
}

/** The ids of the clips whose painted rectangles overlap the marquee (inclusive comparisons). */
export function marqueeSelection(project, collapsed, rows, view, metrics, marquee) {
  const left = Math.min(marquee.x1, marquee.x2);
  const right = Math.max(marquee.x1, marquee.x2);
  const top = Math.min(marquee.y1, marquee.y2);
  const bottom = Math.max(marquee.y1, marquee.y2);
  const selected = [];
  for (const clip of project.clips) {
    if (collapsed.has(clip.trackId)) continue;
    const rect = paintedRect(clip, rows, view, metrics);
    if (!rect) continue;
    if (rect.left + rect.width >= left && rect.left <= right && rect.top + rect.height >= top && rect.top <= bottom) {
      selected.push(clip.id);
    }
  }
  return selected;
}

/** The clips to mount: not collapsed, intersecting the visible time and row ranges inclusively. */
export function visibleClips(project, collapsed, rows, view, metrics, cull = true) {
  const rowById = new Map(rows.map((row) => [row.track.id, row]));
  const startSeconds = view.offsetX / view.scale;
  const endSeconds = (view.offsetX + metrics.gridWidth) / view.scale;
  const topPx = view.offsetY;
  const bottomPx = view.offsetY + metrics.gridHeight;
  return project.clips.filter((clip) => {
    if (collapsed.has(clip.trackId)) return false;
    if (!cull) return true;
    const row = rowById.get(clip.trackId);
    if (!row) return false;
    return (
      clip.start <= endSeconds &&
      clip.start + clip.duration >= startSeconds &&
      row.top <= bottomPx &&
      row.top + row.height >= topPx
    );
  });
}
