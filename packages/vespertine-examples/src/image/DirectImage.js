// The direct-image example (APP-011 R7): pixels uploaded to an image element through its public
// reference, plus reference-driven scrolling.

import { React, h } from "../h.js";
import { palette } from "../theme.js";
import { HEIGHT, PHASE_STEP, WIDTH, renderTrace } from "./trace.js";

export const ELEMENT_WIDTH = 720;
export const ELEMENT_HEIGHT = 96;
export const LIST_HEIGHT = 120;
export const ROW_HEIGHT = 48;
export const ROWS = 5;

const colors = palette();

export function DirectImage({ width, height }) {
  const image = React.useRef(null);
  const lastRow = React.useRef(null);
  const [phase, setPhase] = React.useState(0);

  // Updating the phase replaces the pixels through the image reference; the element tree and the
  // rest of the UI are not recreated.
  React.useEffect(() => {
    image.current?.setImagePixels(WIDTH, HEIGHT, renderTrace(phase), "bgra");
  }, [phase]);

  return h(
    "div",
    { testId: "direct-image-root", style: { width, height, flexDirection: "column", gap: 12, padding: 12, backgroundColor: colors.surface } },
    h(
      "div",
      { testId: "image-surface", onPointerDown: () => setPhase((value) => value + PHASE_STEP), style: { width: ELEMENT_WIDTH, height: ELEMENT_HEIGHT } },
      h("img", { testId: "direct-image", ref: image, objectFit: "fill", style: { width: ELEMENT_WIDTH, height: ELEMENT_HEIGHT, borderRadius: 10 } }),
    ),
    h(
      "div",
      {
        testId: "scroll-into-view",
        role: "button",
        "aria-label": "Scroll into view",
        onClick: () => lastRow.current?.scrollIntoView(),
        style: { height: 30, width: 160, alignItems: "center", justifyContent: "center", borderRadius: 4, backgroundColor: colors.accent },
      },
      h("text", { style: { fontSize: 14, color: colors.text } }, "Scroll into view"),
    ),
    h(
      "div",
      { testId: "scrolling-list", style: { width: 320, height: LIST_HEIGHT, flexDirection: "column", overflowY: "scroll", backgroundColor: colors.raised } },
      Array.from({ length: ROWS }, (_, index) =>
        h(
          "div",
          { key: index, testId: `scroll-row-${index}`, ref: index === ROWS - 1 ? lastRow : undefined, style: { height: ROW_HEIGHT, flexShrink: 0, paddingLeft: 8, justifyContent: "center", borderColor: colors.border, borderWidth: 1 } },
          h("text", { style: { fontSize: 13, color: colors.text } }, `Row ${index + 1}`),
        ),
      ),
    ),
  );
}
