import { createSignal } from "solid-js";

export function Counter(props: { start: number }): any {
  const [n] = createSignal<number>(props.start);
  return <text testId="installed">{`installed ${n()}`}</text>;
}
