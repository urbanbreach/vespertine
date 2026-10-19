// The Solid universal renderer over the retained host tree (APP-009 R6).
//
// Host nodes are plain descriptions until they are inserted under a placed parent; property and
// text updates after that are incremental (only the changed property is compared and sent).

import { createRenderer } from "solid-js/universal";
import {
  NATIVE_ELEMENT_TYPES,
  createHostNode,
  createTextNode,
  insertNode,
  removeNode,
  setNodeProp,
  setNodeText,
} from "@vespertine/host";

const SUPPORTED = new Set(NATIVE_ELEMENT_TYPES);

export const universal = createRenderer({
  createElement(type) {
    if (!SUPPORTED.has(type)) {
      throw new Error(`Unsupported intrinsic element "${type}"; supported types: ${NATIVE_ELEMENT_TYPES.join(", ")}`);
    }
    return createHostNode(type, {});
  },
  createTextNode: (value) => createTextNode(value),
  replaceText: (node, value) => setNodeText(node, value),
  isTextNode: (node) => node.rawText,
  setProperty: (node, name, value) => setNodeProp(node, name, value),
  insertNode: (parent, node, anchor) => insertNode(parent, node, anchor ?? null),
  removeNode: (parent, node) => removeNode(parent, node),
  getParentNode: (node) => node.parent ?? undefined,
  getFirstChild: (node) => node.children[0],
  getNextSibling(node) {
    const siblings = node.parent?.children;
    return siblings ? siblings[siblings.indexOf(node) + 1] : undefined;
  },
});

export const {
  effect,
  memo,
  createComponent,
  createElement,
  createTextNode: createText,
  insertNode: insertIntoNode,
  insert,
  spread,
  setProp,
  mergeProps,
  use,
} = universal;
