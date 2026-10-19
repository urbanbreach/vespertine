// Runtime JSX entry points (`jsxImportSource: "@vespertine/solid"`): `jsx`, `jsxs` and
// `Fragment` build Solid universal trees without the Babel transform.

import { createComponent, createElement, insert, spread } from "./renderer.js";

export const Fragment = (props) => props.children;

export function jsx(type, props) {
  if (typeof type === "function") return createComponent(type, props);
  const element = createElement(type);
  const { children, ...rest } = props;
  spread(element, rest, children !== undefined);
  if (children !== undefined) insert(element, children);
  return element;
}

export const jsxs = jsx;
export const jsxDEV = jsx;
