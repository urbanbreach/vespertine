import { realpathSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

function realOrSelf(file) {
  try {
    return realpathSync(file);
  } catch {
    return file;
  }
}

/**
 * Whether the module at `moduleUrl` is the file the process was started with, resolving
 * symbolic links (a package manager's command link) and falling back to the unresolved path.
 */
export function isEntryPoint(moduleUrl, entryPath = process.argv[1]) {
  if (!entryPath) return false;
  return realOrSelf(fileURLToPath(moduleUrl)) === realOrSelf(path.resolve(entryPath));
}
