// APP-011 A1 on the native offscreen test host: the task list through real pointer input, native
// layout, hit testing and the motion clock.

import assert from "node:assert/strict";
import { h } from "../src/h.js";
import { TaskList } from "../src/tasks/index.js";
import { bounds, centerOf, click, createTestHost, easeOut, find, hover, native, need, text } from "./native-helpers.js";

const W = 940;
const H = 660;

function mount() {
  const host = createTestHost({ width: W, height: H });
  host.render(h(TaskList, { width: W, height: H }));
  return host;
}

const rowIds = (host) => host.renderer.findByType("div").map((node) => node.testId).filter((id) => /^task-row-\d+$/.test(id ?? "")).map((id) => Number(id.slice(9)));
const counts = (host) => ["general", "today", "starred", "complete"].map((view) => text(host, `count-${view}`));

native("A1: each sidebar category repaints the rows, the header and the counts 6 / 2 / 2 / 2", () => {
  const host = mount();
  assert.deepEqual(counts(host), ["6", "2", "2", "2"]);
  assert.equal(text(host, "header-title"), "Today");
  assert.equal(rowIds(host).length, 2);
  const expected = { general: [6, "Tasks"], starred: [2, "Starred"], complete: [2, "Completed"], today: [2, "Today"] };
  for (const [view, [rows, title]] of Object.entries(expected)) {
    click(host, `view-${view}`);
    assert.equal(rowIds(host).length, rows, view);
    assert.equal(text(host, "header-title"), title);
    assert.equal(text(host, "header-count"), String(rows));
  }
  assert.deepEqual(host.errors, []);
});

native("A1: rows are laid out in the virtual list at the 48 px estimate inside the 640 px column", () => {
  const host = mount();
  click(host, "view-general");
  const list = bounds(host, "task-list");
  const rows = rowIds(host).map((id) => bounds(host, `task-row-${id}`));
  assert.equal(rows.length, 6);
  rows.forEach((row, index) => assert.deepEqual([row.height, row.y], [48, list.y + 48 * index], `row ${index}`));
  assert.equal(bounds(host, "sidebar-outer").width, 236);
  assert.equal(bounds(host, "header").height, 52);
  assert.equal(bounds(host, "composer").height, 46);
  assert.ok(rows.every((row) => row.width <= 640));
  assert.equal(bounds(host, "main").x, 236);
});

native("A1: hovering reveals the delete control and deleting removes exactly the hovered row", () => {
  const host = mount();
  click(host, "view-general");
  assert.equal(find(host, "task-delete-6"), null, "controls are hidden at rest");
  hover(host, "task-row-6");
  assert.ok(find(host, "task-delete-6"), "the hovered row shows its controls");
  assert.equal(find(host, "task-delete-5"), null, "no other row does");
  click(host, "task-delete-6");
  assert.deepEqual(rowIds(host), [3, 4, 5, 7, 8]);
  assert.deepEqual(counts(host).slice(0, 2), ["5", "2"]);
});

native("A1: completing a row removes it from incomplete views and adds it to the complete view", () => {
  const host = mount();
  const [first] = rowIds(host);
  click(host, `task-check-${first}`);
  assert.equal(rowIds(host).includes(first), false);
  assert.deepEqual(counts(host), ["5", "1", "1", "3"]);
  click(host, "view-complete");
  assert.equal(rowIds(host).includes(first), true);
});

native("A1: toggling a star changes membership of the starred view immediately and the empty state replaces the list", () => {
  const host = mount();
  click(host, "view-starred");
  const starred = rowIds(host);
  assert.equal(starred.length, 2);
  for (const id of starred) {
    hover(host, `task-row-${id}`);
    click(host, `task-star-${id}`);
    assert.equal(rowIds(host).includes(id), false, "an un-starred row disappears from the starred view");
  }
  assert.ok(find(host, "empty-state"), "the empty-state element replaces the list");
  assert.equal(find(host, "task-list"), null);
  assert.equal(text(host, "empty-state"), "Nothing here yet");
  assert.deepEqual(counts(host)[2], "0");
});

native("A1: sidebar collapse interpolates the outer width between 236 and 0 over 0.2 s on the motion clock while the inner width stays 236", () => {
  const host = mount();
  host.renderer.clockPause();
  host.renderer.clockSet(0);
  host.renderer.flush();
  assert.equal(bounds(host, "sidebar-outer").width, 236, "the first frame is already at the target width");
  click(host, "sidebar-toggle");
  assert.equal(bounds(host, "sidebar-outer").width, 236, "the track starts at the visible width");
  for (const ms of [50, 100, 150, 200, 400]) {
    host.renderer.clockSet(ms);
    host.renderer.flush();
    const expected = 236 * (1 - easeOut(Math.min(ms / 200, 1)));
    assert.ok(Math.abs(bounds(host, "sidebar-outer").width - expected) <= 0.5, `at ${ms} ms: ${bounds(host, "sidebar-outer").width} vs ${expected}`);
    assert.equal(bounds(host, "sidebar-inner").width, 236, "the inner sidebar never reflows");
  }
  assert.equal(bounds(host, "sidebar-outer").width, 0);
  assert.equal(bounds(host, "main").x, 0, "the content column takes the freed space");
  click(host, "sidebar-toggle");
  host.renderer.clockSet(500);
  host.renderer.flush();
  assert.ok(bounds(host, "sidebar-outer").width > 0 && bounds(host, "sidebar-outer").width < 236, "retargeted from the visible value");
  host.renderer.clockSet(800);
  host.renderer.flush();
  assert.equal(bounds(host, "sidebar-outer").width, 236);
  assert.deepEqual(host.errors, []);
});
