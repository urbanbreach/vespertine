// The headless compound Dialog for React (CMP-003 R1-R10) over the JavaScript reference tree.

import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import { Dialog, DismissableLayer, Select, flushSync } from "../src/index.js";
import { h, microtasks, mount, settle } from "./components-helpers.js";

function dialogApp({ root = {}, trigger = {}, portal = {}, backdrop = {}, popup = {}, close = {}, withTrigger = true } = {}) {
  return h(
    "div",
    { testId: "page" },
    h(
      Dialog.Root,
      root,
      withTrigger ? h(Dialog.Trigger, { testId: "trigger", ...trigger }, h("text", {}, "Open")) : null,
      h(
        Dialog.Portal,
        { testId: "portal", ...portal },
        h(Dialog.Backdrop, { testId: "backdrop", ...backdrop }),
        h(
          Dialog.Popup,
          { testId: "popup", ...popup },
          h(Dialog.Title, { testId: "title" }, h("text", {}, "Title")),
          h(Dialog.Description, { testId: "description" }, h("text", {}, "Body")),
          h("div", { testId: "field", tabIndex: 0 }),
          h(Dialog.Close, { testId: "close", ...close }, h("text", {}, "Close")),
        ),
      ),
    ),
  );
}

const calls = (ui, name) => ui.renderer.calls.filter((call) => call[0] === name);

test("CMP-003 R1: a dialog is closed by default and the portal and popup mount only while open", () => {
  const ui = mount(dialogApp());
  assert.equal(ui.has("portal"), false);
  assert.equal(ui.has("popup"), false);
  assert.equal(ui.has("backdrop"), false);
  assert.ok(ui.has("trigger"));
  const open = mount(dialogApp({ root: { defaultOpen: true } }));
  assert.ok(open.has("portal") && open.has("backdrop") && open.has("popup"));
});

test("CMP-003 R1: open is controlled with open, uncontrolled with defaultOpen, and reports only real changes", () => {
  const log = [];
  const controlled = mount(dialogApp({ root: { open: false, onOpenChange: (next) => log.push(next) } }));
  controlled.emit("trigger", { type: "click", button: 0 });
  assert.deepEqual(log, [true], "a controlled request is reported");
  assert.equal(controlled.has("popup"), false, "and never overwrites the supplied value");
  controlled.render(dialogApp({ root: { open: true, onOpenChange: (next) => log.push(next) } }));
  assert.ok(controlled.has("popup"), "the owner decides");
  controlled.emit("trigger", { type: "click", button: 0 });
  assert.deepEqual(log, [true], "an identical request (open while open) is not reported");
  controlled.emit("close", { type: "click", button: 0 });
  assert.deepEqual(log, [true, false]);

  const loose = mount(dialogApp({ root: { onOpenChange: (next) => log.push(`loose ${next}`) } }));
  loose.emit("trigger", { type: "click", button: 0 });
  assert.ok(loose.has("popup"));
  loose.emit("close", { type: "click", button: 0 });
  assert.equal(loose.has("popup"), false);
  assert.deepEqual(log.slice(2), ["loose true", "loose false"]);
});

test("CMP-003 R2: the trigger exposes the expanded state, runs the caller's activation first and never toggles", () => {
  const log = [];
  const ui = mount(dialogApp({ trigger: { onClick: () => log.push("caller") }, root: { onOpenChange: (open) => log.push(`open ${open}`) } }));
  assert.equal(ui.node("trigger").props.role, "button");
  assert.equal(ui.node("trigger").props["aria-expanded"], false);
  assert.equal(ui.node("trigger").props.tabIndex, 0);
  ui.emit("trigger", { type: "click", button: 0 });
  assert.deepEqual(log, ["caller", "open true"], "the caller's activation runs first, then the open request");
  assert.equal(ui.node("trigger").props["aria-expanded"], true);
  ui.emit("trigger", { type: "click", button: 0 });
  assert.deepEqual(log, ["caller", "open true", "caller"], "a second activation does not toggle");
  assert.ok(ui.has("popup"));
});

