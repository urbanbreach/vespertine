// Helpers for the native acceptance tests of the reference examples (APP-011, APP-012): the
// offscreen test host, element lookup by test id, text readouts and PNG decoding.

import { test } from "node:test";
import { createTestHost, testHostAvailable } from "@vespertine/react/testing";
import { loadNative } from "@vespertine/host/native";
import { readPng, screenshotPixels } from "@vespertine/host/testing";

export { createTestHost, readPng, screenshotPixels };

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

/** Types `value` into the focused editor (a space is the `space` key). */
export function typeText(host, value) {
  host.renderer.nativeSimulateKeystrokes([...value].map((char) => (char === " " ? "space" : char)).join(" "));
}

/** Lets promises and the React scheduler run, then flushes the native tree. */
export async function settle(host, ms = 30) {
  await new Promise((resolve) => setTimeout(resolve, ms));
  host.renderer.flush();
}

/**
 * A native addon whose test renderer records every mutation batch it receives: pass `native` to
 * `createTestHost`; `batches` holds the parsed batches in order.
 */
export function recordingNative() {
  const addon = loadNative();
  const batches = [];
  class Recording extends addon.TestRenderer {
    applyBatch(json) {
      batches.push(JSON.parse(json));
      return super.applyBatch(json);
    }
  }
  return { native: { ...addon, TestRenderer: Recording }, batches };
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
