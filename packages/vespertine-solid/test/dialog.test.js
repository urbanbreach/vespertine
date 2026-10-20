// The headless compound Dialog for Solid (CMP-003 R1-R10) over the JavaScript reference tree.

import assert from "node:assert/strict";
import { test } from "node:test";
import { Dialog, DismissableLayer, Select, createSignal, mergeProps } from "../src/index.js";
import { c, h, microtasks, mount, settle } from "./components-helpers.js";

const text = (content) => h("text", {}, content);

function dialogApp({ root = {}, trigger = {}, portal = {}, backdrop = {}, popup = {}, close = {}, withTrigger = true } = {}) {
  return h(
    "div",
    { testId: "page" },
    c(
      Dialog.Root,
      root,
      () => (withTrigger ? c(Dialog.Trigger, mergeProps({ testId: "trigger" }, trigger), () => text("Open")) : null),
      () =>
        c(
          Dialog.Portal,
          mergeProps({ testId: "portal" }, portal),
          () => c(Dialog.Backdrop, mergeProps({ testId: "backdrop" }, backdrop)),
          () =>
            c(
              Dialog.Popup,
              mergeProps({ testId: "popup" }, popup),
              () => c(Dialog.Title, { testId: "title" }, () => text("Title")),
              () => c(Dialog.Description, { testId: "description" }, () => text("Body")),
              () => h("div", { testId: "field", tabIndex: 0 }),
              () => c(Dialog.Close, mergeProps({ testId: "close" }, close), () => text("Close")),
            ),
        ),
    ),
  );
}

const calls = (ui, name) => ui.renderer.calls.filter((call) => call[0] === name);
const click = (ui, testId) => ui.emit(testId, { type: "click", button: 0 });

test("CMP-003 R1: a dialog is closed by default and the portal and popup mount only while open", () => {
  const ui = mount(() => dialogApp());
  assert.equal(ui.has("portal"), false);
  assert.equal(ui.has("popup"), false);
  assert.equal(ui.has("backdrop"), false);
  assert.ok(ui.has("trigger"));
  const open = mount(() => dialogApp({ root: { defaultOpen: true } }));
  assert.ok(open.has("portal") && open.has("backdrop") && open.has("popup"));
});

test("CMP-003 R1: open is controlled with open, uncontrolled with defaultOpen, and reports only real changes", () => {
  const log = [];
  const [open, setOpen] = createSignal(false);
  const controlled = mount(() =>
    dialogApp({
      root: {
        get open() {
          return open();
        },
        onOpenChange: (next) => log.push(next),
      },
    }),
  );
  click(controlled, "trigger");
  assert.deepEqual(log, [true], "a controlled request is reported");
  assert.equal(controlled.has("popup"), false, "and never overwrites the supplied value");
  controlled.set(() => setOpen(true));
  assert.ok(controlled.has("popup"), "the owner decides");
  click(controlled, "trigger");
  assert.deepEqual(log, [true], "an identical request (open while open) is not reported");
  click(controlled, "close");
  assert.deepEqual(log, [true, false]);

  const loose = mount(() => dialogApp({ root: { onOpenChange: (next) => log.push(`loose ${next}`) } }));
  click(loose, "trigger");
  assert.ok(loose.has("popup"));
  click(loose, "close");
  assert.equal(loose.has("popup"), false);
  assert.deepEqual(log.slice(2), ["loose true", "loose false"]);
});

test("CMP-003 R2: the trigger exposes the expanded state, runs the caller's activation first and never toggles", () => {
  const log = [];
  const ui = mount(() => dialogApp({ trigger: { onClick: () => log.push("caller") }, root: { onOpenChange: (open) => log.push(`open ${open}`) } }));
  assert.equal(ui.node("trigger").props.role, "button");
  assert.equal(ui.node("trigger").props["aria-expanded"], false);
  assert.equal(ui.node("trigger").props.tabIndex, 0);
  click(ui, "trigger");
  assert.deepEqual(log, ["caller", "open true"], "the caller's activation runs first, then the open request");
  assert.equal(ui.node("trigger").props["aria-expanded"], true);
  click(ui, "trigger");
  assert.deepEqual(log, ["caller", "open true", "caller"], "a second activation does not toggle");
  assert.ok(ui.has("popup"));
});

