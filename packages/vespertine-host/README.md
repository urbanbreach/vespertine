# @vespertine/host

JavaScript host runtime for Vespertine (APP-005, APP-007, APP-008): runs under Bun and Node.

```js
import { createNativeRenderer, createMutationQueue, startFrameLoop } from "@vespertine/host";

const renderer = createNativeRenderer({ onError: console.error });
renderer.init({ title: "Hello", width: 640, height: 360, appId: "org.example.hello" });
const queue = createMutationQueue(renderer);
queue.createElement(0, "div");
queue.setStyle(0, { backgroundColor: "#336699" });
queue.setRoot(0);
queue.flushMutations();
startFrameLoop(renderer, { onTerminated: () => process.exit(0) });
```

## Native addon

The renderer is the Node-API addon `vespertine_js_node` (core: `vespertine_js`). Build it with
`node scripts/build-native.mjs [--release]`; it is installed as `native/vespertine_js.node`
(or point `VESPERTINE_NATIVE_ADDON` at the file). Importing the helpers never loads the addon or
opens a window; `createNativeRenderer` does.

The window runs on its own UI thread. Commands that need an answer (`getWindowSize`,
`getWindowInsets`) wait at most 2 s and then throw `Timed out after 2 seconds waiting for <operation>`;
after the last window closed `tick()` returns `false` and later commands throw
`The GPUI UI thread is not running`. `applyBatch` on a batch that changes what a frame draws also
queues a repaint; when that request cannot be queued (not initialized / loop stopped) the batch is
still applied and the call throws with the cause. Repeat-sending the same operations after such an
error (what `createMutationQueue` does with retained operations) re-creates the elements, so start
flushing after `init`.

Extras beyond the specified renderer API: `closeWindow()`, `requestRepaint()`, `getRenderCount()`.

## Helpers

| Area | Exports |
| --- | --- |
| Mutation queue (APP-007 R2) | `createMutationQueue`, `mutationBatchAttempts` |
| Structural comparison (R3) | `sameMutationValue`, `serializeCustomProp` |
| Event routing, layers (R1, R4, R5) | `createRendererState`, `pushDismissLayer`, `createHandlerMap`, `registerEventHandler`, `unregisterEventHandler`, `unregisterEventHandlers` |
| Focus targets (R6) | `resolveFocusTarget` |
| Geometry and selection (R7) | `readWindowSize`, `readWindowInsets`, `observeWindowSize`, `observeWindowInsets`, `observeSelectedText` |
| Renderer and frame loop (R8) | `createNativeRenderer`, `startFrameLoop` |
| Idle GC (APP-008) | `collectGarbageWhenIdle` |

Event objects delivered by the native side: `{ type, id, ... }`; key events carry `key` (lowercase
token), `ctrl`/`alt`/`cmd`/`shift` and a `keystroke` identity; window-level events use the types
`windowKeyDown`, `windowKeyUp` and `windowSelectionChange` with `id` equal to the generation of the
mounted root (announced to the renderer through `setWindowGeneration`).

## Tests and tools

`npm test` (or `bun test`) runs the helper tests, plus headless checks of the addon when it is built;
`cargo test -p vespertine_js_node` builds the addon and runs both runtimes.
`scripts/record-fixture.mjs` records a mutation queue from a real `react-reconciler` run (needs
`npm install`); `scripts/measure-serialization.mjs` times the JavaScript side of the DEV-008 fixture.
