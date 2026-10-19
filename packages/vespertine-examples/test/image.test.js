import assert from "node:assert/strict";
import { test } from "node:test";
import { h } from "../src/h.js";
import { BACKGROUND, DirectImage, HEIGHT, PHASE_STEP, TRACE, WIDTH, renderTrace, traceRow } from "../src/image/index.js";
import { mount } from "./helpers.js";

function checkBuffer(buffer, phase) {
  assert.equal(buffer.length, WIDTH * HEIGHT * 4);
  for (let x = 0; x < WIDTH; x++) {
    const expected = Math.floor(95.5 - 0.72 * 95.5 * Math.sin((6 * Math.PI * x) / WIDTH + phase) + 0.5);
    let traces = 0;
    for (let y = 0; y < HEIGHT; y++) {
      const at = (y * WIDTH + x) * 4;
      const pixel = [...buffer.subarray(at, at + 4)];
      if (pixel.every((byte, i) => byte === TRACE[i])) {
        traces++;
        assert.equal(y, expected, `column ${x}`);
      } else {
        assert.deepEqual(pixel, [...BACKGROUND], `column ${x} row ${y}`);
      }
    }
    assert.equal(traces, 1, `column ${x} has exactly one trace pixel`);
  }
}

test("A3: every column has exactly one #5CA9FF pixel at the formula's row and the rest is #121626", () => {
  assert.deepEqual([...BACKGROUND], [0x26, 0x16, 0x12, 0xff]);
  assert.deepEqual([...TRACE], [0xff, 0xa9, 0x5c, 0xff]);
  checkBuffer(renderTrace(0), 0);
  checkBuffer(renderTrace(PHASE_STEP), PHASE_STEP);
  assert.equal(traceRow(0, 0), 96, "round half up of 95.5");
});

test("A3: a pointer press advances the phase by 0.6 and replaces the pixels through the image reference without remounting", () => {
  const ui = mount(h(DirectImage, { width: 800, height: 500 }));
  const image = ui.find("direct-image");
  const uploads = () => ui.renderer.calls.filter((c) => c[0] === "setImagePixels");
  assert.equal(uploads().length, 1);
  const [, id, width, height, bytes, format] = uploads()[0];
  assert.deepEqual([id, width, height, format], [image.id, 1440, 192, "bgra"]);
  checkBuffer(bytes, 0);
  const created = ui.renderer.count("createElement");
  ui.emit("image-surface", { type: "pointerDown", x: 10, y: 10, button: 0 });
  assert.equal(uploads().length, 2);
  checkBuffer(uploads()[1][4], PHASE_STEP);
  assert.notDeepEqual(uploads()[0][4], uploads()[1][4]);
  assert.equal(ui.renderer.count("createElement"), created, "no element was recreated");
  assert.equal(ui.find("direct-image").id, image.id);
  assert.deepEqual([image.style.width, image.style.height, image.style.borderRadius, image.style.objectFit], [720, 96, 10, "fill"]);
});

test("A3: the scroll-into-view control reveals the last of five 48 px rows through its reference", () => {
  const ui = mount(h(DirectImage, { width: 800, height: 500 }));
  assert.equal(ui.find("scrolling-list").style.height, 120);
  assert.equal(ui.find("scroll-row-0").style.height, 48);
  ui.emit("scroll-into-view", { type: "click" });
  const call = ui.renderer.calls.find((c) => c[0] === "scrollIntoView");
  assert.equal(call[1], ui.find("scroll-row-4").id);
});
