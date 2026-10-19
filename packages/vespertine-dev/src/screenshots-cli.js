#!/usr/bin/env node
import { isEntryPoint } from "./entry.js";
import { collectScreenshots, DEFAULT_CONFIG } from "./screenshots.js";

const HELP = `Usage: vespertine-screenshots [--config <file>] [--root <dir>] [--destination <dir>]

Runs the curated screenshot jobs one after another (stopping at the first failure or missing
image) and writes each image with metadata stripped, 256 colors and PNG compression level 9
(ImageMagick); without ImageMagick the original image is copied.

Options:
  --config <file>        Jobs and destination (default: ${DEFAULT_CONFIG})
  --root <dir>           Directory the config's paths and commands are relative to (default: current directory)
  --destination <dir>    Overrides the config's destination
  -h, --help             Show this help
`;

export async function main(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === "-h" || flag === "--help") {
      process.stdout.write(HELP);
      return 0;
    }
    const key = { "--config": "config", "--root": "root", "--destination": "destination" }[flag];
    if (!key || argv[index + 1] === undefined) {
      process.stderr.write(`vespertine-screenshots: ${key ? `${flag} needs a value` : `unknown argument "${flag}"`}\n`);
      return 2;
    }
    options[key] = argv[++index];
  }
  try {
    await collectScreenshots(options);
    return 0;
  } catch (error) {
    process.stderr.write(`vespertine-screenshots: ${error.message}\n`);
    return 1;
  }
}

if (isEntryPoint(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
