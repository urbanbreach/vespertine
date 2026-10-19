// The runtime error display (DEV-010 R3, R4, R6): failure normalization, stack rows and the
// element tree of the error view. It is an ordinary element tree rendered through the same
// renderer as the application, so the test host can inspect it.

import React from "react";

export const HEADING = "Uncaught runtime errors:";
export const RELOAD_LABEL = "Reload";
export const UNKNOWN_ERROR = "Unknown error";

/** Colors used when the active theme provides no tokens (sRGB, `#RRGGBBAA`). */
export const FALLBACK_COLORS = Object.freeze({
  surface: "#000000E6",
  danger: "#E83B46",
  dangerHover: "#C92A34",
  panel: "#CE11261A",
  stackText: "#FCCFCF",
  onDanger: "#FFFFFF",
});

export const MONOSPACE = "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace";

const FRAME_INDENT = "    ";

function stringify(value) {
  try {
    return String(value);
  } catch {
    return UNKNOWN_ERROR;
  }
}

/**
 * Normalizes any failure value into `{ message, stack }` (DEV-010 R3). A non-empty component
 * stack is appended to the stack text unless it is already contained; the message is prepended
 * when the stack does not contain it.
 */
export function normalizeFailure(value, componentStack) {
  let message;
  let stack;
  if (value instanceof Error) {
    message = value.message || value.name;
    stack = value.stack || `${value.name}: ${value.message}`;
  } else if (typeof value === "string") {
    message = value;
    stack = value;
  } else {
    message = stringify(value);
    stack = message;
  }
  if (message === "") message = UNKNOWN_ERROR;
  if (typeof componentStack === "string" && componentStack !== "" && !stack.includes(componentStack)) {
    stack = stack === "" || componentStack.startsWith("\n") ? `${stack}${componentStack}` : `${stack}\n${componentStack}`;
  }
  if (!stack.includes(message)) stack = `${message}\n${stack}`;
  return { message, stack };
}

/** The rows of the stack panel for a normalized failure (DEV-010 R4). */
export function stackRows(message, stack) {
  const candidates = stack === "" ? [message] : stack.split("\n");
  const rows = [];
  for (const row of candidates) {
    const trimmed = row.trim();
    if (trimmed === "" || trimmed === message || trimmed === `Error: ${message}`) continue;
    rows.push(/^at\s/.test(row) ? `${FRAME_INDENT}${row}` : row);
  }
  return rows.length === 0 ? [message] : rows;
}

const h = React.createElement;

/**
 * The error view element.
 *
 * @param {{ message: string, stack: string, onReload: () => void, colors?: object }} props
 */
export function ErrorView({ message, stack, onReload, colors }) {
  const palette = { ...FALLBACK_COLORS, ...colors };
  const rows = stackRows(message, stack);
  return h(
    "div",
    {
      testId: "error-view",
      style: {
        display: "flex",
        flexDirection: "column",
        gap: 20,
        width: "100%",
        height: "100%",
        paddingTop: 32,
        paddingLeft: 32,
        paddingRight: 32,
        paddingBottom: 40,
        backgroundColor: palette.surface,
      },
    },
    h(
      "text",
      {
        testId: "error-heading",
        style: { fontSize: 32, fontWeight: 700, lineHeight: 44, flexShrink: 0, color: palette.danger },
      },
      HEADING,
    ),
    h(
      "div",
      {
        testId: "error-panel",
        style: {
          display: "flex",
          flexDirection: "column",
          gap: 4,
          paddingTop: 16,
          paddingLeft: 16,
          paddingRight: 16,
          paddingBottom: 24,
          borderRadius: 4,
          backgroundColor: palette.panel,
          overflowY: "scroll",
          flexShrink: 1,
          flexGrow: 0,
          minHeight: 0,
        },
      },
      h(
        "text",
        {
          testId: "error-message",
          style: { fontSize: 20, fontWeight: 700, lineHeight: 28, marginBottom: 12, color: palette.danger, flexShrink: 0 },
        },
        message,
      ),
      rows.map((row, index) =>
        h(
          "text",
          {
            key: index,
            testId: `error-row-${index}`,
            style: {
              fontSize: 13,
              lineHeight: 20,
              whiteSpace: "nowrap",
              fontFamily: MONOSPACE,
              color: palette.stackText,
              flexShrink: 0,
            },
          },
          row,
        ),
      ),
    ),
    h(
      "div",
      {
        testId: "error-reload",
        role: "button",
        "aria-label": RELOAD_LABEL,
        tabIndex: 0,
        onClick: onReload,
        style: {
          alignSelf: "flex-start",
          flexShrink: 0,
          paddingTop: 10,
          paddingBottom: 10,
          paddingLeft: 16,
          paddingRight: 16,
          borderRadius: 4,
          backgroundColor: palette.danger,
          cursor: "pointer",
          hover: { backgroundColor: palette.dangerHover },
        },
      },
      h("text", { style: { fontSize: 14, fontWeight: 700, lineHeight: 20, color: palette.onDanger } }, RELOAD_LABEL),
    ),
  );
}
