// The headless Dialog on the native offscreen test host (CMP-003 A1, A2) for Solid: real layout,
// hit testing, focus traversal and the dismissal path.

import assert from "node:assert/strict";
import { test } from "node:test";
import { Button, Dialog, Select, createSignal, mergeProps } from "../src/index.js";
import { createTestHost, testHostAvailable } from "../src/testing.js";
import { c, h, settle } from "./components-helpers.js";

const available = testHostAvailable();
const native = (name, body) => test(name, { skip: available ? false : "no GPU test backend" }, body);

const find = (host, testId) => host.renderer.findByTestId(testId)[0] ?? null;
const bounds = (host, testId) => find(host, testId)?.bounds;
const center = ({ x, y, width, height }) => [x + width / 2, y + height / 2];
const focusedId = (host, testId) => host.renderer.getFocusedElementId() === find(host, testId)?.id;

async function flush(host) {
  await settle();
  host.renderer.flush();
}

async function click(host, testId) {
  host.renderer.nativeSimulateClick(...center(bounds(host, testId)));
  await flush(host);
}

async function press(host, keys) {
  host.renderer.simulateKeystrokes(keys);
  await flush(host);
}

const chip = { width: 100, height: 30, backgroundColor: "#303030" };
const label = (text) => h("text", {}, text);

function dialogApp({ root = {}, popup = {}, inside, onPage } = {}) {
  return h(
    "div",
    { style: { width: 600, height: 400, flexDirection: "column", paddingLeft: 20, paddingTop: 20, gap: 10 } },
    c(
      Dialog.Root,
      root,
      () => c(Dialog.Trigger, { testId: "trigger", style: chip }, () => label("Open")),
      () => h("div", { testId: "page-button", tabIndex: 0, onClick: () => onPage?.(), style: { ...chip, width: 80 } }),
      () =>
        c(
          Dialog.Portal,
          { testId: "portal" },
          () => c(Dialog.Backdrop, { testId: "backdrop", style: { backgroundColor: "#00000080" } }),
          () =>
            c(
              Dialog.Popup,
              mergeProps({ testId: "popup", style: { width: 320, flexDirection: "column", padding: 16, gap: 8, backgroundColor: "#202020" } }, popup),
              () => c(Dialog.Title, { testId: "title" }, () => label("Title")),
              () =>
                inside?.() ??
                h(
                  "div",
                  { style: { flexDirection: "column", gap: 8 } },
                  c(Button, { testId: "first", style: chip }, () => label("First")),
                  c(Button, { testId: "second", style: chip }, () => label("Second")),
                  c(Dialog.Close, { testId: "close", style: chip }, () => label("Close")),
                ),
            ),
        ),
    ),
  );
}

/** First, Close, then a select last, so its rows extend below the popup. */
function selectInside() {
  return h(
    "div",
    { style: { flexDirection: "column", gap: 8 } },
    c(Button, { testId: "first", style: chip }, () => label("First")),
    c(Dialog.Close, { testId: "close", style: chip }, () => label("Close")),
    c(
      Select.Root,
      { testId: "select" },
      () => c(Select.Trigger, { testId: "select-trigger", style: chip }, () => c(Select.Value, { placeholder: "Pick" })),
      () =>
        c(
          Select.Content,
          { testId: "select-content", sideOffset: 4, style: { width: 120, flexDirection: "column", padding: 4 } },
          () => c(Select.Item, { testId: "item-a", value: "a", style: { height: 24 } }, () => label("Apple")),
          () => c(Select.Item, { testId: "item-b", value: "b", style: { height: 24 } }, () => label("Banana")),
        ),
    ),
  );
}

native("CMP-003 A1: Tab cycles inside a modal popup and wraps last to first, Shift-Tab wraps first to last", async () => {
  const host = createTestHost({ width: 600, height: 400 });
  host.render(() => dialogApp());
  assert.equal(find(host, "popup"), null, "closed by default: nothing is mounted");
  await click(host, "trigger");
  assert.ok(find(host, "popup"), "the trigger opens the dialog");
  assert.ok(focusedId(host, "popup"), "the popup itself takes focus first");
  await press(host, "tab");
  assert.ok(focusedId(host, "first"), "the first Tab enters the first focusable descendant");
  await press(host, "tab");
  assert.ok(focusedId(host, "second"));
  await press(host, "tab");
  assert.ok(focusedId(host, "close"));
  await press(host, "tab");
  assert.ok(focusedId(host, "first"), "Tab wraps from the last to the first and never reaches the page");
  await press(host, "shift-tab");
  assert.ok(focusedId(host, "close"), "Shift-Tab wraps from the first to the last");
  await press(host, "shift-tab");
  assert.ok(focusedId(host, "second"));
  assert.deepEqual(host.errors, []);
});