test("CMP-003 R2: the trigger and the close part use the button keyboard behavior", () => {
  const ui = mount(() => dialogApp());
  ui.key("trigger", "enter");
  assert.ok(ui.has("popup"), "Enter opens");
  ui.key("close", "space");
  assert.equal(ui.has("popup"), false, "Space closes through the close part");
  const disabled = mount(() => dialogApp({ trigger: { disabled: true } }));
  assert.equal(disabled.node("trigger").props.tabIndex, -1);
  click(disabled, "trigger");
  assert.equal(disabled.has("popup"), false);
});

test("CMP-003 R2: the close part runs the caller's activation first and then requests open = false", () => {
  const log = [];
  const ui = mount(() => dialogApp({ root: { defaultOpen: true, onOpenChange: (open) => log.push(`open ${open}`) }, close: { onClick: () => log.push("caller") } }));
  click(ui, "close");
  assert.deepEqual(log, ["caller", "open false"]);
  assert.equal(ui.has("popup"), false);
});

test("CMP-003 R2 (Solid): the trigger and the close part merge onto a host element child with asChild", () => {
  const log = [];
  const ui = mount(() =>
    c(
      Dialog.Root,
      {},
      () => c(Dialog.Trigger, { asChild: true }, () => h("div", { testId: "own-trigger", onClick: () => log.push("child") })),
      () =>
        c(Dialog.Portal, {}, () =>
          c(Dialog.Popup, { testId: "popup" }, () => c(Dialog.Close, { asChild: true }, () => h("div", { testId: "own-close" }))),
        ),
    ),
  );
  assert.equal(ui.node("own-trigger").props.role, "button");
  assert.equal(ui.node("own-trigger").props.tabIndex, 0);
  click(ui, "own-trigger");
  assert.deepEqual(log, ["child"], "the child's handler runs too");
  assert.ok(ui.has("popup"));
  click(ui, "own-close");
  assert.equal(ui.has("popup"), false);
});

test("CMP-003 R3: the portal is a deferred full-window layer at priority 0 that centers its children on a transparent background", () => {
  const ui = mount(() => dialogApp({ root: { defaultOpen: true } }));
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
  const nonmodal = mount(() => dialogApp({ root: { defaultOpen: true, modal: false } }));
  assert.equal(nonmodal.node("portal").props.occlude, false);
  assert.equal(nonmodal.node("portal").style.pointerEvents, "none");
  const custom = mount(() => dialogApp({ root: { defaultOpen: true }, portal: { style: { alignItems: "flex-start", backgroundColor: "#112233" } } }));
  assert.equal(custom.node("portal").style.alignItems, "flex-start");
  assert.equal(custom.node("portal").style.justifyContent, "center", "defaults the caller leaves alone stay");
  assert.equal(custom.node("portal").style.backgroundColor, "#112233");
});

test("CMP-003 R3 (Solid): the modal option stays reactive", () => {
  const [modal, setModal] = createSignal(true);
  const ui = mount(() =>
    dialogApp({
      root: {
        defaultOpen: true,
        get modal() {
          return modal();
        },
      },
    }),
  );
  assert.equal(ui.node("portal").props.occlude, true);
  ui.set(() => setModal(false));
  assert.equal(ui.node("portal").props.occlude, false);
  assert.equal(ui.node("portal").style.pointerEvents, "none");
});

