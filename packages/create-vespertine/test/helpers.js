import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

export const fixtureDirectory = path.join(path.dirname(fileURLToPath(import.meta.url)), "fixtures", "starter");

function octal(value, width) {
  return `${value.toString(8).padStart(width - 1, "0")}\0`;
}

function header({ name, prefix = "", size, mode, type }) {
  const block = Buffer.alloc(512);
  block.write(name, 0, 100, "utf8");
  block.write(octal(mode, 8), 100, "ascii");
  block.write(octal(0, 8), 108, "ascii");
  block.write(octal(0, 8), 116, "ascii");
  block.write(octal(size, 12), 124, "ascii");
  block.write(octal(0, 12), 136, "ascii");
  block.write("        ", 148, "ascii");
  block.write(type, 156, "ascii");
  block.write("ustar\0", 257, "ascii");
  block.write("00", 263, "ascii");
  block.write(prefix, 345, 155, "utf8");
  let sum = 0;
  for (const byte of block) sum += byte;
  block.write(`${sum.toString(8).padStart(6, "0")}\0 `, 148, "ascii");
  return block;
}

function body(data) {
  const padding = (512 - (data.length % 512)) % 512;
  return Buffer.concat([data, Buffer.alloc(padding)]);
}

function paxRecord(key, value) {
  const payload = ` ${key}=${value}\n`;
  let length = Buffer.byteLength(payload) + 1;
  while (String(length).length + Buffer.byteLength(payload) !== length) {
    length = String(length).length + Buffer.byteLength(payload);
  }
  return `${length}${payload}`;
}

/**
 * Builds a gzipped tarball. Entries: `{ path, data?, mode?, type? }`; a `/`-terminated path
 * or `type: "5"` is a directory. Paths over 100 bytes are stored with a pax extended header.
 * The archive starts with a pax global header like the ones GitHub writes.
 */
export function createTarGz(entries) {
  const parts = [];
  const global = Buffer.from(paxRecord("comment", "0123456789abcdef"));
  parts.push(header({ name: "pax_global_header", size: global.length, mode: 0o666, type: "g" }), body(global));
  for (const entry of entries) {
    const data = Buffer.from(entry.data ?? "");
    const directory = entry.type === "5" || entry.path.endsWith("/");
    let name = entry.path;
    if (Buffer.byteLength(name) > 100) {
      const extended = Buffer.from(paxRecord("path", name));
      parts.push(header({ name: "pax_extended_header", size: extended.length, mode: 0o644, type: "x" }), body(extended));
      name = name.slice(-100);
    }
    parts.push(
      header({
        name,
        size: directory ? 0 : data.length,
        mode: entry.mode ?? (directory ? 0o755 : 0o644),
        type: entry.type ?? (directory ? "5" : "0"),
      }),
    );
    if (!directory) parts.push(body(data));
  }
  parts.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(parts));
}

function walk(directory, base = "") {
  const files = [];
  for (const item of readdirSync(path.join(directory, base), { withFileTypes: true })) {
    const relative = path.posix.join(base, item.name);
    if (item.isDirectory()) files.push(...walk(directory, relative));
    else files.push(relative);
  }
  return files;
}

/** The fixture starter as archive entries below `<root>/<subtree>/`. */
export function starterEntries(root = "vespertine-starter-main", subtree = "packages/starter") {
  const entries = [{ path: `${root}/` }, { path: `${root}/${subtree}/` }, { path: `${root}/README.md`, data: "not part of the starter\n" }];
  for (const file of walk(fixtureDirectory)) {
    entries.push({
      path: `${root}/${subtree}/${file}`,
      data: readFileSync(path.join(fixtureDirectory, file)),
      mode: file.endsWith(".sh") ? 0o755 : 0o644,
    });
  }
  return entries;
}

/** A `fetch` stub that serves the archive and metadata and records every request. */
export function stubFetch({ archive, version = "1.2.3", archiveStatus = 200, metadataStatus = 200 } = {}) {
  const calls = [];
  const fetchStub = async (url, init = {}) => {
    calls.push({ url: String(url), headers: init.headers ?? {} });
    const isArchive = String(url).includes("archive");
    const status = isArchive ? archiveStatus : metadataStatus;
    return {
      ok: status >= 200 && status < 300,
      status,
      arrayBuffer: async () => {
        const buffer = archive ?? createTarGz(starterEntries());
        return buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength);
      },
      json: async () => ({ name: "@vespertine/react", version }),
    };
  };
  fetchStub.calls = calls;
  return fetchStub;
}

export const stubUrls = { archiveUrl: "https://example.test/archive.tar.gz", registryUrl: "https://example.test/registry/latest" };
