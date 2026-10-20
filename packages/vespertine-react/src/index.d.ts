import type { ReactNode, Ref } from "react";

/** A mounted host element reference. */
export interface HostRef {
  readonly id: number;
  focus(): unknown;
  blur(): unknown;
  focusNext(): unknown;
  focusPrevious(): unknown;
  focusNextWithin(): unknown;
  focusPreviousWithin(): unknown;
  scrollIntoView(options?: object): unknown;
  scrollTo(x: number, y: number): unknown;
  scrollToItem(index: number, offset?: number): unknown;
  getScrollOffset(): { x: number; y: number } | null;
  getListScrollTop(): unknown;
  setImage(bytes: Uint8Array): unknown;
  setImagePixels(width: number, height: number, bytes: Uint8Array, format?: "bgra" | "rgba"): unknown;
}

export interface NativeEvent {
  type: string;
  id: number;
  x?: number;
  y?: number;
  [key: string]: unknown;
}

export interface RootOptions {
  onEvent?(event: NativeEvent): void;
  onWindowKeyDown?(event: NativeEvent): void;
  onWindowKeyUp?(event: NativeEvent): void;
  onSelectionChange?(event: NativeEvent): void;
  tabNavigation?: boolean;
  keyboardFocusDim?: boolean;
  onUncaughtError?(error: unknown, info: { componentStack?: string } | string | undefined): void;
}

export interface RenderOptions extends RootOptions {
  renderer?: object;
  native?: object;
  window?: Record<string, unknown>;
  debugOverlay?: "hidden" | "minimal" | "full";
  frameMs?: number;
  errorColors?: Partial<Record<"surface" | "danger" | "dangerHover" | "panel" | "stackText" | "onDanger", string>>;
}

export interface Root {
  readonly context: object;
  readonly renderer: object;
  render(node: ReactNode): void;
  renderSync(node: ReactNode): void;
  unmount(): void;
}

export function createRoot(renderer: object, options?: RootOptions): Root;
export function createRenderer(onEvent?: (event: NativeEvent) => void, options?: { onError?(error: unknown): void; native?: object }): any;
export function render(node: ReactNode, options?: RenderOptions): Root;
export function resetRender(): void;
export function flushSync<T>(fn: () => T): T;
export function createPortal(children: ReactNode, target: HostRef, key?: string | number | null): ReactNode;
export function useRenderer(): any;
export function useRendererContext(): { renderer: any };
export function useOptionalRendererContext(): { renderer: any } | null;

export interface Size {
  width: number;
  height: number;
}
export interface Edges {
  top: number;
  right: number;
  bottom: number;
  left: number;
}
export interface Insets {
  safeArea: Edges;
  ime: Edges;
  effective: Edges;
  keyboardTop: number;
  keyboardVisible: boolean;
  visibleHeight: number;
}
export function useWindowSize(options?: { intervalMs?: number | false }): Size;
export function useWindowInsets(options?: { intervalMs?: number | false }): Insets;

export interface TextSearchOptions {
  query?: string;
  caseSensitive?: boolean;
  wholeWord?: boolean;
  matchIndexOffset?: number;
  color?: string;
  activeColor?: string;
  radius?: number;
}
export interface TextSearchResult {
  props: { highlight: object; onHighlight(event: { matchCount: number }): void };
  total: number;
  active: number;
  next(): void;
  previous(): void;
  goTo(index: number): void;
}
export function useTextSearch(options?: TextSearchOptions): TextSearchResult;
export function findRanges(
  texts: string | string[],
  query: string,
  options?: { caseSensitive?: boolean; wholeWord?: boolean },
): [number, number][];

/** Host properties shared by every native element. */
export interface HostProps {
  style?: Record<string, unknown>;
  ref?: Ref<HostRef>;
  key?: string | number;
  children?: ReactNode;
  testId?: string;
  autoFocus?: boolean;
  tabIndex?: number;
  motion?: object;
  highlight?: object;
  role?: string;
  [property: string]: unknown;
}

