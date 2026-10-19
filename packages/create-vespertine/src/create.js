import { spawn as nodeSpawn } from "node:child_process";
import { promises as fs } from "node:fs";
import path from "node:path";

import { ADAPTER_PACKAGE, REGISTRY_URL, STARTER_ARCHIVE_URL, STARTER_SUBTREE } from "./config.js";
import { parseTarGz } from "./tar.js";

/** A network request that completed with a non-success HTTP status. */
export class RequestError extends Error {
  /**
   * @param {"archive" | "metadata"} request which of the two requests failed
   * @param {number} status HTTP status code
   * @param {string} url
   */
  constructor(request, status, url) {
    const label = request === "archive" ? "Starter archive" : "Adapter package metadata";
    super(`${label} request failed with status ${status} (${url})`);
    this.name = "RequestError";
    this.request = request;
    this.status = status;
  }
}

async function exists(target) {
  try {
    await fs.lstat(target);
    return true;
  } catch (error) {
    if (error.code === "ENOENT") return false;
    throw error;
  }
}

/** Splits an archive path into its components, dropping the root directory component. */
function innerComponents(entryPath) {
  return entryPath.split("/").filter((part) => part !== "" && part !== ".").slice(1);
}

/** The non-directory entries of the starter subtree with their paths relative to it. */
function starterFiles(entries, subtree) {
  const wanted = subtree.split("/").filter((part) => part !== "");
  const files = [];
  const byPath = new Map();
  for (const entry of entries) {
    const parts = innerComponents(entry.path);
    if (parts.length <= wanted.length || !wanted.every((part, index) => parts[index] === part)) {
      continue;
    }
    const relative = parts.slice(wanted.length);
    if (relative.includes("..") || path.isAbsolute(relative.join("/"))) {
      throw new Error(`Unsafe path in the starter archive: ${entry.path}`);
    }
    const relativePath = relative.join("/");
    if (entry.type === "directory") continue;
    if (entry.type === "link") {
      // A hard link repeats the content of an earlier entry of the archive.
      const target = innerComponents(entry.linkname).slice(wanted.length).join("/");
      const original = byPath.get(target);
      if (!original) throw new Error(`Unresolvable hard link in the starter archive: ${entry.path}`);
      files.push({ ...original, path: relativePath });
      continue;
    }
    if (entry.type === "other") {
      throw new Error(`Unsupported entry type in the starter archive: ${entry.path}`);
    }
    const file = { path: relativePath, type: entry.type, mode: entry.mode, data: entry.data, linkname: entry.linkname };
    byPath.set(relativePath, file);
    files.push(file);
  }
  return files;
}

async function writeFiles(files, target) {
  for (const file of files) {
    const destination = path.join(target, file.path);
    await fs.mkdir(path.dirname(destination), { recursive: true });
    if (file.type === "symlink") {
      const resolved = path.resolve(path.dirname(destination), file.linkname);
      if (path.isAbsolute(file.linkname) || path.relative(target, resolved).startsWith("..")) {
        throw new Error(`Symbolic link leaves the project: ${file.path} -> ${file.linkname}`);
      }
      await fs.symlink(file.linkname, destination);
    } else {
      await fs.writeFile(destination, file.data, { mode: file.mode || 0o644 });
      await fs.chmod(destination, file.mode || 0o644);
    }
  }
}

async function editManifest(target, name, version) {
  const manifestPath = path.join(target, "package.json");
  const manifest = JSON.parse(await fs.readFile(manifestPath, "utf8"));
  manifest.name = name;
  const sections = ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"];
  const section = sections.find((key) => manifest[key] && ADAPTER_PACKAGE in manifest[key]) ?? "dependencies";
  manifest[section] = { ...manifest[section], [ADAPTER_PACKAGE]: `^${version}` };
  await fs.writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
}

/**
 * Creates a project from the published starter.
 *
 * @param {object} options
 * @param {string} options.targetDirectory directory to create; must not exist
 * @param {string} [options.githubToken] bearer token, sent on the archive request only
 * @param {typeof fetch} [options.fetch] network layer
 * @param {(targetDirectory: string) => Promise<void> | void} [options.install] runs after the
 *   project is on disk; its failure is rethrown but never removes the project
 * @param {string} [options.archiveUrl]
 * @param {string} [options.registryUrl]
 * @param {string} [options.subtree] starter location inside the archive
 * @returns {Promise<void>} resolves when the project is on disk (and installed)
 */
export async function createProject({
  targetDirectory,
  githubToken,
  fetch: fetchImpl = globalThis.fetch,
  install,
  archiveUrl = STARTER_ARCHIVE_URL,
  registryUrl = REGISTRY_URL,
  subtree = STARTER_SUBTREE,
}) {
  const target = path.resolve(targetDirectory);
  if (await exists(target)) {
    throw new Error(`Target directory already exists: ${target}`);
  }

  const archiveHeaders = githubToken ? { Authorization: `Bearer ${githubToken}` } : {};
  const [archive, metadata] = await Promise.all([
    (async () => {
      const response = await fetchImpl(archiveUrl, { headers: archiveHeaders });
      if (!response.ok) throw new RequestError("archive", response.status, archiveUrl);
      return Buffer.from(await response.arrayBuffer());
    })(),
    (async () => {
      const response = await fetchImpl(registryUrl, { headers: { Accept: "application/json" } });
      if (!response.ok) throw new RequestError("metadata", response.status, registryUrl);
      return response.json();
    })(),
  ]);

  const version = metadata?.version;
  if (typeof version !== "string" || version === "") {
    throw new Error(`The metadata of ${ADAPTER_PACKAGE} has no version`);
  }
  const files = starterFiles(parseTarGz(archive), subtree);
  if (files.length === 0) {
    throw new Error(`The starter archive has no starter project at ${subtree}`);
  }

  await fs.mkdir(path.dirname(target), { recursive: true });
  await fs.mkdir(target);
  try {
    await writeFiles(files, target);
    await editManifest(target, path.basename(target), version);
  } catch (error) {
    await fs.rm(target, { recursive: true, force: true });
    throw error;
  }

  if (install) await install(target);
}

/**
 * Installs the project's dependencies with the Bun package manager, output inherited from the
 * parent process.
 */
export function installWithBun(targetDirectory, { spawn = nodeSpawn } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn("bun", ["install"], { cwd: targetDirectory, stdio: "inherit" });
    child.once("error", (error) => {
      reject(new Error(`Could not run "bun install": ${error.message}`));
    });
    child.once("exit", (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`"bun install" failed (${signal ?? `exit code ${code}`})`));
    });
  });
}
