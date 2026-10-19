// Helpers for the native acceptance tests of the reference examples (APP-011, APP-012): the
// offscreen test host, element lookup by test id, text readouts and PNG decoding.

import { readFileSync } from "node:fs";
import { inflateSync } from "node:zlib";
import { test } from "node:test";
import { createTestHost, testHostAvailable } from "@vespertine/react/testing";

export { createTestHost };

const available = testHostAvailable();

/** A test that needs the offscreen test host; skipped where the machine has no rendering adapter. */
export const native = (name, body) => test(name, { skip: available ? false : "no GPU test backend" }, body);

/** The (first) native tree node with `testId`, or null. */
export function find(host, testId) {
  return host.renderer.findByTestId(testId)[0] ?? null;
}

export function need(host, testId) {
  const node = find(host, testId);
  if (!node) throw new Error(`no element with testId ${testId}`);
  return node;
}

export const centerOf = ({ x, y, width, height }) => [x + width / 2, y + height / 2];

/** The text of a node and its descendants. */
export const textIn = (node) => node.text ?? (node.children ?? []).map(textIn).join("");

export const text = (host, testId) => textIn(need(host, testId));

export const bounds = (host, testId) => need(host, testId).bounds;

/** Clicks the center of the element with `testId` through the native input path. */
export function click(host, testId, button = 0, modifiers = "") {
  host.renderer.nativeSimulateClick(...centerOf(bounds(host, testId)), button, modifiers);
}

/** Moves the pointer to the center of the element (a hover). */
export function hover(host, testId) {
  host.renderer.nativeSimulateMouseMove(...centerOf(bounds(host, testId)));
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

/** The ease-out curve cubic-bezier(0, 0, 0.58, 1) evaluated at normalized time `t`. */
export function easeOut(t) {
  const bezier = (a, b, u) => 3 * a * (1 - u) ** 2 * u + 3 * b * (1 - u) * u ** 2 + u ** 3;
  let low = 0;
  let high = 1;
  for (let i = 0; i < 40; i++) {
    const mid = (low + high) / 2;
    if (bezier(0, 0.58, mid) < t) low = mid;
    else high = mid;
  }
  return bezier(0, 1, (low + high) / 2);
}
