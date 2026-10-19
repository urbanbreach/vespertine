import { useState } from "react";
import { render } from "@vespertine/react";

function App() {
  const [clicks, setClicks] = useState(0);
  return (
    <div onClick={() => setClicks(clicks + 1)}>
      Hello from Vespertine. Clicked {clicks} times.
    </div>
  );
}

render(<App />);
