// Native test helpers (DEV-006): the production renderer and event path over an offscreen test
// window. `createTestRenderer` returns a renderer with the same interface adapters use in
// production (mutation batches in, native events out) plus the test host's inspection and input
// helpers (click, key presses, painted text, element bounds, screenshots, motion clock).
//
// Import this module from tests only; production code imports `@vespertine/host`.

import { mkdirSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { inflateSync } from "node:zlib";
import { createNativeRenderer } from "./renderer.js";
import { loadNative } from "./native.js";

/**
 * @param {{ width?: number, height?: number, scaleFactor?: number, native?: object,
 *           onError?: Function, onEvent?: Function }} [options]
 */
export function createTestRenderer({ width, height, scaleFactor, native, onError, onEvent } = {}) {
  const addon = native ?? loadNative();
  if (typeof addon.TestRenderer !== "function") {
    throw new Error(
      "The native addon was built without the offscreen test host (TestRenderer); " +
        "build it with `node scripts/build-native.mjs --test-host`",
    );
  }
  return createNativeRenderer({
    native: { Renderer: addon.TestRenderer },
    rendererOptions: { width, height, scaleFactor },
    onError,
    onEvent,
    serveAutomation: () => {},
  });
}

/** True when the offscreen test host can render on this machine (a GPU or software adapter exists). */
export function testHostAvailable(native) {
  const addon = native ?? loadNative();
  return typeof addon.testHostAvailable === "function" ? addon.testHostAvailable() : false;
}

/** Decodes an 8-bit RGBA or RGB PNG file into `{ width, height, at(x, y) -> [r, g, b, a] }`. */
export function readPng(path) {
  const file = readFileSync(path);
  let offset = 8;
  let header = null;
  const data = [];
  while (offset < file.length) {
    const length = file.readUInt32BE(offset);
    const type = file.toString("latin1", offset + 4, offset + 8);
    const body = file.subarray(offset + 8, offset + 8 + length);
    if (type === "IHDR") header = { width: body.readUInt32BE(0), height: body.readUInt32BE(4), depth: body[8], color: body[9] };
    if (type === "IDAT") data.push(body);
    offset += 12 + length;
  }
  if (!header || header.depth !== 8 || (header.color !== 6 && header.color !== 2)) throw new Error("unsupported PNG");
  const channels = header.color === 6 ? 4 : 3;
  const stride = header.width * channels;
  const raw = inflateSync(Buffer.concat(data));
  const pixels = Buffer.alloc(stride * header.height);
  for (let y = 0; y < header.height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? pixels[y * stride + x - channels] : 0;
      const up = y > 0 ? pixels[(y - 1) * stride + x] : 0;
      const upLeft = y > 0 && x >= channels ? pixels[(y - 1) * stride + x - channels] : 0;
      let value = line[x];
      if (filter === 1) value += left;
      else if (filter === 2) value += up;
      else if (filter === 3) value += (left + up) >> 1;
      else if (filter === 4) {
        const p = left + up - upLeft;
        const pa = Math.abs(p - left);
        const pb = Math.abs(p - up);
        const pc = Math.abs(p - upLeft);
        value += pa <= pb && pa <= pc ? left : pb <= pc ? up : upLeft;
      }
      pixels[y * stride + x] = value & 0xff;
    }
  }
  return {
    width: header.width,
    height: header.height,
    at: (x, y) => {
      const at = (y * header.width + x) * channels;
      return [pixels[at], pixels[at + 1], pixels[at + 2], channels === 4 ? pixels[at + 3] : 255];
    },
  };
}

let screenshotCounter = 0;

/**
 * Captures the test window and decodes it (see `readPng`). The PNG goes through a scratch file
 * under the build output directory (`VESPERTINE_TEST_OUT`, default `<repository>/target`), which
 * is removed again.
 */
export function screenshotPixels(renderer) {
  const base = process.env.VESPERTINE_TEST_OUT ?? fileURLToPath(new URL("../../../target", import.meta.url));
  const directory = join(base, "test-screenshots");
  mkdirSync(directory, { recursive: true });
  const path = join(directory, `${process.pid}-${screenshotCounter++}.png`);
  renderer.captureScreenshot(path);
  try {
    return readPng(path);
  } finally {
    rmSync(path, { force: true });
  }
}
