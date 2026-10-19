import { spawn as nodeSpawn } from "node:child_process";
import { copyFile, mkdir, readFile, rm, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

/** The curated jobs and the destination directory of the documentation screenshots. */
export const DEFAULT_CONFIG = fileURLToPath(new URL("../screenshots.config.json", import.meta.url));

/** ImageMagick: strip metadata, 256-color palette, PNG compression level 9. */
export const MAGICK_OPTIONS = ["-strip", "-colors", "256", "-define", "png:compression-level=9"];

/** ImageMagick 7 (`magick`) first, then the ImageMagick 6 `convert` entry point. */
const MAGICK_COMMANDS = ["magick", "convert"];

function run(spawn, command, args, options) {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(command, args, options);
    } catch (error) {
      resolve({ code: null, error });
      return;
    }
    child.once("error", (error) => resolve({ code: null, error }));
    child.once("exit", (code, signal) => resolve({ code, signal }));
  });
}

async function fileExists(file) {
  try {
    return (await stat(file)).isFile();
  } catch {
    return false;
  }
}

/**
 * Writes `source` to `destination` with ImageMagick (stripped, 256 colors, PNG level 9). If
 * ImageMagick is unavailable or fails, copies the original unchanged and logs that.
 * @returns {Promise<"converted" | "copied">}
 */
export async function writeOptimized({ source, destination, spawn = nodeSpawn, env = process.env, log }) {
  await mkdir(path.dirname(destination), { recursive: true });
  let problem = "ImageMagick (magick/convert) is not available";
  for (const command of MAGICK_COMMANDS) {
    const result = await run(spawn, command, [source, ...MAGICK_OPTIONS, destination], { env, stdio: "ignore" });
    if (result.code === 0) return "converted";
    if (result.error?.code === "ENOENT") continue;
    problem = `${command} failed (${result.error?.message ?? result.signal ?? `exit code ${result.code}`})`;
    break;
  }
  await rm(destination, { force: true });
  await copyFile(source, destination);
  log(`${problem}; copied the original ${source} to ${destination} unchanged`);
  return "copied";
}

/**
 * Runs the curated rendering jobs one after another and writes their images to the
 * destination. Stops at the first failing job or missing expected image.
 *
 * @param {object} options
 * @param {string} [options.config] config file (`{ destination, jobs: [{ name, command, images: [{ source, name }] }] }`)
 * @param {string} [options.root] directory the config's relative paths and commands are resolved in
 * @param {string} [options.destination] overrides the config's destination
 * @param {(message: string) => void} [options.log]
 * @returns {Promise<{ written: string[] }>} rejects with the reason on the first failure
 */
export async function collectScreenshots({
  config = DEFAULT_CONFIG,
  root = process.cwd(),
  destination,
  spawn = nodeSpawn,
  env = process.env,
  log = (message) => process.stderr.write(`[screenshots] ${message}\n`),
} = {}) {
  const { destination: configured, jobs } = JSON.parse(await readFile(config, "utf8"));
  const outputDirectory = path.resolve(root, destination ?? configured);
  const written = [];
  for (const job of jobs) {
    log(`running ${job.name}: ${job.command.join(" ")}`);
    const [command, ...args] = job.command;
    const result = await run(spawn, command, args, { cwd: root, env, stdio: "inherit" });
    if (result.code !== 0) {
      const reason = result.error?.message ?? result.signal ?? `exit code ${result.code}`;
      throw new Error(`Screenshot job "${job.name}" failed (${reason})`);
    }
    for (const image of job.images) {
      const source = path.resolve(root, image.source);
      if (!(await fileExists(source))) {
        throw new Error(`Screenshot job "${job.name}" did not produce the expected image: ${source}`);
      }
      const target = path.join(outputDirectory, image.name ?? path.basename(image.source));
      await writeOptimized({ source, destination: target, spawn, env, log });
      written.push(target);
      log(`wrote ${target}`);
    }
  }
  return { written };
}
