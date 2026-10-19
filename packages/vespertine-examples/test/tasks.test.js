import assert from "node:assert/strict";
import { test } from "node:test";
import { h } from "../src/h.js";
import { TaskList, addTask, counts, initialTasks, tasksInView } from "../src/tasks/index.js";
import { allNodes, findByTestId, mount, textOf } from "./helpers.js";

const ui = () => mount(h(TaskList, { width: 940, height: 660 }));
const titles = (u) => allNodes(u.renderer.snapshot()).filter((n) => /^task-title-/.test(n.props.testId ?? "")).map(textOf);
const count = (u, view) => textOf(u.find(`count-${view}`));
const rowIds = (u) => allNodes(u.renderer.snapshot()).filter((n) => /^task-row-/.test(n.props.testId ?? "")).map((n) => Number(n.props.testId.slice(9)));

test("R1: initial data, counts 6 / 2 / 2 / 2, default view today", () => {
  const tasks = initialTasks();
  assert.equal(tasks.length, 8);
  assert.deepEqual(counts(tasks), { general: 6, today: 2, starred: 2, complete: 2 });
  assert.equal(tasks.filter((t) => t.complete).every((t) => t.today && !t.starred), true);
  const u = ui();
  assert.deepEqual(["general", "today", "starred", "complete"].map((v) => count(u, v)), ["6", "2", "2", "2"]);
  assert.equal(textOf(u.find("header-title")), "Today");
  assert.equal(textOf(u.find("header-count")), "2");
  assert.equal(rowIds(u).length, 2);
});

test("A1: each sidebar category repaints the rows, the header title and the header count", () => {
  const u = ui();
  const expectations = { general: [6, "Tasks"], starred: [2, "Starred"], complete: [2, "Completed"], today: [2, "Today"] };
  for (const [view, [rows, title]] of Object.entries(expectations)) {
    u.emit(`view-${view}`, { type: "click" });
    assert.equal(rowIds(u).length, rows, view);
    assert.equal(textOf(u.find("header-title")), title);
    assert.equal(textOf(u.find("header-count")), String(rows));
  }
});

test("R1: layout constants and the capped content column", () => {
  const u = ui();
  assert.equal(u.find("sidebar-inner").style.width, 236);
  assert.equal(u.find("sidebar-inner").style.paddingTop, 20);
  assert.equal(u.find("header").style.height, 52);
  assert.equal(u.find("composer").style.height, 46);
  assert.equal(u.find("task-list").props.estimatedItemHeight, 48);
  assert.equal(u.find("task-list").type, "virtual-list");
  assert.equal(allNodes(u.renderer.snapshot()).find((n) => n.style?.maxWidth === 640)?.style.maxWidth, 640);
});

test("A1: hover reveals the controls (without duplicating a static star) and deletion removes exactly the hovered row", () => {
  const u = ui();
  u.emit("view-general", { type: "click" });
  const starred = initialTasks().find((t) => t.starred && t.today);
  assert.ok(u.find(`task-star-static-${starred.id}`), "a starred row shows a static star");
  assert.equal(u.find(`task-star-${starred.id}`), null);
  u.emit(`task-row-${starred.id}`, { type: "enter" });
  assert.equal(u.find(`task-star-static-${starred.id}`), null, "the interactive star replaces the static one");
  assert.ok(u.find(`task-star-${starred.id}`));
  const victim = 6;
  u.emit(`task-row-${victim}`, { type: "enter" });
  assert.ok(u.find(`task-delete-${victim}`));
  assert.equal(u.find(`task-delete-${starred.id}`), null, "only the hovered row shows controls");
  u.emit(`task-delete-${victim}`, { type: "click" });
  assert.deepEqual(rowIds(u), [3, 4, 5, 7, 8]);
});

