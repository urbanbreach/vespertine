#!/usr/bin/env node
import { readFileSync, realpathSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { REGISTRY_URL, STARTER_ARCHIVE_URL } from "./config.js";
import { createProject, installWithBun } from "./create.js";

const NAME = "create-vespertine";
const COMMANDS = ["new", "completion", "help"];
const OPTIONS = ["--no-install", "--help", "--version"];

const HELP = `Usage: ${NAME} <command>

Commands:
  new <target> [--no-install]   Create a Vespertine project in <target>
  completion [bash|zsh|fish]    Print a shell completion script
  help                          Show this help

Options:
  --no-install                  Skip installing dependencies (bun install)
  -h, --help                    Show this help
  -v, --version                 Show the version

Environment:
  GITHUB_TOKEN                          Sent as a bearer token on the starter archive request
  VESPERTINE_STARTER_ARCHIVE_URL        Starter archive endpoint (default ${STARTER_ARCHIVE_URL})
  VESPERTINE_REGISTRY_URL               Adapter package metadata endpoint (default ${REGISTRY_URL})
`;

const COMPLETIONS = {
  bash: `_create_vespertine() {
  local current="\${COMP_WORDS[COMP_CWORD]}"
  if [ "$COMP_CWORD" -eq 1 ]; then
    COMPREPLY=($(compgen -W "${COMMANDS.join(" ")} ${OPTIONS.join(" ")}" -- "$current"))
  elif [ "\${COMP_WORDS[1]}" = "completion" ]; then
    COMPREPLY=($(compgen -W "bash zsh fish" -- "$current"))
  elif [ "\${COMP_WORDS[1]}" = "new" ]; then
    COMPREPLY=($(compgen -W "--no-install" -- "$current") $(compgen -d -- "$current"))
  fi
}
complete -F _create_vespertine ${NAME}
`,
  zsh: `#compdef ${NAME}
_create_vespertine() {
  local -a commands
  commands=('new:Create a Vespertine project' 'completion:Print a shell completion script' 'help:Show help')
  if (( CURRENT == 2 )); then
    _describe 'command' commands
    _values 'option' ${OPTIONS.join(" ")}
  elif [[ $words[2] == completion ]]; then
    _values 'shell' bash zsh fish
  elif [[ $words[2] == new ]]; then
    _arguments '--no-install[skip installing dependencies]' '*:directory:_files -/'
  fi
}
_create_vespertine "$@"
`,
  fish: `complete -c ${NAME} -f
complete -c ${NAME} -n '__fish_use_subcommand' -a new -d 'Create a Vespertine project'
complete -c ${NAME} -n '__fish_use_subcommand' -a completion -d 'Print a shell completion script'
complete -c ${NAME} -n '__fish_use_subcommand' -a help -d 'Show help'
complete -c ${NAME} -n '__fish_use_subcommand' -l help -s h -d 'Show help'
complete -c ${NAME} -n '__fish_use_subcommand' -l version -s v -d 'Show the version'
complete -c ${NAME} -n '__fish_seen_subcommand_from new' -l no-install -d 'Skip installing dependencies'
complete -c ${NAME} -n '__fish_seen_subcommand_from new' -a '(__fish_complete_directories)'
complete -c ${NAME} -n '__fish_seen_subcommand_from completion' -a 'bash zsh fish'
`,
};

function version() {
  const manifest = new URL("../package.json", import.meta.url);
  return JSON.parse(readFileSync(manifest, "utf8")).version;
}

function realOrSelf(file) {
  try {
    return realpathSync(file);
  } catch {
    return file;
  }
}

/**
 * Whether the module at `moduleUrl` is the file the process was started with. Symbolic links
 * are resolved on both sides (a package manager's command link points at the command file);
 * a path that cannot be resolved is compared as given.
 */
export function isEntryPoint(moduleUrl, entryPath = process.argv[1]) {
  if (!entryPath) return false;
  return realOrSelf(fileURLToPath(moduleUrl)) === realOrSelf(path.resolve(entryPath));
}

/**
 * Runs the command line `argv` (without the executable and script) and returns the exit code.
 *
 * @param {string[]} argv
 * @param {object} [context]
 */
export async function main(argv, context = {}) {
  const {
    cwd = process.cwd(),
    env = process.env,
    stdout = process.stdout,
    stderr = process.stderr,
    fetch: fetchImpl = globalThis.fetch,
    install = installWithBun,
  } = context;
  const out = (message) => stdout.write(`${message}\n`);
  const fail = (message) => {
    stderr.write(`${NAME}: ${message}\n`);
    return 2;
  };

  const [command, ...rest] = argv;
  if (command === undefined || command === "help" || command === "--help" || command === "-h") {
    stdout.write(HELP);
    return 0;
  }
  if (command === "--version" || command === "-v") {
    out(version());
    return 0;
  }
  if (command === "completion") {
    const shell = rest[0] ?? "bash";
    if (rest.length > 1 || !(shell in COMPLETIONS)) {
      return fail(`completion supports bash, zsh and fish (got: ${rest.join(" ")})`);
    }
    stdout.write(COMPLETIONS[shell]);
    return 0;
  }
  if (command !== "new") {
    return fail(`unknown command "${command}" (try ${NAME} help)`);
  }

  let noInstall = false;
  const positional = [];
  for (const argument of rest) {
    if (argument === "--no-install") noInstall = true;
    else if (argument.startsWith("-")) return fail(`unknown option "${argument}"`);
    else positional.push(argument);
  }
  if (positional.length !== 1) {
    return fail("new takes exactly one target directory name");
  }

  const targetDirectory = path.resolve(cwd, positional[0]);
  try {
    await createProject({
      targetDirectory,
      githubToken: env.GITHUB_TOKEN || undefined,
      fetch: fetchImpl,
      archiveUrl: env.VESPERTINE_STARTER_ARCHIVE_URL || undefined,
      registryUrl: env.VESPERTINE_REGISTRY_URL || undefined,
    });
  } catch (error) {
    stderr.write(`${NAME}: ${error.message}\n`);
    return 1;
  }
  out(`Created ${path.basename(targetDirectory)} in ${targetDirectory}`);

  const relative = path.relative(cwd, targetDirectory) || ".";
  let installed = !noInstall;
  if (!noInstall) {
    try {
      await install(targetDirectory);
    } catch (error) {
      installed = false;
      stderr.write(`${NAME}: ${error.message}\n`);
      stderr.write(`${NAME}: the project was created; run "bun install" in it once the problem is fixed\n`);
    }
  }
  out("");
  out("Next steps:");
  out(`  cd ${relative}`);
  if (!installed) out("  bun install");
  out("  bun run start");
  return installed || noInstall ? 0 : 1;
}

if (isEntryPoint(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
