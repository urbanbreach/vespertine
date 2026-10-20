// APP-007 R8 / DEV-007: an application whose window can be automated live. When it is started
// with a piped standard input, `createNativeRenderer` serves the automation protocol over
// standard input and output after `init`; a driver (for example `Automation::launch` of
// `vespertine_test_host`) then reads the tree and painted bounds and clicks, types, focuses and
// scrolls the real window.
//
//   node examples/automation_app.mjs
//
// Standard output is the protocol stream, so this program logs to standard error only. The
// click counter, the typed keys, the focus label and the window size are all retained tree
// text, so a driver observes the effect of its input the way a user would.
import {
  createHandlerMap,
  createMutationQueue,
  createNativeRenderer,
  createRendererState,
  registerEventHandler,
  startFrameLoop,
} from "../src/index.js";

const renderer = createNativeRenderer({ onError: (error) => console.error(error) });
renderer.init({ title: "Automation", width: 400, height: 300, appId: "org.vespertine.automation" });

// A second call is an error: standard input and output serve one automation channel.
let secondServe = "no error";
try {
  renderer.serveAutomation();
} catch (error) {
  secondServe = String(error.message ?? error);
}

const TITLE = 2;
const NAME = 3;
const SAVE = 4;
const LIST = 5;
const CLICKS = 6;
const KEYS = 7;
const FOCUS = 8;
const WINDOW = 9;
const SERVE = 10;
const FIRST_ITEM = 20;

const queue = createMutationQueue(renderer);
const handlers = createHandlerMap();
createRendererState(renderer).attach({ handlers });

const text = (id, value, testId) => {
  queue.createElement(id, "text");
  queue.setStyle(id, { height: 20 });
  queue.setText(id, value);
  queue.setCustomProp(id, "testId", testId);
};

queue.createElement(1, "div");
queue.setStyle(1, {
  width: 400,
  height: 300,
  flexDirection: "column",
  backgroundColor: "#ffffff",
  fontSize: 14,
  color: "#111111",
});
text(TITLE, "Automation", "title");

queue.createElement(NAME, "input");
queue.setStyle(NAME, { width: 200, height: 28, backgroundColor: "#eeeeee" });
queue.setCustomProp(NAME, "testId", "name");

queue.createElement(SAVE, "div");
queue.setStyle(SAVE, { width: 80, height: 30, backgroundColor: "#3355ff", color: "#ffffff" });
queue.setText(SAVE, "Save");
queue.setCustomProp(SAVE, "testId", "save");

queue.createElement(LIST, "div");
queue.setStyle(LIST, {
  width: 200,
  height: 90,
  flexDirection: "column",
  overflow: "scroll",
  backgroundColor: "#f4f4f4",
});
queue.setCustomProp(LIST, "testId", "list");
for (let item = 0; item < 10; item++) {
  const id = FIRST_ITEM + item;
  queue.createElement(id, "text");
  queue.setStyle(id, { height: 30, width: "100%" });
  queue.setText(id, `Item ${item}`);
  queue.appendChild(LIST, id);
}

text(CLICKS, "clicks: 0", "clicks");
text(KEYS, "keys:", "keys");
text(FOCUS, "focus: none", "focus");
const windowLabel = () => {
  const size = renderer.getWindowSize();
  return `window: ${size.width}x${size.height}`;
};
let shownWindow = windowLabel();
text(WINDOW, shownWindow, "window");
text(SERVE, `second serve: ${secondServe}`, "serve");

for (const child of [TITLE, NAME, SAVE, LIST, CLICKS, KEYS, FOCUS, WINDOW, SERVE]) {
  queue.appendChild(1, child);
}
queue.setRoot(1);

let clicks = 0;
let keys = "";
const update = (id, value) => {
  queue.setText(id, value);
  queue.flushMutations();
};

queue.setEventListener(SAVE, "click", true);
registerEventHandler(handlers, SAVE, "click", () => update(CLICKS, `clicks: ${++clicks}`));
queue.setEventListener(NAME, "keyDown", true);
registerEventHandler(handlers, NAME, "keyDown", (event) => update(KEYS, `keys:${(keys += ` ${event.key}`)}`));
queue.setEventListener(NAME, "focus", true);
registerEventHandler(handlers, NAME, "focus", () => update(FOCUS, "focus: name"));
queue.setEventListener(NAME, "blur", true);
registerEventHandler(handlers, NAME, "blur", () => update(FOCUS, "focus: none"));
queue.flushMutations();
console.error("automation app ready");

let terminated = false;
const loop = startFrameLoop(renderer, { onTerminated: () => (terminated = true) });
// The window size is retained text too: a compositor may lay the window out again after it
// opened (a tiling compositor does), so the label follows the window.
while (!terminated) {
  await new Promise((resolve) => setTimeout(resolve, 20));
  if (terminated) break;
  const label = windowLabel();
  if (label !== shownWindow) {
    shownWindow = label;
    update(WINDOW, label);
  }
}
loop.stop();