native("CMP-003 A1: a nested select's Escape closes the select first, the next Escape closes the dialog and focus returns to the trigger", async () => {
  const host = createTestHost({ width: 600, height: 400 });
  host.render(() => dialogApp({ inside: selectInside }));
  await click(host, "trigger");
  assert.ok(find(host, "popup"));
  await click(host, "select-trigger");
  assert.ok(find(host, "select-content"), "the select opens inside the dialog");
  await press(host, "escape");
  assert.equal(find(host, "select-content"), null, "the first Escape closes the select only");
  assert.ok(find(host, "popup"), "the dialog stays open");
  await press(host, "escape");
  assert.equal(find(host, "popup"), null, "the next Escape closes the dialog");
  assert.ok(focusedId(host, "trigger"), "focus returns to the trigger");
  assert.deepEqual(host.errors, []);
});

native("CMP-003 A1: choosing a select row inside the dialog leaves it open; Close closes it and the trigger opens it again", async () => {
  const host = createTestHost({ width: 600, height: 400 });
  host.render(() => dialogApp({ inside: selectInside }));
  await click(host, "trigger");
  await click(host, "select-trigger");
  await click(host, "item-b");
  assert.equal(find(host, "select-content"), null);
  assert.ok(find(host, "popup"), "a press inside a nested layer is not a backdrop press");
  await click(host, "close");
  assert.equal(find(host, "popup"), null, "Close closes");
  assert.ok(focusedId(host, "trigger"));
  await click(host, "trigger");
  assert.ok(find(host, "popup"), "the trigger opens it again");
  assert.deepEqual(host.errors, []);
});

native("CMP-003 A2: explicit initial and final focus targets win, and false suppresses the movement", async () => {
  const host = createTestHost({ width: 600, height: 400 });
  let second = null;
  let page = null;
  const app = (popup) =>
    h(
      "div",
      { style: { width: 600, height: 400, flexDirection: "column", padding: 20, gap: 10 } },
      c(
        Dialog.Root,
        {},
        () => h("div", { ref: (node) => (page = node), testId: "page-target", tabIndex: 0, style: chip }),
        () => c(Dialog.Trigger, { testId: "trigger", style: chip }, () => label("Open")),
        () =>
          c(
            Dialog.Portal,
            {},
            () =>
              c(
                Dialog.Popup,
                mergeProps({ testId: "popup", style: { width: 240, flexDirection: "column", padding: 8, backgroundColor: "#202020" } }, popup),
                () => c(Button, { testId: "first", style: chip }, () => label("First")),
                () => c(Button, { ref: (node) => (second = node), testId: "second", style: chip }, () => label("Second")),
                () => c(Dialog.Close, { testId: "close", style: chip }, () => label("Close")),
              ),
          ),
      ),
    );
  host.render(() =>
    app({
      get initialFocus() {
        return second;
      },
      get finalFocus() {
        return page;
      },
    }),
  );
  await click(host, "trigger");
  assert.ok(focusedId(host, "second"), "an explicit initial focus element wins over the popup default");
  await click(host, "close");
  assert.equal(find(host, "popup"), null);
  assert.ok(focusedId(host, "page-target"), "an explicit final focus element wins over the trigger");

  host.unmount();
  const suppressed = createTestHost({ width: 600, height: 400 });
  suppressed.render(() => app({ initialFocus: false, finalFocus: false }));
  await click(suppressed, "trigger");
  assert.ok(focusedId(suppressed, "trigger"), "initialFocus false leaves focus where it was");
  await click(suppressed, "close");
  assert.equal(find(suppressed, "popup"), null);
  assert.equal(focusedId(suppressed, "trigger"), false, "finalFocus false does not move focus to the trigger");
  assert.deepEqual(host.errors.concat(suppressed.errors), []);
});

native("CMP-003 A2: with no trigger, closing restores the previously focused element", async () => {
  const host = createTestHost({ width: 600, height: 400 });
  const [open, setOpen] = createSignal(false);
  host.render(() =>
    h(
      "div",
      { style: { width: 600, height: 400, flexDirection: "column", padding: 20, gap: 10 } },
      c(
        Dialog.Root,
        {
          get open() {
            return open();
          },
          onOpenChange: setOpen,
        },
        () => h("div", { testId: "opener", tabIndex: 0, style: chip }),
        () =>
          c(
            Dialog.Portal,
            {},
            () =>
              c(
                Dialog.Popup,
                { testId: "popup", style: { width: 240, flexDirection: "column", padding: 8, backgroundColor: "#202020" } },
                () => c(Button, { testId: "first", style: chip }, () => label("First")),
                () => c(Dialog.Close, { testId: "close", style: chip }, () => label("Close")),
              ),
          ),
      ),
    ),
  );
  await click(host, "opener");
  assert.ok(focusedId(host, "opener"));
  host.root.flushSync(() => setOpen(true));
  await flush(host);
  assert.ok(find(host, "popup"), "the owner opened it");
  assert.ok(focusedId(host, "popup"));
  await click(host, "close");
  assert.equal(find(host, "popup"), null);
  assert.ok(focusedId(host, "opener"), "focus returns to the element focused before opening");
  assert.deepEqual(host.errors, []);
});

