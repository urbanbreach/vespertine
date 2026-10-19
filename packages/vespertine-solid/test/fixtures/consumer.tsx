// Consumer fixture: TSX with type annotations, a query-string import and a signal.
import { createSignal } from "solid-js";
import { createNativeRenderer } from "@vespertine/host";
import { createReferenceNative } from "@vespertine/host/reference";
import { createRoot } from "@vespertine/solid";
import { Label } from "./label.tsx?variant=1";

type Props = { label: string };

function App(props: Props) {
  const [n, setN] = createSignal<number>(0);
  return (
    <div testId="app" onClick={() => setN(n() + 1)} style={{ width: 10 }}>
      <Label text={`${props.label}: ${n()}`} />
    </div>
  );
}

const renderer = createNativeRenderer({ native: createReferenceNative() });
const root = createRoot(renderer);
root.render(() => <App label="count" />);
renderer.emit({ type: "click", id: renderer.snapshot().children[0].id });
console.log(JSON.stringify(renderer.snapshot()));
