import type { Accessor, JSX as SolidJSX } from "solid-js";

export * from "solid-js";
export * from "solid-js/store";

export interface NativeEvent {
  type: string;
  id: number;
  [key: string]: unknown;
}

export interface RootOptions {
  onEvent?(event: NativeEvent): void;
  onWindowKeyDown?(event: NativeEvent): void;
  onWindowKeyUp?(event: NativeEvent): void;
  onSelectionChange?(event: NativeEvent): void;
  tabNavigation?: boolean;
  keyboardFocusDim?: boolean;
}

export interface RenderOptions extends RootOptions {
  renderer?: object;
  native?: object;
  window?: Record<string, unknown>;
  debugOverlay?: "hidden" | "minimal" | "full";
  frameMs?: number;
}

export interface Root {
  readonly context: object;
  readonly renderer: object;
  render(code: () => unknown): void;
  flush(): void;
  flushSync<T>(fn: () => T): T;
  dispatch(event: NativeEvent): boolean;
  unmount(): void;
}

export function createRoot(renderer: object, options?: RootOptions): Root;
export function createRenderer(onEvent?: (event: NativeEvent) => void, options?: { onError?(error: unknown): void; native?: object }): any;
export function render(code: () => unknown, options?: RenderOptions): Root;
export function resetRender(): void;
export function useRenderer(): any;
export function useRendererContext(): { renderer: any };
export function useOptionalRendererContext(): { renderer: any } | undefined;

export function createWindowSize(options?: { intervalMs?: number | false }): () => { width: number; height: number };
export function createWindowInsets(options?: { intervalMs?: number | false }): () => object;
export function createSelectedText(): () => string | null;
export function createTextSearch(options: () => {
  query?: string;
  caseSensitive?: boolean;
  wholeWord?: boolean;
  matchIndexOffset?: number;
}): {
  readonly props: { highlight: object; onHighlight(event: { matchCount: number }): void };
  readonly total: number;
  readonly active: number;
  next(): void;
  previous(): void;
  goTo(index: number): void;
};
export function findRanges(texts: string | string[], query: string, options?: { caseSensitive?: boolean; wholeWord?: boolean }): [number, number][];

// ---- Headless components (CMP-001, CMP-002, CMP-004, CMP-005) ----------------------------------
//
// Solid differences from React (see the header comments of src/components): asChild needs one
// intrinsic host element child and replaces (never merges) style; the caller style of a floating
// layer applies to the inner container only; tooltip provider settings and root hoverability are
// captured at construction and timers are overwritten, not replaced; the select binds no Ctrl-N or
// Ctrl-P, keeps navigation while disabled and initializes its highlight on an open request.

/** A host element of the universal renderer (a retained host tree node). */
export type HostElement = { readonly type: string; readonly id: number | null; readonly mounted: boolean; focus(): void; [member: string]: any };

export type StyleOrFunction<State> = Record<string, unknown> | ((state: State) => Record<string, unknown> | undefined);

/** Properties of a part: host properties plus `asChild`; `ref` is a host-element callback. */
export interface SlotProps {
  asChild?: boolean;
  children?: SolidJSX.Element;
  ref?: (element: HostElement) => void;
  style?: Record<string, unknown>;
  testId?: string;
  tabIndex?: number;
  role?: string;
  [property: string]: unknown;
}

export interface ButtonProps extends Omit<SlotProps, "style"> {
  disabled?: boolean;
  focusableWhenDisabled?: boolean;
  style?: StyleOrFunction<{ disabled: boolean }>;
  onClick?(event: NativeEvent): void;
  onKeyDown?(event: NativeEvent): void;
  onKeyUp?(event: NativeEvent): void;
}
/** A style-free button: role `button`, tab index 0 (-1 while disabled unless focusable), click, Enter and Space activation. */
export function Button(props: ButtonProps): SolidJSX.Element;

export interface ButtonLatch {
  enter: boolean;
}
/** The per-control Enter latch of `buttonProps`; keep it as long as the control. */
export function createButtonLatch(): ButtonLatch;
/** The behavior of a button for any control: `role`, `tabIndex`, `onClick`, `onKeyDown`, `onKeyUp`. */
export function buttonProps(options?: {
  disabled?: boolean;
  focusableWhenDisabled?: boolean;
  tabIndex?: number;
  onClick?(event: NativeEvent): void;
  onKeyDown?(event: NativeEvent): void;
  onKeyUp?(event: NativeEvent): void;
  latch?: ButtonLatch;
}): {
  role: "button";
  tabIndex: number;
  onClick(event: NativeEvent): void;
  onKeyDown(event: NativeEvent): void;
  onKeyUp(event: NativeEvent): void;
};

export function mergeStyles(...styles: unknown[]): Record<string, unknown>;
export function resolveStyle<State>(style: StyleOrFunction<State> | undefined, state: State): Record<string, unknown> | undefined;
export function composeHandlers<A extends unknown[]>(first?: (...args: A) => void, second?: (...args: A) => void): ((...args: A) => void) | undefined;

/**
 * Renders a part: a new `type` element (default `div`) holding the children, or with `asChild` the
 * part's properties spread onto the single intrinsic host element child (which is returned).
 * `children` is a function returning the children; `props` may carry getters.
 */
export function renderSlot(part: {
  asChild?: boolean;
  children?: () => unknown;
  type?: string;
  props?: Record<string, unknown>;
  defaults?: { tabIndex?: number };
}): HostElement;

/**
 * `[value, request]` with `value` an accessor. `options.value` is an accessor of the controlled
 * value (undefined means uncontrolled); a request calls `onChange` only for a different value and
 * returns whether it did.
 */
export function createControllableState<T>(options?: {
  value?: Accessor<T | undefined>;
  defaultValue?: T;
  onChange?(value: T): void;
}): [Accessor<T>, (next: T) => boolean];