test("CMP-003 R4: the backdrop covers all four edges, runs the caller's press first and then closes", () => {
  const log = [];
  const ui = mount(() => dialogApp({ root: { defaultOpen: true, onOpenChange: (open) => log.push(`open ${open}`) }, backdrop: { onPointerDown: () => log.push("caller") } }));
  const style = ui.node("backdrop").style;
  assert.deepEqual([style.position, style.top, style.left, style.right, style.bottom], ["absolute", 0, 0, 0, 0]);
  assert.equal(style.backgroundColor, undefined, "no color is imposed");
  ui.emit("backdrop", { type: "pointerDown", button: 0 });
  assert.deepEqual(log, ["caller", "open false"]);
  assert.equal(ui.has("popup"), false);
});

test("CMP-003 R4: disablePointerDismissal keeps the dialog open on a backdrop press (the caller's code still runs)", () => {
  const log = [];
  const ui = mount(() => dialogApp({ root: { defaultOpen: true, disablePointerDismissal: true }, backdrop: { onPointerDown: () => log.push("caller") } }));
  ui.emit("backdrop", { type: "pointerDown", button: 0 });
  assert.deepEqual(log, ["caller"]);
  assert.ok(ui.has("popup"));
});

test("CMP-003 R5: popup defaults are role dialog, tab index -1, pointer events auto and no keyboard focus dim; the caller's properties win", () => {
  const ui = mount(() => dialogApp({ root: { defaultOpen: true } }));
  const popup = ui.node("popup");
  assert.equal(popup.props.role, "dialog");
  assert.equal(popup.props.tabIndex, -1);
  assert.equal(popup.style.pointerEvents, "auto");
  assert.equal(popup.style.keyboardFocusDim, false);
  assert.equal(popup.style.padding, undefined, "no padding, radius or shadow is imposed");
  const custom = mount(() => dialogApp({ root: { defaultOpen: true }, popup: { role: "alertdialog", tabIndex: 0, style: { pointerEvents: "none", padding: 4 } } }));
  assert.equal(custom.node("popup").props.role, "alertdialog");
  assert.equal(custom.node("popup").props.tabIndex, 0);
  assert.equal(custom.node("popup").style.pointerEvents, "none");
  assert.equal(custom.node("popup").style.padding, 4);
});