test("CMP-003 R2: the trigger and the close part use the button keyboard behavior", () => {
  const ui = mount(dialogApp());
  ui.key("trigger", "enter");
  assert.ok(ui.has("popup"), "Enter opens");
  ui.key("close", "space");
  assert.equal(ui.has("popup"), false, "Space closes through the close part");
  const disabled = mount(dialogApp({ trigger: { disabled: true } }));
  assert.equal(disabled.node("trigger").props.tabIndex, -1);
  disabled.emit("trigger", { type: "click", button: 0 });
  assert.equal(disabled.has("popup"), false);
});

test("CMP-003 R2: the close part runs the caller's activation first and then requests open = false", () => {
  const log = [];
  const ui = mount(dialogApp({ root: { defaultOpen: true, onOpenChange: (open) => log.push(`open ${open}`) }, close: { onClick: () => log.push("caller") } }));
  ui.emit("close", { type: "click", button: 0 });
  assert.deepEqual(log, ["caller", "open false"]);
  assert.equal(ui.has("popup"), false);
});

test("CMP-003 R2: the trigger and the close part support asChild", () => {
  const log = [];
  const ui = mount(
    h(
      Dialog.Root,
      {},
      h(Dialog.Trigger, { asChild: true }, h("div", { testId: "own-trigger", onClick: () => log.push("child") })),
      h(
        Dialog.Portal,
        {},
        h(Dialog.Popup, { testId: "popup" }, h(Dialog.Close, { asChild: true }, h("div", { testId: "own-close" }))),
      ),
    ),
  );
  assert.equal(ui.node("own-trigger").props.role, "button");
  assert.equal(ui.node("own-trigger").props.tabIndex, 0);
  ui.emit("own-trigger", { type: "click", button: 0 });
  assert.deepEqual(log, ["child"], "the child's handler runs too");
  assert.ok(ui.has("popup"));
  ui.emit("own-close", { type: "click", button: 0 });
  assert.equal(ui.has("popup"), false);
});

test("CMP-003 R3: the portal is a deferred full-window layer at priority 0 that centers its children on a transparent background", () => {
  const ui = mount(dialogApp({ root: { defaultOpen: true } }));
  const portal = ui.node("portal");
  assert.equal(portal.type, "anchored");
  assert.equal(portal.props.fill, "window");
  assert.equal(portal.props.deferred, true);
  assert.equal(portal.props.priority, 0);
  assert.equal(portal.props.occlude, true, "a modal portal occludes the page beneath it");
  assert.equal(portal.style.display, "flex");
  assert.equal(portal.style.alignItems, "center");
  assert.equal(portal.style.justifyContent, "center");
  assert.equal(portal.style.backgroundColor, "transparent");
  assert.equal(portal.style.pointerEvents, undefined);
});

test("CMP-003 R3: a nonmodal portal disables pointer events on its surface and the caller's style may override the layout", () => {
  const nonmodal = mount(dialogApp({ root: { defaultOpen: true, modal: false } }));
  assert.equal(nonmodal.node("portal").props.occlude, false);
  assert.equal(nonmodal.node("portal").style.pointerEvents, "none");
  const custom = mount(dialogApp({ root: { defaultOpen: true }, portal: { style: { alignItems: "flex-start", backgroundColor: "#112233" } } }));
  assert.equal(custom.node("portal").style.alignItems, "flex-start");
  assert.equal(custom.node("portal").style.justifyContent, "center", "defaults the caller leaves alone stay");
  assert.equal(custom.node("portal").style.backgroundColor, "#112233");
});

test("CMP-003 R4: the backdrop covers all four edges, runs the caller's press first and then closes", () => {
  const log = [];
  const ui = mount(dialogApp({ root: { defaultOpen: true, onOpenChange: (open) => log.push(`open ${open}`) }, backdrop: { onPointerDown: () => log.push("caller") } }));
  const style = ui.node("backdrop").style;
  assert.deepEqual([style.position, style.top, style.left, style.right, style.bottom], ["absolute", 0, 0, 0, 0]);
  assert.equal(style.backgroundColor, undefined, "no color is imposed");
  ui.emit("backdrop", { type: "pointerDown", button: 0 });
  assert.deepEqual(log, ["caller", "open false"]);
  assert.equal(ui.has("popup"), false);
});