test("A1: completion removes a row from every incomplete view; star toggling changes starred membership", () => {
  const u = ui();
  u.emit("view-general", { type: "click" });
  u.emit("task-check-4", { type: "click" });
  assert.equal(rowIds(u).includes(4), false);
  assert.deepEqual(["general", "today", "starred", "complete"].map((v) => count(u, v)), ["5", "1", "2", "3"]);
  u.emit("view-complete", { type: "click" });
  assert.ok(rowIds(u).includes(4));
  u.emit("view-starred", { type: "click" });
  assert.deepEqual(rowIds(u), [3, 5]);
  u.emit("task-row-5", { type: "enter" });
  u.emit("task-star-5", { type: "click" });
  assert.deepEqual(rowIds(u), [3], "an un-starred row inside the starred view disappears");
});

test("A1: the empty-state element replaces the list", () => {
  const u = ui();
  u.emit("view-today", { type: "click" });
  u.emit("task-check-3", { type: "click" });
  u.emit("task-check-4", { type: "click" });
  assert.equal(u.find("task-list"), null);
  assert.ok(u.find("empty-state"));
  assert.equal(textOf(u.find("header-count")), "0");
});

test("A1: creation trims, rejects blanks, disables Add, clears the composer and applies the current-day rule", () => {
  const u = ui();
  const add = () => u.find("composer-add");
  assert.equal(add().props.tabIndex, -1, "Add is disabled while the trimmed input is empty");
  u.emit("composer-input", { type: "change", value: "   " });
  assert.equal(add().props.tabIndex, -1);
  assert.equal(add().listeners.includes("click"), false, "a disabled Add has no click handler");
  u.emit("composer-input", { type: "submit", value: "   " });
  assert.equal(rowIds(u).length, 2, "a blank input is rejected");

  u.emit("composer-input", { type: "change", value: "  Write the agenda  " });
  assert.equal(add().props.tabIndex, 0);
  assert.ok(add().listeners.includes("click"));
  u.emit("composer-add", { type: "click" });
  assert.equal(titles(u)[0], "Write the agenda");
  assert.equal(u.find("composer-input").props.value ?? "", "", "the composer is cleared");
  assert.equal(count(u, "today"), "3", "created in the today view: current-day");

  u.emit("composer-input", { type: "change", value: "Second" });
  u.emit("composer-input", { type: "submit" });
  assert.equal(titles(u)[0], "Second", "Enter in the input submits like Add");

  u.emit("view-general", { type: "click" });
  u.emit("composer-input", { type: "change", value: "From general" });
  u.emit("composer-add", { type: "click" });
  assert.equal(count(u, "today"), "4", "created in the general view: not current-day");
  assert.equal(count(u, "general"), "9");

  const model = addTask([], "x", "general", 1);
  assert.equal(model[0].today, false);
  assert.equal(addTask(model, "  ", "today", 2), model);
  assert.deepEqual(tasksInView(addTask([], "y", "starred", 3), "today").map((t) => t.id), [3]);
});

test("A1: twenty consecutive prepends each leave the new row first in the list", () => {
  const u = ui();
  for (let i = 1; i <= 20; i++) {
    u.emit("composer-input", { type: "change", value: `New ${i}` });
    u.emit("composer-add", { type: "click" });
    assert.equal(titles(u)[0], `New ${i}`);
  }
  assert.equal(rowIds(u).length, 22);
});

test("R3: the sidebar collapse animates only the outer clipping container, from its target, without an initial animation", () => {
  const u = ui();
  const outer = () => u.find("sidebar-outer");
  assert.deepEqual(outer().props.motion, { initial: false, animate: { width: 236 }, transition: { duration: 0.2, ease: "easeOut" } });
  assert.equal(outer().style.width, 236);
  assert.equal(outer().style.overflow, "hidden");
  u.emit("sidebar-toggle", { type: "click" });
  assert.equal(outer().props.motion.animate.width, 0);
  assert.equal(outer().props.motion.initial, false);
  assert.equal(u.find("sidebar-inner").style.width, 236, "the inner sidebar keeps its fixed width");
});

test("explicit sizes keep the root independent of the window", () => {
  const u = ui();
  assert.deepEqual([u.find("task-list-root").style.width, u.find("task-list-root").style.height], [940, 660]);
  const flexible = mount(h(TaskList, {}));
  assert.equal(findByTestId(flexible.renderer, "task-list-root").style.width, 800);
});
