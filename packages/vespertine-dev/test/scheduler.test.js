import assert from "node:assert/strict";
import { test } from "node:test";

import { createAppRunner } from "../src/runner.js";
import { createRebuildScheduler } from "../src/scheduler.js";
import { createFakeChild, createFakeClock, deferred, settle } from "./helpers.js";

function harness({ results = [] } = {}) {
  const clock = createFakeClock();
  const builds = [];
  const built = [];
  const errors = [];
  const scheduler = createRebuildScheduler({
    clock,
    build: () => {
      const build = deferred();
      builds.push(build);
      return build.promise;
    },
    onBuilt: () => void built.push(builds.length),
    onError: (error) => errors.push(error),
  });
  return { clock, builds, built, errors, scheduler };
}

test("A3: the initial build runs at start", async () => {
  const { builds, built, scheduler } = harness();
  const started = scheduler.start();
  assert.equal(builds.length, 1);
  builds[0].resolve(true);
  await started;
  assert.deepEqual(built, [1]);
});

test("A3: ten change events within 120 ms cause one rebuild", async () => {
  const { clock, builds, built, scheduler } = harness();
  const started = scheduler.start();
  builds[0].resolve(true);
  await started;

  for (let event = 0; event < 10; event += 1) {
    scheduler.notifyChange();
    if (event < 9) clock.advance(11); // 99 ms in total: every event is inside the quiet period
  }
  assert.equal(builds.length, 1, "no rebuild while changes keep arriving");
  clock.advance(119);
  assert.equal(builds.length, 1, "119 ms after the last change is still quiet");
  clock.advance(1);
  assert.equal(builds.length, 2, "one rebuild 120 ms after the last change");
  builds[1].resolve(true);
  await scheduler.whenIdle();
  assert.deepEqual(built, [1, 2]);
  clock.advance(10_000);
  assert.equal(builds.length, 2);
});

test("A3: changes during a running build queue exactly one further rebuild", async () => {
  const { clock, builds, built, scheduler } = harness();
  const started = scheduler.start();
  builds[0].resolve(true);
  await started;

  scheduler.notifyChange();
  clock.advance(120);
  assert.equal(builds.length, 2);
  assert.equal(scheduler.building, true);

  for (let event = 0; event < 25; event += 1) {
    scheduler.notifyChange();
    clock.advance(30);
  }
  assert.equal(builds.length, 2, "never two builds at once");
  assert.equal(scheduler.queued, true);

  builds[1].resolve(true);
  await settle();
  assert.equal(builds.length, 3, "exactly one queued rebuild runs after the first finished");
  assert.equal(scheduler.queued, false);

  builds[2].resolve(true);
  await scheduler.whenIdle();
  clock.advance(10_000);
  await settle();
  assert.equal(builds.length, 3, "nothing more was queued");
  assert.deepEqual(built, [1, 2, 3]);
});

test("A3: a change during the initial build queues one rebuild", async () => {
  const { builds, scheduler } = harness();
  const started = scheduler.start();
  scheduler.notifyChange();
  scheduler.notifyChange();
  builds[0].resolve(true);
  await settle();
  assert.equal(builds.length, 2);
  builds[1].resolve(true);
  await started;
  assert.equal(builds.length, 2);
});

test("A3: a failed build does not terminate the running child and later changes still rebuild", async () => {
  const clock = createFakeClock();
  const children = [];
  const runner = createAppRunner({
    spawn: () => {
      const child = createFakeChild();
      children.push(child);
      return child;
    },
  });
  const outcomes = [true, false, true];
  const scheduler = createRebuildScheduler({
    clock,
    build: async () => outcomes.shift(),
    onBuilt: () => runner.restart("app", []),
  });

  await scheduler.start();
  assert.equal(children.length, 1);

  scheduler.notifyChange();
  clock.advance(120);
  await scheduler.whenIdle();
  assert.equal(outcomes.length, 1, "the failing build ran");
  assert.equal(children.length, 1, "no new application was started");
  assert.deepEqual(children[0].signals, [], "the running application was not terminated");

  scheduler.notifyChange();
  clock.advance(120);
  await settle();
  assert.deepEqual(children[0].signals, ["SIGTERM"], "a successful build terminates the previous application");
  assert.equal(children.length, 1, "the new application starts only after the old one exited");
  children[0].finish(0, "SIGTERM");
  await scheduler.whenIdle();
  assert.equal(children.length, 2);
  const stopping = runner.stop();
  children[1].finish(0, "SIGTERM");
  await stopping;
  assert.deepEqual(children[1].signals, ["SIGTERM"]);
});

test("a build that rejects is a failed build and is reported", async () => {
  const clock = createFakeClock();
  const errors = [];
  let built = 0;
  const scheduler = createRebuildScheduler({
    clock,
    build: async () => {
      throw new Error("spawn failed");
    },
    onBuilt: () => void (built += 1),
    onError: (error) => errors.push(error.message),
  });
  await scheduler.start();
  assert.deepEqual(errors, ["spawn failed"]);
  assert.equal(built, 0);
});

test("stop cancels the pending quiet period and waits for a running build", async () => {
  const { clock, builds, scheduler } = harness();
  const started = scheduler.start();
  builds[0].resolve(true);
  await started;
  scheduler.notifyChange();
  const stopped = scheduler.stop();
  clock.advance(1000);
  await stopped;
  assert.equal(builds.length, 1);
  scheduler.notifyChange();
  assert.equal(clock.pending, 0);
});