test("CMP-003 R4: disablePointerDismissal keeps the dialog open on a backdrop press (the caller's code still runs)", () => {
  const log = [];
  const ui = mount(dialogApp({ root: { defaultOpen: true, disablePointerDismissal: true }, backdrop: { onPointerDown: () => log.push("caller") } }));
  ui.emit("backdrop", { type: "pointerDown", button: 0 });
  assert.deepEqual(log, ["caller"]);
  assert.ok(ui.has("popup"));
});

test("CMP-003 R5: popup defaults are role dialog, tab index -1, pointer events auto and no keyboard focus dim; the caller's properties win", () => {
  const ui = mount(dialogApp({ root: { defaultOpen: true } }));
  const popup = ui.node("popup");
  assert.equal(popup.props.role, "dialog");
  assert.equal(popup.props.tabIndex, -1);
  assert.equal(popup.style.pointerEvents, "auto");
  assert.equal(popup.style.keyboardFocusDim, false);
  assert.equal(popup.style.padding, undefined, "no padding, radius or shadow is imposed");
  const custom = mount(dialogApp({ root: { defaultOpen: true }, popup: { role: "alertdialog", tabIndex: 0, style: { pointerEvents: "none", padding: 4 } } }));
  assert.equal(custom.node("popup").props.role, "alertdialog");
  assert.equal(custom.node("popup").props.tabIndex, 0);
  assert.equal(custom.node("popup").style.pointerEvents, "none");
  assert.equal(custom.node("popup").style.padding, 4);
});

test("CMP-003 R8: the title is a level 2 heading that the caller's properties may override; the description is an ordinary container", () => {
  const ui = mount(
    h(
      Dialog.Root,
      { defaultOpen: true },
      h(
        Dialog.Portal,
        {},
        h(
          Dialog.Popup,
          {},
          h(Dialog.Title, { testId: "title" }, "T"),
          h(Dialog.Title, { testId: "other", "aria-level": 3 }, "U"),
          h(Dialog.Title, { testId: "plain", role: "presentation" }, "V"),
          h(Dialog.Description, { testId: "description" }, "D"),
        ),
      ),
    ),
  );
  assert.equal(ui.node("title").props.role, "heading");
  assert.equal(ui.node("title").props["aria-level"], 2);
  assert.equal(ui.node("other").props["aria-level"], 3);
  assert.equal(ui.node("plain").props.role, "presentation");
  assert.equal(ui.node("description").type, "div");
  assert.equal(ui.node("description").props.role, undefined);
});

test("CMP-003: a part outside its root is reported", () => {
  const trigger = mount(h(Dialog.Trigger, {}));
  assert.match(String(trigger.errors[0]), /Dialog\.Trigger must be used inside Dialog\.Root/);
  const popup = mount(h(Dialog.Popup, {}));
  assert.match(String(popup.errors[0]), /Dialog\.Popup must be used inside Dialog\.Root/);
});

test("CMP-003 R6: the popup takes focus on opening and focus returns to the trigger when it is mounted", () => {
  const ui = mount(dialogApp());
  ui.renderer.focusElement(ui.node("trigger").id);
  ui.emit("trigger", { type: "click", button: 0 });
  assert.equal(ui.focused(), ui.node("popup").id, "the popup itself takes the initial focus");
  ui.emit("close", { type: "click", button: 0 });
  assert.equal(ui.focused(), ui.node("trigger").id);
});

test("CMP-003 R6: with no trigger focus returns to the element focused before opening", () => {
  let setOpen;
  function Owner() {
    const [open, setter] = React.useState(false);
    setOpen = setter;
    return h(
      "div",
      {},
      h("div", { testId: "other", tabIndex: 0 }),
      h(Dialog.Root, { open, onOpenChange: setter }, h(Dialog.Portal, {}, h(Dialog.Popup, { testId: "popup" }, h(Dialog.Close, { testId: "close" })))),
    );
  }
  const ui = mount(h(Owner));
  ui.renderer.focusElement(ui.node("other").id);
  flushSync(() => setOpen(true));
  assert.ok(ui.has("popup"));
  assert.equal(ui.focused(), ui.node("popup").id);
  ui.emit("close", { type: "click", button: 0 });
  assert.equal(ui.has("popup"), false);
  assert.equal(ui.focused(), ui.node("other").id, "the captured owner is restored");
});