test("CMP-003 R8: the title is a level 2 heading that the caller's properties may override; the description is an ordinary container", () => {
  const ui = mount(() =>
    c(Dialog.Root, { defaultOpen: true }, () =>
      c(Dialog.Portal, {}, () =>
        c(
          Dialog.Popup,
          {},
          () => c(Dialog.Title, { testId: "title" }, () => text("T")),
          () => c(Dialog.Title, { testId: "other", "aria-level": 3 }, () => text("U")),
          () => c(Dialog.Title, { testId: "plain", role: "presentation" }, () => text("V")),
          () => c(Dialog.Description, { testId: "description" }, () => text("D")),
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

test("CMP-003 (Solid): a part outside its root throws", () => {
  assert.throws(() => mount(() => c(Dialog.Trigger, {})), /Dialog\.Trigger must be used inside Dialog\.Root/);
  assert.throws(() => mount(() => c(Dialog.Popup, {})), /Dialog\.Popup must be used inside Dialog\.Root/);
});

test("CMP-003 R6: the popup takes focus on opening and focus returns to the trigger when it is mounted", () => {
  const ui = mount(() => dialogApp());
  ui.renderer.focusElement(ui.node("trigger").id);
  click(ui, "trigger");
  assert.equal(ui.focused(), ui.node("popup").id, "the popup itself takes the initial focus");
  click(ui, "close");
  assert.equal(ui.focused(), ui.node("trigger").id);
});

test("CMP-003 R6: with no trigger focus returns to the element focused before opening", () => {
  const [open, setOpen] = createSignal(false);
  const ui = mount(() =>
    c(
      Dialog.Root,
      {
        get open() {
          return open();
        },
        onOpenChange: setOpen,
      },
      () => h("div", { testId: "other", tabIndex: 0 }),
      () => c(Dialog.Portal, {}, () => c(Dialog.Popup, { testId: "popup" }, () => c(Dialog.Close, { testId: "close" }))),
    ),
  );
  ui.renderer.focusElement(ui.node("other").id);
  ui.set(() => setOpen(true));
  assert.ok(ui.has("popup"));
  assert.equal(ui.focused(), ui.node("popup").id);
  click(ui, "close");
  assert.equal(ui.has("popup"), false);
  assert.equal(ui.focused(), ui.node("other").id, "the captured owner is restored");
});

test("CMP-003 R6: explicit initialFocus and finalFocus targets use the focus-target contract, including suppression", () => {
  let field = null;
  let outside = null;
  const app = (popup) =>
    c(
      Dialog.Root,
      {},
      () => h("div", { ref: (node) => (outside = node), testId: "outside", tabIndex: 0 }),
      () => c(Dialog.Trigger, { testId: "trigger" }),
      () =>
        c(Dialog.Portal, {}, () =>
          c(
            Dialog.Popup,
            mergeProps({ testId: "popup" }, popup),
            () => h("div", { ref: (node) => (field = node), testId: "field", tabIndex: 0 }),
            () => c(Dialog.Close, { testId: "close" }),
          ),
        ),
    );
  const explicit = mount(() =>
    app({
      get initialFocus() {
        return field;
      },
      get finalFocus() {
        return outside;
      },
    }),
  );
  click(explicit, "trigger");
  assert.equal(explicit.focused(), explicit.node("field").id, "a host element target");
  click(explicit, "close");
  assert.equal(explicit.focused(), explicit.node("outside").id, "final focus beats the trigger");

  const functions = mount(() => app({ initialFocus: () => field, finalFocus: (previous) => (previous === null ? false : outside) }));
  functions.renderer.focusElement(functions.node("trigger").id);
  click(functions, "trigger");
  assert.equal(functions.focused(), functions.node("field").id, "a function target");
  click(functions, "close");
  assert.equal(functions.focused(), functions.node("outside").id, "the final-focus function receives the captured owner");

  const suppressed = mount(() => app({ initialFocus: false, finalFocus: false }));
  suppressed.renderer.focusElement(suppressed.node("outside").id);
  click(suppressed, "trigger");
  assert.equal(suppressed.focused(), suppressed.node("outside").id, "initialFocus false moves nothing");
  const before = calls(suppressed, "focusElement").length;
  click(suppressed, "close");
  assert.equal(calls(suppressed, "focusElement").length, before, "finalFocus false moves nothing");
});

test("CMP-003 R7: a modal popup prevents Tab after the caller's key-down and moves focus within itself, Shift reversing", () => {
  const log = [];
  const ui = mount(() => dialogApp({ root: { defaultOpen: true }, popup: { onKeyDown: (event) => log.push(["caller", event.key]) } }));
  const popup = ui.node("popup").id;
  ui.keyDown("popup", "tab");
  assert.deepEqual(calls(ui, "focusNextWithin"), [["focusNextWithin", popup]]);
  ui.keyDown("popup", "tab", { shift: true });
  assert.deepEqual(calls(ui, "focusPreviousWithin"), [["focusPreviousWithin", popup]]);
  assert.deepEqual(calls(ui, "focusNext"), [], "the page's Tab traversal never runs");
  assert.deepEqual(log, [["caller", "tab"], ["caller", "tab"]], "the caller's key-down runs first");
});

test("CMP-003 R7: a caller that prevents Tab, a modified Tab and a nonmodal popup are not trapped", () => {
  const prevented = mount(() => dialogApp({ root: { defaultOpen: true }, popup: { onKeyDown: (event) => event.preventDefault() } }));
  prevented.keyDown("popup", "tab");
  assert.deepEqual(calls(prevented, "focusNextWithin"), []);
  const modified = mount(() => dialogApp({ root: { defaultOpen: true } }));
  modified.keyDown("popup", "tab", { ctrl: true });
  assert.deepEqual(calls(modified, "focusNextWithin"), []);
  const nonmodal = mount(() => dialogApp({ root: { defaultOpen: true, modal: false } }));
  nonmodal.keyDown("popup", "tab");
  assert.deepEqual(calls(nonmodal, "focusNextWithin"), [], "a nonmodal popup never traps Tab");
});

test("CMP-003 R9: Escape closes the dialog after the caller's handler and focus goes to the trigger; the handler may prevent it", () => {
  const log = [];
  const ui = mount(() => dialogApp({ root: { defaultOpen: true, onOpenChange: (open) => log.push(`open ${open}`) }, popup: { onEscapeKeyDown: () => log.push("handler") } }));
  ui.windowKey("escape");
  assert.deepEqual(log, ["handler", "open false"]);
  assert.equal(ui.has("popup"), false);
  assert.equal(ui.focused(), ui.node("trigger").id);
  const kept = mount(() => dialogApp({ root: { defaultOpen: true }, popup: { onEscapeKeyDown: (event) => event.preventDefault() } }));
  kept.windowKey("escape");
  assert.ok(kept.has("popup"));
});

test("CMP-003 R9: Escape closes a select opened from inside the dialog first, then the dialog", () => {
  const ui = mount(() =>
    c(Dialog.Root, { defaultOpen: true }, () =>
      c(Dialog.Portal, {}, () =>
        c(Dialog.Popup, { testId: "popup" }, () =>
          c(
            Select.Root,
            { defaultOpen: true },
            () => c(Select.Trigger, { testId: "select-trigger" }),
            () => c(Select.Content, { testId: "select-content" }, () => c(Select.Item, { value: "a" }, "A")),
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
  const ui = mount(() =>
    c(DismissableLayer, { testId: "page-layer", onEscapeKeyDown: () => log.push("below") }, () =>
      dialogApp({ root: { defaultOpen: true, disableEscapeDismissal: true }, popup: { onEscapeKeyDown: () => log.push("popup handler") } }),
    ),
  );
  ui.windowKey("escape");
  assert.ok(ui.has("popup"), "the dialog stays open");
  assert.deepEqual(log, ["popup handler"], "and the layer below never sees the Escape");
});

test("CMP-003 R10: a nonmodal outside press closes the dialog unless pointer dismissal is disabled; a modal dialog ignores it", async () => {
  const nonmodal = mount(() => dialogApp({ root: { defaultOpen: true, modal: false } }));
  nonmodal.emit("popup", { type: "outsideDown", x: 1, y: 1 });
  await microtasks();
  assert.equal(nonmodal.has("popup"), false);
  const kept = mount(() => dialogApp({ root: { defaultOpen: true, modal: false, disablePointerDismissal: true } }));
  kept.emit("popup", { type: "outsideDown", x: 1, y: 1 });
  await microtasks();
  assert.ok(kept.has("popup"));
  const modal = mount(() => dialogApp({ root: { defaultOpen: true } }));
  modal.emit("popup", { type: "outsideDown", x: 1, y: 1 });
  await microtasks();
  assert.ok(modal.has("popup"), "a modal dialog is dismissed through its backdrop only");
});

test("CMP-003 R9: a press on the trigger of an open nonmodal dialog does not dismiss it first", async () => {
  const log = [];
  const ui = mount(() => dialogApp({ root: { defaultOpen: true, modal: false, onOpenChange: (open) => log.push(open) } }));
  ui.emit("popup", { type: "outsideDown", x: 1, y: 1 });
  ui.emit("trigger", { type: "pointerDown", button: 0 });
  await microtasks();
  click(ui, "trigger");
  assert.ok(ui.has("popup"), "the trigger's press flag keeps the outside press from closing it");
  assert.deepEqual(log, []);
  await settle();
});