export type FocusTarget = boolean | HostElement | { current: HostElement | null } | (() => HostElement | boolean | null | undefined);

export interface FloatingLayerProps extends SlotProps {
  side?: "top" | "right" | "bottom" | "left";
  align?: "start" | "center" | "end";
  sideOffset?: number;
  alignOffset?: number;
  collisionPadding?: number;
}
/** Content positioned against the nearest enclosing floating root; the style applies to the inner container only (Solid). */
export function FloatingLayer(props: FloatingLayerProps): SolidJSX.Element;
/** The default fill of floating content. */
export const FLOATING_FILL: "#1A1A1A";
/** The properties of the native `anchored` host for the floating options. */
export function anchoredProps(options?: {
  side?: "top" | "right" | "bottom" | "left";
  align?: "start" | "center" | "end";
  sideOffset?: number;
  alignOffset?: number;
  collisionPadding?: number;
  occlude?: boolean;
}): Record<string, unknown>;

export interface DismissableLayerProps extends SlotProps {
  open?: boolean;
  onEscapeKeyDown?(event: { key: "escape"; preventDefault(): void; readonly defaultPrevented: boolean }): void;
  onDismiss?(reason: "escape" | "outside"): void;
  onOutsideDown?(event: NativeEvent): void;
  initialFocus?: FocusTarget;
  finalFocus?: boolean | HostElement | { current: HostElement | null } | ((previous: number | null) => HostElement | boolean | null | undefined);
  defaultFocus?(node: HostElement | null): number | null;
  trigger?(): HostElement | null;
  pressFlag?: { consumes(): boolean; defer(fn: () => void): void };
}
export function DismissableLayer(props: DismissableLayerProps): SolidJSX.Element;

/** Native exit target of popup content (CMP-002 R17): the content stays mounted, inert, until the exit completes. */
export interface ExitProps {
  initial?: false | Record<string, number>;
  animate?: Record<string, number>;
  exit?: Record<string, number>;
  transition?: { duration?: number; delay?: number; ease?: string | [number, number, number, number] };
}

/**
 * The exit lifecycle of popup content: `open` is an accessor, `options` the exit properties (or an
 * accessor of them). `mounted` stays true while an exit animation runs.
 */
export function createPresence(
  open: Accessor<boolean>,
  options?: ExitProps | Accessor<ExitProps>,
): {
  mounted: Accessor<boolean>;
  closing: Accessor<boolean>;
  motion: Accessor<(ExitProps & { present: boolean; generation: number }) | undefined>;
  onMotionComplete(event: { exit?: boolean; generation?: number }): void;
};

export namespace Tooltip {
  function Provider(props: { delayDuration?: number; skipDelayDuration?: number; disableHoverableContent?: boolean; children?: SolidJSX.Element }): SolidJSX.Element;
  function Root(
    props: Omit<SlotProps, "asChild"> & { open?: boolean; defaultOpen?: boolean; onOpenChange?(open: boolean): void; delayDuration?: number; disableHoverableContent?: boolean },
  ): SolidJSX.Element;
  function Trigger(props: SlotProps): SolidJSX.Element;
  function Content(
    props: FloatingLayerProps & ExitProps & { onEscapeKeyDown?: DismissableLayerProps["onEscapeKeyDown"] },
  ): SolidJSX.Element;
}

export namespace Select {
  type ItemMetadata = Record<string, string> | { value: string; label: string }[];
  type ItemState = { readonly selected: boolean; readonly highlighted: boolean; readonly disabled: boolean };
  function Root(
    props: Omit<SlotProps, "asChild"> & {
      value?: string;
      defaultValue?: string;
      onValueChange?(value: string): void;
      open?: boolean;
      defaultOpen?: boolean;
      onOpenChange?(open: boolean): void;
      disabled?: boolean;
      items?: ItemMetadata;
    },
  ): SolidJSX.Element;
  function Trigger(
    props: Omit<SlotProps, "style"> & { disabled?: boolean; style?: StyleOrFunction<{ open: boolean; disabled: boolean; placeholder: boolean }> },
  ): SolidJSX.Element;
  function Value(props: Omit<SlotProps, "asChild"> & { placeholder?: SolidJSX.Element }): SolidJSX.Element;
  function Content(
    props: FloatingLayerProps & ExitProps & { onEscapeKeyDown?: DismissableLayerProps["onEscapeKeyDown"] },
  ): SolidJSX.Element;
  function Item(
    props: Omit<SlotProps, "style" | "children"> & {
      value: string;
      disabled?: boolean;
      textValue?: string;
      style?: StyleOrFunction<ItemState>;
      children?: SolidJSX.Element | ((state: ItemState) => SolidJSX.Element);
    },
  ): SolidJSX.Element;
  function Group(props: SlotProps): SolidJSX.Element;
  function Label(props: SlotProps): SolidJSX.Element;
  function Separator(props: SlotProps): SolidJSX.Element;
  function ScrollUpButton(props: SlotProps): SolidJSX.Element;
  function ScrollDownButton(props: SlotProps): SolidJSX.Element;
  /** The text of resolved children (strings, numbers and host nodes). */
  function textOf(node: unknown): string;
}

declare global {
  namespace JSX {
    type HostProps = { style?: Record<string, unknown>; ref?: unknown; children?: unknown; testId?: string; [property: string]: unknown };
    interface IntrinsicElements {
      div: HostProps;
      text: HostProps;
      img: HostProps;
      svg: HostProps;
      canvas: HostProps;
      input: HostProps;
      textarea: HostProps;
      anchored: HostProps;
      code: HostProps;
      diff: HostProps;
      markdown: HostProps;
      "virtual-list": HostProps;
    }
  }
}
