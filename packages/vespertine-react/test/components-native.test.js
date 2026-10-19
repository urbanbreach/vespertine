// The headless components on the native offscreen test host (DEV-006): real layout, hit testing,
// focus and the dismissal path.

import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import { Select, Tooltip } from "../src/index.js";
import { createTestHost, testHostAvailable } from "../src/testing.js";
import { settle } from "./helpers.js";

const h = React.createElement;
const available = testHostAvailable();
const native = (name, body) => test(name, { skip: available ? false : "no GPU test backend" }, body);

const find = (host, testId) => host.renderer.findByTestId(testId)[0] ?? null;
const bounds = (host, testId) => find(host, testId)?.bounds;
const center = ({ x, y, width, height }) => [x + width / 2, y + height / 2];
const textIn = (node) => node.text ?? (node.children ?? []).map(textIn).join("");

function selectApp(props = {}) {
  return h(
    "div",
    { style: { width: 400, height: 300, paddingLeft: 12, paddingTop: 12 } },
    h(
      Select.Root,
      { testId: "root", ...props },
      h(
        Select.Trigger,
        { testId: "trigger", style: { width: 180, height: 36, justifyContent: "center", backgroundColor: "#303030" } },
        h(Select.Value, { testId: "value", placeholder: "Pick", style: { color: "#ffffff", fontSize: 14 } }),
      ),
      h(
        Select.Content,
        { testId: "content", sideOffset: 4, style: { width: 180, flexDirection: "column", padding: 4 } },
        ["Apple", "Banana", "Cherry"].map((label) =>
          h(Select.Item, { key: label, testId: `item-${label}`, value: label, disabled: label === "Banana", style: { height: 32, width: 172 } }, h("text", { style: { color: "#ffffff", fontSize: 14 } }, label)),
        ),
      ),
    ),
  );
}

native("select: a click opens the popup below the trigger, keys step over the disabled item and Enter selects and returns focus", async () => {
  const host = createTestHost({ width: 400, height: 300 });
  host.render(selectApp());
  assert.equal(find(host, "content"), null);
  host.renderer.nativeSimulateClick(...center(bounds(host, "trigger")));
  await settle();
  host.renderer.flush();
  assert.ok(find(host, "content"), "a click opens the popup");
  const trigger = bounds(host, "trigger");
  const content = bounds(host, "content");
  // The popup container sits inside the floating surface's 4 px padding.
  assert.equal(content.x, trigger.x + 4);
  assert.equal(content.y, trigger.y + trigger.height + 4 + 4, "below the trigger with the side offset");
  assert.equal(host.renderer.getFocusedElementId(), find(host, "content").id, "the popup takes focus");
  host.renderer.simulateKeystrokes("down");
  host.renderer.simulateKeystrokes("down");
  host.renderer.simulateKeystrokes("enter");
  await settle();
  host.renderer.flush();
  assert.equal(find(host, "content"), null, "Enter closes the popup");
  assert.equal(textIn(find(host, "value")), "Cherry", "the disabled Banana was skipped");
  assert.equal(host.renderer.getFocusedElementId(), find(host, "trigger").id, "focus returns to the trigger");
  assert.deepEqual(host.errors, []);
});

native("select: Escape closes the popup and focuses the trigger; an outside click closes it; pressing the open trigger leaves it closed", async () => {
  const host = createTestHost({ width: 400, height: 300 });
  host.render(selectApp());
  const open = async () => {
    host.renderer.nativeSimulateClick(...center(bounds(host, "trigger")));
    await settle();
    host.renderer.flush();
    assert.ok(find(host, "content"));
  };
  await open();
  host.renderer.simulateKeystrokes("escape");
  await settle();
  host.renderer.flush();
  assert.equal(find(host, "content"), null, "Escape closes it");
  assert.equal(host.renderer.getFocusedElementId(), find(host, "trigger").id);
  await open();
  host.renderer.nativeSimulateClick(380, 280);
  await settle();
  host.renderer.flush();
  assert.equal(find(host, "content"), null, "an outside press closes it");
  await open();
  host.renderer.nativeSimulateClick(...center(bounds(host, "trigger")));
  await settle();
  host.renderer.flush();
  assert.equal(find(host, "content"), null, "pressing the open trigger leaves it closed (not closed and reopened)");
  await settle();
  host.renderer.flush();
  assert.equal(find(host, "content"), null);
  assert.deepEqual(host.errors, []);
});

native("tooltip: focus by Tab opens at once and Escape closes it", async () => {
  const host = createTestHost({ width: 400, height: 300 });
  host.render(
    h(
      "div",
      { style: { width: 400, height: 300, paddingLeft: 100, paddingTop: 100 } },
      h(
        Tooltip.Provider,
        {},
        h(
          Tooltip.Root,
          {},
          h(Tooltip.Trigger, { testId: "trigger", style: { width: 60, height: 30 } }, h("text", { style: { color: "#fff" } }, "?")),
          h(Tooltip.Content, { testId: "tip", sideOffset: 4, style: { padding: 4 } }, h("text", { style: { color: "#fff" } }, "Help")),
        ),
      ),
    ),
  );
  assert.equal(find(host, "tip"), null);
  host.renderer.simulateKeystrokes("tab");
  await settle();
  host.renderer.flush();
  assert.ok(find(host, "tip"), "keyboard focus opens the tooltip");
  assert.equal(host.renderer.getFocusedElementId(), find(host, "trigger").id, "the tooltip took no focus");
  const trigger = bounds(host, "trigger");
  const tip = bounds(host, "tip");
  assert.ok(tip.y + tip.height <= trigger.y, "side top places it above the trigger");
  host.renderer.simulateKeystrokes("escape");
  await settle();
  host.renderer.flush();
  assert.equal(find(host, "tip"), null);
  assert.deepEqual(host.errors, []);
});
