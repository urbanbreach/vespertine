// APP-011 A3 on the native offscreen test host: the uploaded 1440 x 192 buffer reaches the screen
// pixel for pixel on a 2x window, a phase change replaces the pixels through the image reference
// without remounting, and the scroll-into-view control reveals the last row.

import assert from "node:assert/strict";
import { h } from "../src/h.js";
import { DirectImage, HEIGHT, PHASE_STEP, WIDTH } from "../src/image/index.js";
import { bounds, click, createTestHost, native, need, screenshotPixels } from "./native-helpers.js";

const BACKGROUND = [0x12, 0x16, 0x26, 0xff];
const TRACE = [0x5c, 0xa9, 0xff, 0xff];
/** The element's corner radius in device pixels: 10 logical px at scale 2. */
const CORNER = 20;

const inCorner = (x, y) => (x < CORNER || x >= WIDTH - CORNER) && (y < CORNER || y >= HEIGHT - CORNER);
const same = (a, b) => a[0] === b[0] && a[1] === b[1] && a[2] === b[2] && a[3] === b[3];

/** Checks the screenshot's image region against the formula for `phase`; returns how many corner pixels were clipped. */
function checkRegion(png, left, top, phase) {
  let clipped = 0;
  for (let x = 0; x < WIDTH; x++) {
    const expected = Math.floor(95.5 - 0.72 * 95.5 * Math.sin((6 * Math.PI * x) / WIDTH + phase) + 0.5);
    let traces = 0;
    for (let y = 0; y < HEIGHT; y++) {
      const pixel = png.at(left + x, top + y);
      if (same(pixel, TRACE)) {
        traces++;
        assert.equal(y, expected, `column ${x}: the trace is at row ${y}`);
      } else if (inCorner(x, y)) {
        if (!same(pixel, BACKGROUND)) clipped++;
      } else {
        assert.deepEqual(pixel, BACKGROUND, `column ${x} row ${y}`);
      }
    }
    assert.equal(traces, 1, `column ${x} has exactly one trace pixel`);
  }
  return clipped;
}

function mount() {
  const host = createTestHost({ width: 800, height: 500, scaleFactor: 2 });
  host.render(h(DirectImage, { width: 800, height: 500 }));
  host.renderer.flush();
  return host;
}

native("A3: every column of the shown image has exactly one #5CA9FF pixel at the formula's row and the rest is #121626", () => {
  const host = mount();
  const image = bounds(host, "direct-image");
  assert.deepEqual([image.width, image.height], [720, 96]);
  const png = screenshotPixels(host.renderer);
  assert.equal(png.width, 1600, "the window is rendered at the 2x device resolution");
  const clipped = checkRegion(png, image.x * 2, image.y * 2, 0);
  assert.ok(clipped > 0, "the 10 px corner radius clips the corners of the element");
  assert.deepEqual(host.errors, []);
});

native("A3: a pointer press changes the pixels through the image reference without remounting", () => {
  const host = mount();
  const image = need(host, "direct-image");
  const before = host.renderer.getRetainedElementCount();
  const first = screenshotPixels(host.renderer);
  click(host, "image-surface");
  host.renderer.flush();
  assert.equal(need(host, "direct-image").id, image.id, "the same element shows the new pixels");
  assert.equal(host.renderer.getRetainedElementCount(), before, "no element was created or destroyed");
  const second = screenshotPixels(host.renderer);
  checkRegion(second, image.bounds.x * 2, image.bounds.y * 2, PHASE_STEP);
  let differing = 0;
  for (let x = 0; x < WIDTH; x += 40) {
    for (let y = 0; y < HEIGHT; y += 4) {
      if (!same(first.at(image.bounds.x * 2 + x, image.bounds.y * 2 + y), second.at(image.bounds.x * 2 + x, image.bounds.y * 2 + y))) differing++;
    }
  }
  assert.ok(differing > 0, "the pixels changed");
  assert.deepEqual(host.errors, []);
});

native("A3: the scroll-into-view control sets the list's offset to 120 px", () => {
  const host = mount();
  const list = need(host, "scrolling-list");
  assert.equal(list.bounds.height, 120);
  assert.equal(need(host, "scroll-row-0").bounds.height, 48);
  assert.equal(host.renderer.getScrollOffset(list.id).y, 0);
  click(host, "scroll-into-view");
  host.renderer.flush();
  assert.equal(-host.renderer.getScrollOffset(list.id).y, 120, "the content moved up by 120 px");
  assert.deepEqual(host.errors, []);
});
