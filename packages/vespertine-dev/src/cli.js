#!/usr/bin/env node
import { DEFAULT_SHOTS_PATTERN, startDev } from "./dev.js";
import { isEntryPoint } from "./entry.js";

const HELP = `Usage: vespertine-dev [--app <example> | --shots [pattern]] [options]

Watches the Rust sources, rebuilds after changes (120 ms quiet period) and either restarts
an example or re-runs the screenshot tests. Without --app it runs the screenshot tests.

Options:
  --app <example>      Restart this cargo example after every successful build
  --shots [pattern]    Run the tests matching the pattern (default: ${DEFAULT_SHOTS_PATTERN}) after every successful build
  -p, --package <crate>  Build and test only this cargo package
  --watch <dir>        Directory to watch (repeatable; default: <root>/crates)
  --root <dir>         Workspace root (default: the current directory)
  -h, --help           Show this help
`;

/**
 * Parses the command line into options for `startDev`. Throws on invalid usage.
 * @param {string[]} argv
 */
export function parseArguments(argv) {
  const options = { mode: undefined, app: undefined, pattern: undefined, watchDirs: [], help: false };
  const needValue = (flag, value) => {
    if (value === undefined || value.startsWith("-")) {
      throw new Error(`${flag} needs a value`);
    }
    return value;
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    const equals = argument.startsWith("--") ? argument.indexOf("=") : -1;
    const flag = equals === -1 ? argument : argument.slice(0, equals);
    const inline = equals === -1 ? undefined : argument.slice(equals + 1);
    const value = () => (inline !== undefined ? inline : needValue(flag, argv[++index]));
    const optionalValue = () => {
      if (inline !== undefined) return inline;
      return argv[index + 1] !== undefined && !argv[index + 1].startsWith("-") ? argv[++index] : undefined;
    };
    switch (flag) {
      case "-h":
      case "--help":
        options.help = true;
        break;
      case "--app":
        if (options.mode === "shots") throw new Error("--app and --shots cannot be combined");
        options.mode = "app";
        // A missing name is reported by startDev before anything is built.
        options.app = optionalValue();
        break;
      case "--shots":
        if (options.mode === "app") throw new Error("--app and --shots cannot be combined");
        options.mode = "shots";
        options.pattern = optionalValue();
        break;
      case "-p":
      case "--package":
        options.packageName = value();
        break;
      case "--watch":
        options.watchDirs.push(value());
        break;
      case "--root":
        options.root = value();
        break;
      default:
        throw new Error(`unknown argument "${argument}"`);
    }
  }
  options.mode ??= "shots";
  if (options.watchDirs.length === 0) delete options.watchDirs;
  return options;
}

/** Runs the launcher; resolves with an exit code when it fails before the watch loop starts. */
export async function main(argv) {
  let options;
  try {
    options = parseArguments(argv);
  } catch (error) {
    process.stderr.write(`vespertine-dev: ${error.message}\n`);
    return 2;
  }
  if (options.help) {
    process.stdout.write(HELP);
    return 0;
  }
  const { help: _help, ...startOptions } = options;
  try {
    await startDev({ ...startOptions, handleSignals: true });
  } catch (error) {
    process.stderr.write(`vespertine-dev: ${error.message}\n`);
    return 1;
  }
  return null;
}

if (isEntryPoint(import.meta.url)) {
  const code = await main(process.argv.slice(2));
  if (code !== null) process.exit(code);
}
