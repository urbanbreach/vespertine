// Minimal reader for gzipped tar archives (ustar, pax extended headers and GNU long names),
// enough for the archives GitHub serves.

import { gunzipSync } from "node:zlib";

const BLOCK = 512;

function text(header, start, length) {
  const field = header.subarray(start, start + length);
  const end = field.indexOf(0);
  return field.subarray(0, end === -1 ? length : end).toString("utf8");
}

function number(header, start, length) {
  const field = header.subarray(start, start + length);
  if (field[0] & 0x80) {
    // GNU base-256 encoding for values that do not fit the octal field.
    let value = BigInt(field[0] & 0x7f);
    for (let index = 1; index < field.length; index += 1) {
      value = (value << 8n) | BigInt(field[index]);
    }
    return Number(value);
  }
  const digits = text(header, start, length).trim();
  return digits === "" ? 0 : Number.parseInt(digits, 8);
}

function verifyChecksum(header) {
  let sum = 0;
  for (let index = 0; index < BLOCK; index += 1) {
    sum += index >= 148 && index < 156 ? 0x20 : header[index];
  }
  if (sum !== number(header, 148, 8)) {
    throw new Error("Invalid tar archive: header checksum mismatch");
  }
}

function parsePax(data) {
  const fields = {};
  let offset = 0;
  while (offset < data.length) {
    const space = data.indexOf(0x20, offset);
    if (space === -1) break;
    const length = Number.parseInt(data.subarray(offset, space).toString("utf8"), 10);
    if (!Number.isInteger(length) || length <= space - offset + 1) {
      throw new Error("Invalid tar archive: malformed pax record");
    }
    const record = data.subarray(space + 1, offset + length - 1).toString("utf8");
    const equals = record.indexOf("=");
    if (equals !== -1) fields[record.slice(0, equals)] = record.slice(equals + 1);
    offset += length;
  }
  return fields;
}

const TYPES = new Map([
  ["0", "file"],
  ["\0", "file"],
  ["7", "file"],
  ["1", "link"],
  ["2", "symlink"],
  ["5", "directory"],
]);

/**
 * Parses an uncompressed tar archive.
 * @param {Buffer} tar
 * @returns {{ path: string, type: "file" | "directory" | "symlink" | "link" | "other", mode: number, data: Buffer, linkname: string }[]}
 */
export function parseTar(tar) {
  const entries = [];
  let offset = 0;
  let pax = {};
  let longName = null;
  let longLink = null;
  while (offset + BLOCK <= tar.length) {
    const header = tar.subarray(offset, offset + BLOCK);
    if (header.every((byte) => byte === 0)) break;
    verifyChecksum(header);
    const size = number(header, 124, 12);
    const flag = String.fromCharCode(header[156]);
    const bodyStart = offset + BLOCK;
    const bodyEnd = bodyStart + size;
    if (bodyEnd > tar.length) {
      throw new Error("Invalid tar archive: truncated entry");
    }
    const data = tar.subarray(bodyStart, bodyEnd);
    offset = bodyStart + Math.ceil(size / BLOCK) * BLOCK;

    if (flag === "x") {
      pax = parsePax(data);
    } else if (flag === "g") {
      // Global extended headers (GitHub's commit id) carry nothing this reader needs.
    } else if (flag === "L") {
      longName = text(data, 0, data.length);
    } else if (flag === "K") {
      longLink = text(data, 0, data.length);
    } else {
      let name = text(header, 0, 100);
      const prefix = text(header, 345, 155);
      if (text(header, 257, 5) === "ustar" && prefix !== "") name = `${prefix}/${name}`;
      name = pax.path ?? longName ?? name;
      const linkname = pax.linkpath ?? longLink ?? text(header, 157, 100);
      entries.push({
        path: name,
        type: TYPES.get(flag) ?? "other",
        mode: number(header, 100, 8) & 0o777,
        data: Buffer.from(data),
        linkname,
      });
      pax = {};
      longName = null;
      longLink = null;
    }
  }
  return entries;
}

/** Decompresses and parses a gzipped tar archive. */
export function parseTarGz(archive) {
  return parseTar(gunzipSync(archive));
}
