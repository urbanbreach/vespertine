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
