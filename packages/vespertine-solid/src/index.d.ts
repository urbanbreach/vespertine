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
