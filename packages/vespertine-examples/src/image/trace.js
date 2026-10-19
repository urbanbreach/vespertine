// The direct-image example's pixels (APP-011 R7): a 1440 x 192 BGRA buffer (the 2x device-pixel
// resolution of a 720 x 96 element) with an opaque background and a one-pixel sine trace.

export const WIDTH = 1440;
export const HEIGHT = 192;
export const BACKGROUND = Object.freeze([0x26, 0x16, 0x12, 0xff]); // #121626 as B, G, R, A
export const TRACE = Object.freeze([0xff, 0xa9, 0x5c, 0xff]); // #5CA9FF as B, G, R, A
export const PHASE_STEP = 0.6;
/** The trace's amplitude factor and vertical center (in device pixels). */
export const AMPLITUDE = 0.72;
export const CENTER = (HEIGHT - 1) / 2; // 95.5

/** Round-half-up on the real value. */
const roundHalfUp = (value) => Math.floor(value + 0.5);

/** The trace's row at device column `x` for a phase. */
export function traceRow(x, phase) {
  return roundHalfUp(CENTER - AMPLITUDE * CENTER * Math.sin((6 * Math.PI * x) / WIDTH + phase));
}

/** Renders the buffer: 4 bytes per pixel, rows contiguous; exactly one trace pixel per column. */
export function renderTrace(phase, buffer = new Uint8Array(WIDTH * HEIGHT * 4)) {
  for (let i = 0; i < WIDTH * HEIGHT; i++) buffer.set(BACKGROUND, i * 4);
  for (let x = 0; x < WIDTH; x++) buffer.set(TRACE, (traceRow(x, phase) * WIDTH + x) * 4);
  return buffer;
}