test("CMP-003 R6: explicit initialFocus and finalFocus targets use the focus-target contract, including suppression", () => {
  const field = React.createRef();
  const outside = React.createRef();
  const app = (popup) =>
    h(
      "div",
      {},
      h("div", { ref: outside, testId: "outside", tabIndex: 0 }),
      h(
        Dialog.Root,
        {},
        h(Dialog.Trigger, { testId: "trigger" }),
        h(
          Dialog.Portal,
          {},
          h(Dialog.Popup, { testId: "popup", ...popup }, h("div", { ref: field, testId: "field", tabIndex: 0 }), h(Dialog.Close, { testId: "close" })),
        ),
      ),
    );
  const explicit = mount(app({ initialFocus: field, finalFocus: outside }));
  explicit.emit("trigger", { type: "click", button: 0 });
  assert.equal(explicit.focused(), explicit.node("field").id, "a ref target");
  explicit.emit("close", { type: "click", button: 0 });
  assert.equal(explicit.focused(), explicit.node("outside").id, "final focus beats the trigger");

  const functions = mount(app({ initialFocus: () => field.current, finalFocus: (previous) => (previous === null ? false : outside.current) }));
  functions.renderer.focusElement(functions.node("trigger").id);
  functions.emit("trigger", { type: "click", button: 0 });
  assert.equal(functions.focused(), functions.node("field").id, "a function target");
  functions.emit("close", { type: "click", button: 0 });
  assert.equal(functions.focused(), functions.node("outside").id, "the final-focus function receives the captured owner");

  const suppressed = mount(app({ initialFocus: false, finalFocus: false }));
  suppressed.renderer.focusElement(suppressed.node("outside").id);
  suppressed.emit("trigger", { type: "click", button: 0 });
  assert.equal(suppressed.focused(), suppressed.node("outside").id, "initialFocus false moves nothing");
  const before = calls(suppressed, "focusElement").length;
  suppressed.emit("close", { type: "click", button: 0 });
  assert.equal(calls(suppressed, "focusElement").length, before, "finalFocus false moves nothing");
});

test("CMP-003 R7: a modal popup prevents Tab after the caller's key-down and moves focus within itself, Shift reversing", () => {
  const log = [];
  const ui = mount(dialogApp({ root: { defaultOpen: true }, popup: { onKeyDown: (event) => log.push(["caller", event.key]) } }));
  const popup = ui.node("popup").id;
  ui.keyDown("popup", "tab");
  assert.deepEqual(calls(ui, "focusNextWithin"), [["focusNextWithin", popup]]);
  ui.keyDown("popup", "tab", { shift: true });
  assert.deepEqual(calls(ui, "focusPreviousWithin"), [["focusPreviousWithin", popup]]);
  assert.deepEqual(calls(ui, "focusNext"), [], "the page's Tab traversal never runs");
  assert.deepEqual(log, [["caller", "tab"], ["caller", "tab"]], "the caller's key-down runs first");
});

test("CMP-003 R7: a caller that prevents Tab, a modified Tab and a nonmodal popup are not trapped", () => {
  const prevented = mount(dialogApp({ root: { defaultOpen: true }, popup: { onKeyDown: (event) => event.preventDefault() } }));
  prevented.keyDown("popup", "tab");
  assert.deepEqual(calls(prevented, "focusNextWithin"), []);
  const modified = mount(dialogApp({ root: { defaultOpen: true } }));
  modified.keyDown("popup", "tab", { ctrl: true });
  assert.deepEqual(calls(modified, "focusNextWithin"), []);
  const nonmodal = mount(dialogApp({ root: { defaultOpen: true, modal: false } }));
  nonmodal.keyDown("popup", "tab");
  assert.deepEqual(calls(nonmodal, "focusNextWithin"), [], "a nonmodal popup never traps Tab");
});

