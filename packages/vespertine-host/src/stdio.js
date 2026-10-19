// Standard input inspection, isolated so tests can replace it.

import { fstatSync } from "node:fs";
import { isatty } from "node:tty";

export const tty = {
  stdinExists() {
    try {
      fstatSync(0);
      return true;
    } catch {
      return false;
    }
  },
  stdinIsTerminal() {
    return isatty(0);
  },
};