declare module "react" {
  namespace JSX {
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

// ---- Headless components (CMP-001, CMP-002, CMP-003, CMP-004, CMP-005, CMP-006) ----------------------------------

export type StyleOrFunction<State> = Record<string, unknown> | ((state: State) => Record<string, unknown> | undefined);

/** Properties of a part that can merge onto its single element child. */
export interface SlotProps extends HostProps {
  asChild?: boolean;
}

export interface ButtonProps extends SlotProps {
  disabled?: boolean;
  focusableWhenDisabled?: boolean;
  style?: StyleOrFunction<{ disabled: boolean }>;
  onClick?(event: NativeEvent): void;
}
/** A style-free button: role `button`, tab index 0 (-1 while disabled unless focusable), click, Enter and Space activation. */
export const Button: React.ForwardRefExoticComponent<ButtonProps & React.RefAttributes<HostRef>>;

export interface ButtonLatch {
  enter: boolean;
}
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
export function useButtonLatch(): ButtonLatch;

export function mergeStyles(...styles: unknown[]): Record<string, unknown>;
export function resolveStyle<State>(style: StyleOrFunction<State> | undefined, state: State): Record<string, unknown> | undefined;
export function composeHandlers<A extends unknown[]>(first?: (...args: A) => void, second?: (...args: A) => void): ((...args: A) => void) | undefined;
export function renderSlot(part: {
  asChild?: boolean;
  children?: ReactNode;
  type?: string;
  props: Record<string, unknown>;
  defaults?: { tabIndex?: number };
  ref?: Ref<HostRef>;
}): ReactNode;

/** `[value, request]`: an undefined `value` is uncontrolled; a request calls `onChange` only for a different value. */
export function useControllableState<T>(options: { value?: T; defaultValue?: T; onChange?(value: T): void }): [T, (next: T) => boolean];

export type FocusTarget = boolean | HostRef | { current: HostRef | null } | (() => HostRef | boolean | null | undefined);

export interface FloatingLayerProps extends SlotProps {
  side?: "top" | "right" | "bottom" | "left";
  align?: "start" | "center" | "end";
  sideOffset?: number;
  alignOffset?: number;
  collisionPadding?: number;
}
/** Content positioned against the nearest enclosing floating root (React routes opacity and radii to the outer surface). */
export const FloatingLayer: React.ForwardRefExoticComponent<FloatingLayerProps & React.RefAttributes<HostRef>>;

export interface DismissableLayerProps extends HostProps {
  open?: boolean;
  onEscapeKeyDown?(event: { key: "escape"; preventDefault(): void; readonly defaultPrevented: boolean }): void;
  onDismiss?(reason: "escape" | "outside", info?: { top: boolean }): void;
  initialFocus?: FocusTarget;
  finalFocus?: boolean | HostRef | { current: HostRef | null } | ((previous: number | null) => HostRef | boolean | null | undefined);
  defaultFocus?(node: HostRef | null): number | null;
  trigger?(): HostRef | null;
  /** A mounted trigger wins over the captured focus owner as the default final focus. */
  preferTrigger?: boolean;
  pressFlag?: { consumes(): boolean; defer(fn: () => void): void };
}
export const DismissableLayer: React.ForwardRefExoticComponent<DismissableLayerProps & React.RefAttributes<HostRef>>;

/** Native exit target of popup content (CMP-002 R17): the content stays mounted, inert, until the exit completes. */
export interface ExitProps {
  initial?: false | Record<string, number>;
  animate?: Record<string, number>;
  exit?: Record<string, number>;
  transition?: { duration?: number; delay?: number; ease?: string | [number, number, number, number] };
}

export namespace Tooltip {
  function Provider(props: { delayDuration?: number; skipDelayDuration?: number; disableHoverableContent?: boolean; children?: ReactNode }): ReactNode;
  const Root: React.ForwardRefExoticComponent<
    HostProps & { open?: boolean; defaultOpen?: boolean; onOpenChange?(open: boolean): void; delayDuration?: number; disableHoverableContent?: boolean } & React.RefAttributes<HostRef>
  >;
  const Trigger: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
  const Content: React.ForwardRefExoticComponent<
    FloatingLayerProps & ExitProps & { onEscapeKeyDown?: DismissableLayerProps["onEscapeKeyDown"] } & React.RefAttributes<HostRef>
  >;
}

export namespace Select {
  type ItemMetadata = Record<string, string> | { value: string; label: string }[];
  const Root: React.ForwardRefExoticComponent<
    HostProps & {
      value?: string;
      defaultValue?: string;
      onValueChange?(value: string): void;
      open?: boolean;
      defaultOpen?: boolean;
      onOpenChange?(open: boolean): void;
      disabled?: boolean;
      items?: ItemMetadata;
    } & React.RefAttributes<HostRef>
  >;
  const Trigger: React.ForwardRefExoticComponent<
    Omit<SlotProps, "style"> & { disabled?: boolean; style?: StyleOrFunction<{ open: boolean; disabled: boolean; placeholder: boolean }> } & React.RefAttributes<HostRef>
  >;
  const Value: React.ForwardRefExoticComponent<HostProps & { placeholder?: ReactNode } & React.RefAttributes<HostRef>>;
  const Content: React.ForwardRefExoticComponent<
    FloatingLayerProps & ExitProps & { onEscapeKeyDown?: DismissableLayerProps["onEscapeKeyDown"] } & React.RefAttributes<HostRef>
  >;
  const Item: React.ForwardRefExoticComponent<
    Omit<SlotProps, "style" | "children"> & {
      value: string;
      disabled?: boolean;
      textValue?: string;
      style?: StyleOrFunction<{ selected: boolean; highlighted: boolean; disabled: boolean }>;
      children?: ReactNode | ((state: { selected: boolean; highlighted: boolean; disabled: boolean }) => ReactNode);
    } & React.RefAttributes<HostRef>
  >;
  const Group: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
  const Label: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
  const Separator: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
  const ScrollUpButton: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
  const ScrollDownButton: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
}

export namespace Dialog {
  /** Renders no element: it provides the dialog state to its parts. */
  function Root(props: {
    open?: boolean;
    defaultOpen?: boolean;
    onOpenChange?(open: boolean): void;
    /** Modal dialogs occlude the page and trap Tab (default true). */
    modal?: boolean;
    /** A press on the backdrop (or outside a nonmodal popup) does not close the dialog. */
    disablePointerDismissal?: boolean;
    /** Escape does not close the dialog (it is still consumed by it). */
    disableEscapeDismissal?: boolean;
    children?: ReactNode;
  }): ReactNode;
  type ActivationProps = Omit<SlotProps, "style"> & {
    disabled?: boolean;
    style?: StyleOrFunction<{ open: boolean; disabled: boolean }>;
    onClick?(event: NativeEvent): void;
  };
  /** Requests `open = true` after the caller's activation; exposes `aria-expanded`. */
  const Trigger: React.ForwardRefExoticComponent<ActivationProps & React.RefAttributes<HostRef>>;
  /** Requests `open = false` after the caller's activation. */
  const Close: React.ForwardRefExoticComponent<ActivationProps & React.RefAttributes<HostRef>>;
  /** The full-window deferred layer (priority 0), mounted only while open. */
  const Portal: React.ForwardRefExoticComponent<HostProps & React.RefAttributes<HostRef>>;
  /** Absolute, inset 0; a pointer press runs the caller's `onPointerDown`, then closes the dialog. */
  const Backdrop: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
  const Popup: React.ForwardRefExoticComponent<
    HostProps & {
      initialFocus?: FocusTarget;
      finalFocus?: DismissableLayerProps["finalFocus"];
      onEscapeKeyDown?: DismissableLayerProps["onEscapeKeyDown"];
    } & React.RefAttributes<HostRef>
  >;
  /** Role `heading` at level 2 unless overridden. */
  const Title: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
  const Description: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
}

export namespace Combobox {
  type ItemState = { selected: boolean; highlighted: boolean; disabled: boolean };
  const Root: React.ForwardRefExoticComponent<
    HostProps & {
      items?: readonly any[];
      /** A string, or an array of strings when `multiple`; null for none. */
      value?: string | string[] | null;
      defaultValue?: string | string[] | null;
      onValueChange?(value: any): void;
      inputValue?: string;
      defaultInputValue?: string;
      onInputValueChange?(value: string): void;
      open?: boolean;
      defaultOpen?: boolean;
      onOpenChange?(open: boolean): void;
      multiple?: boolean;
      disabled?: boolean;
      /** Highlight the first enabled filtered item on every edit. */
      autoHighlight?: boolean | "always";
      /** `null` disables filtering; React passes the raw query. */
      filter?: ((item: any, query: string, toString: (item: any) => string) => boolean) | null;
      itemToStringValue?(item: any): string;
    } & React.RefAttributes<HostRef>
  >;
  const Input: React.ForwardRefExoticComponent<HostProps & { readOnly?: boolean } & React.RefAttributes<HostRef>>;
  const Trigger: React.ForwardRefExoticComponent<
    Omit<SlotProps, "style"> & { disabled?: boolean; style?: StyleOrFunction<{ open: boolean; disabled: boolean }> } & React.RefAttributes<HostRef>
  >;
  const Value: React.ForwardRefExoticComponent<
    Omit<HostProps, "children"> & { placeholder?: ReactNode; children?: ReactNode | ((value: string | string[] | null) => ReactNode) } & React.RefAttributes<HostRef>
  >;
  const Content: React.ForwardRefExoticComponent<
    FloatingLayerProps & { onEscapeKeyDown?: DismissableLayerProps["onEscapeKeyDown"] } & React.RefAttributes<HostRef>
  >;
  const List: React.ForwardRefExoticComponent<
    Omit<SlotProps, "children"> & { children?: ReactNode | ((item: any, index: number) => ReactNode) } & React.RefAttributes<HostRef>
  >;
  const Item: React.ForwardRefExoticComponent<
    Omit<SlotProps, "style" | "children"> & {
      value: string;
      disabled?: boolean;
      style?: StyleOrFunction<ItemState>;
      children?: ReactNode | ((state: ItemState) => ReactNode);
    } & React.RefAttributes<HostRef>
  >;
  /** Mounts only when no item passes the filter. */
  const Empty: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
  const Group: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
  const Label: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
  const Separator: React.ForwardRefExoticComponent<SlotProps & React.RefAttributes<HostRef>>;
}
