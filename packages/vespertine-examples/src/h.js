import React from "react";

/**
 * `React.createElement` under a short name: the examples are written without JSX. Host `div`
 * elements use the layout engine's default (block) display, so the examples declare flex
 * containers explicitly; this helper adds `display: "flex"` to every `div` style that does not
 * choose a display itself.
 */
export function h(type, props, ...children) {
  if (type === "div") {
    const style = props?.style;
    if (style?.display === undefined) {
      props = { ...props, style: { display: "flex", ...style } };
    }
  }
  return React.createElement(type, props, ...children);
}
export { React };