native("CMP-003 A2: a press in the popup does not close it; a press on the backdrop closes it unless pointer dismissal is disabled", async () => {
  const host = createTestHost({ width: 600, height: 400 });
  host.render(() => dialogApp());
  await click(host, "trigger");
  const popup = bounds(host, "popup");
  assert.ok(find(host, "backdrop"));
  host.renderer.nativeSimulateClick(popup.x + 4, popup.y + 4);
  await flush(host);
  assert.ok(find(host, "popup"), "a press inside the popup never reaches the backdrop");
  host.renderer.nativeSimulateClick(...center(bounds(host, "title")));
  await flush(host);
  assert.ok(find(host, "popup"), "nor does a press on a part of the popup");
  host.renderer.nativeSimulateClick(590, 390);
  await flush(host);
  assert.equal(find(host, "popup"), null, "a press on the backdrop closes the dialog");
  assert.ok(focusedId(host, "trigger"), "focus returns to the trigger");
  host.unmount();

  const kept = createTestHost({ width: 600, height: 400 });
  kept.render(() => dialogApp({ root: { disablePointerDismissal: true } }));
  await click(kept, "trigger");
  kept.renderer.nativeSimulateClick(590, 390);
  await flush(kept);
  assert.ok(find(kept, "popup"), "disablePointerDismissal keeps it open on a backdrop press");
  await click(kept, "close");
  assert.equal(find(kept, "popup"), null, "Close still closes");
  assert.deepEqual(host.errors.concat(kept.errors), []);
});

native("CMP-003 A2: a modal portal keeps the page from receiving presses", async () => {
  const clicks = [];
  const host = createTestHost({ width: 600, height: 400 });
  host.render(() => dialogApp({ onPage: () => clicks.push("page"), root: { defaultOpen: true } }));
  await flush(host);
  assert.ok(find(host, "popup"), "defaultOpen mounts the popup");
  host.renderer.nativeSimulateClick(...center(bounds(host, "page-button")));
  await flush(host);
  assert.deepEqual(clicks, [], "the page does not see the press");
  assert.equal(find(host, "popup"), null, "the press landed on the backdrop, which closes the dialog");
  assert.deepEqual(host.errors, []);
});

native("CMP-003 A2: a nonmodal portal passes pointer input to the page and does not trap Tab", async () => {
  const clicks = [];
  const host = createTestHost({ width: 600, height: 400 });
  host.render(() => dialogApp({ onPage: () => clicks.push("page"), root: { modal: false, disablePointerDismissal: true } }));
  await click(host, "trigger");
  assert.ok(find(host, "popup"));
  host.renderer.nativeSimulateClick(...center(bounds(host, "page-button")));
  await flush(host);
  assert.deepEqual(clicks, ["page"], "the page stays interactive around a nonmodal dialog");
  assert.ok(find(host, "popup"), "disablePointerDismissal keeps a nonmodal dialog open on an outside press");
  host.renderer.nativeSimulateClick(...center(bounds(host, "second")));
  await flush(host);
  assert.ok(find(host, "popup"), "a press inside the popup is no outside press");
  host.renderer.focusElement(find(host, "close").id);
  await press(host, "tab");
  assert.equal(focusedId(host, "first"), false, "Tab leaves the last control instead of wrapping inside the popup");
  assert.ok(find(host, "popup"));
  assert.deepEqual(host.errors, []);
});

native("CMP-003 A2: a nonmodal outside press dismisses unless disabled, and the page still receives it", async () => {
  const clicks = [];
  const host = createTestHost({ width: 600, height: 400 });
  host.render(() => dialogApp({ onPage: () => clicks.push("page"), root: { modal: false } }));
  await click(host, "trigger");
  assert.ok(find(host, "popup"));
  host.renderer.nativeSimulateClick(...center(bounds(host, "page-button")));
  await flush(host);
  assert.equal(find(host, "popup"), null, "an outside press closes a nonmodal dialog");
  assert.deepEqual(clicks, ["page"], "and is not consumed");
  assert.equal(focusedId(host, "trigger"), false, "focus stays where the press put it");
  assert.ok(focusedId(host, "page-button"));
  assert.deepEqual(host.errors, []);
});

native("CMP-003 R9: presses in a nested layer never count as outside presses of a nonmodal dialog", async () => {
  const host = createTestHost({ width: 600, height: 400 });
  host.render(() => dialogApp({ inside: selectInside, root: { modal: false } }));
  await click(host, "trigger");
  await click(host, "select-trigger");
  assert.ok(find(host, "select-content"));
  const popup = bounds(host, "popup");
  const row = bounds(host, "item-b");
  assert.ok(row.y + row.height > popup.y + popup.height, "the row lies outside the popup's bounds");
  await click(host, "item-b");
  assert.equal(find(host, "select-content"), null, "the row selects and closes the select");
  assert.ok(find(host, "popup"), "the dialog is not dismissed by a press that belonged to the select");
  assert.deepEqual(host.errors, []);
});
