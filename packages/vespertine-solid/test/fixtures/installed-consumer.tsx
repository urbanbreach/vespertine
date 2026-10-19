import { createNativeRenderer } from "@vespertine/host";
import { createReferenceNative } from "@vespertine/host/reference";
import { createRoot } from "@vespertine/solid";
import { Counter } from "installed-ui";

const renderer = createNativeRenderer({ native: createReferenceNative() });
createRoot(renderer).render(() => <div><Counter start={7} /></div>);
console.log(JSON.stringify(renderer.snapshot()));