test("CMP-003 R9: Escape closes the dialog after the caller's handler and focus goes to the trigger; the handler may prevent it", () => {
  const log = [];
  const ui = mount(dialogApp({ root: { defaultOpen: true, onOpenChange: (open) => log.push(`open ${open}`) }, popup: { onEscapeKeyDown: () => log.push("handler") } }));
  ui.windowKey("escape");
  assert.deepEqual(log, ["handler", "open false"]);
  assert.equal(ui.has("popup"), false);
  assert.equal(ui.focused(), ui.node("trigger").id);
  const kept = mount(dialogApp({ root: { defaultOpen: true }, popup: { onEscapeKeyDown: (event) => event.preventDefault() } }));
  kept.windowKey("escape");
  assert.ok(kept.has("popup"));
});

test("CMP-003 R9: Escape closes a select opened from inside the dialog first, then the dialog", () => {
  const ui = mount(
    h(
      Dialog.Root,
      { defaultOpen: true },
      h(
        Dialog.Portal,
        {},
        h(
          Dialog.Popup,
          { testId: "popup" },
          h(
            Select.Root,
            { defaultOpen: true },
            h(Select.Trigger, { testId: "select-trigger" }),
            h(Select.Content, { testId: "select-content" }, h(Select.Item, { value: "a" }, "A")),
          ),
        ),
      ),
    ),
  );
  ui.windowKey("escape");
  assert.equal(ui.has("select-content"), false, "the select, the top layer, takes the first Escape");
  assert.ok(ui.has("popup"));
  ui.windowKey("escape");
  assert.equal(ui.has("popup"), false, "the next Escape closes the dialog");
});

test("CMP-003 R10: with escape dismissal disabled Escape does not close the dialog but is still consumed by it", () => {
  const log = [];
  const ui = mount(
    h(
      DismissableLayer,
      { testId: "page-layer", onEscapeKeyDown: () => log.push("below") },
      dialogApp({ root: { defaultOpen: true, disableEscapeDismissal: true }, popup: { onEscapeKeyDown: () => log.push("popup handler") } }),
    ),
  );
  ui.windowKey("escape");
  assert.ok(ui.has("popup"), "the dialog stays open");
  assert.deepEqual(log, ["popup handler"], "and the layer below never sees the Escape");
});

test("CMP-003 R10: a nonmodal outside press closes the dialog unless pointer dismissal is disabled; a modal dialog ignores it", async () => {
  const nonmodal = mount(dialogApp({ root: { defaultOpen: true, modal: false } }));
  nonmodal.emit("popup", { type: "outsideDown", x: 1, y: 1 });
  await microtasks();
  assert.equal(nonmodal.has("popup"), false);
  const kept = mount(dialogApp({ root: { defaultOpen: true, modal: false, disablePointerDismissal: true } }));
  kept.emit("popup", { type: "outsideDown", x: 1, y: 1 });
  await microtasks();
  assert.ok(kept.has("popup"));
  const modal = mount(dialogApp({ root: { defaultOpen: true } }));
  modal.emit("popup", { type: "outsideDown", x: 1, y: 1 });
  await microtasks();
  assert.ok(modal.has("popup"), "a modal dialog is dismissed through its backdrop only");
});

test("CMP-003 R9: a press on the trigger of an open nonmodal dialog does not dismiss it first", async () => {
  const log = [];
  const ui = mount(dialogApp({ root: { defaultOpen: true, modal: false, onOpenChange: (open) => log.push(open) } }));
  ui.emit("popup", { type: "outsideDown", x: 1, y: 1 });
  ui.emit("trigger", { type: "pointerDown", button: 0 });
  await microtasks();
  ui.emit("trigger", { type: "click", button: 0 });
  assert.ok(ui.has("popup"), "the trigger's press flag keeps the outside press from closing it");
  assert.deepEqual(log, []);
  await settle();
});
