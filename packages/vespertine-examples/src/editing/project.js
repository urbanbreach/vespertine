// Deterministic sample project generation for the editing surface (APP-012 R9).

/** A small seeded generator (mulberry32): the same seed always yields the same sequence. */
export function createRandom(seed) {
  let state = seed >>> 0;
  return function next() {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const round3 = (value) => Math.round(value * 1000) / 1000;

export const DEFAULT_DURATION = 230;
export const DEFAULT_TRACKS = 10;
export const MIN_CLIP = 0.4;

const DURATION_RANGES = { caption: [1.2, 3.8], audio: [24, 64], first: [1.5, 8.5], second: [1.5, 8.5] };

/**
 * Generates the sample project: one caption track, then `trackCount` numbered tracks from
 * `trackCount` down to 1, then one audio track. The two highest-numbered tracks are decorative
 * (one clip spanning the whole project); the others scatter clips. Numbered tracks alternate
 * between a first clip kind (number divisible by 3) and a second.
 *
 * @returns {{ duration: number, tracks: object[], clips: object[] }}
 */
export function generateProject({ seed = 1, trackCount = DEFAULT_TRACKS, durationSeconds = DEFAULT_DURATION } = {}) {
  const random = createRandom(seed);
  const tracks = [{ id: "caption", kind: "caption", clipKind: "caption" }];
  for (let number = trackCount; number >= 1; number--) {
    tracks.push({
      id: `track-${number}`,
      kind: "numbered",
      number,
      clipKind: number % 3 === 0 ? "first" : "second",
      decorative: number > trackCount - 2,
    });
  }
  tracks.push({ id: "audio", kind: "audio", clipKind: "audio" });

  const clips = [];
  let serial = 0;
  for (const track of tracks) {
    if (track.decorative) {
      clips.push({ id: `clip-${serial++}`, trackId: track.id, start: 0, duration: durationSeconds, kind: track.clipKind });
      continue;
    }
    const [low, high] = DURATION_RANGES[track.kind === "numbered" ? track.clipKind : track.kind];
    let cursor = random() * 6;
    for (;;) {
      const duration = low + random() * (high - low);
      if (cursor + duration > durationSeconds) break;
      clips.push({
        id: `clip-${serial++}`,
        trackId: track.id,
        start: round3(cursor),
        duration: round3(duration),
        kind: track.clipKind,
      });
      cursor += duration + random() * 3;
    }
  }
  return { duration: durationSeconds, tracks, clips };
}
