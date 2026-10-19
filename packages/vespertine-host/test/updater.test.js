// checkUpdate through the built addon against a local HTTP server (APP-004).

import assert from "node:assert/strict";
import { createPrivateKey, createPublicKey, sign } from "node:crypto";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:http";
import { chmodSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { checkUpdate } from "../src/index.js";

// A Minisign key pair (legacy "Ed" signatures) made with node:crypto.
const KEY_ID = Buffer.from("JSTEST01");
const privateKey = createPrivateKey({
  key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), Buffer.alloc(32, 5)]),
  format: "der",
  type: "pkcs8",
});
const rawPublicKey = createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32);
const b64 = (data) => Buffer.from(data).toString("base64");
const pubkey = b64(
  `untrusted comment: minisign public key\n${b64(Buffer.concat([Buffer.from("Ed"), KEY_ID, rawPublicKey]))}\n`,
);

function signature(data) {
  const signed = sign(null, data, privateKey);
  const comment = "timestamp:1\tfile:package";
  const global = sign(null, Buffer.concat([signed, Buffer.from(comment)]), privateKey);
  return b64(
    `untrusted comment: signature\n${b64(Buffer.concat([Buffer.from("Ed"), KEY_ID, signed]))}\n` +
      `trusted comment: ${comment}\n${b64(global)}\n`,
  );
}

/** Serves `routes` (path -> {status, body, headers}) and records requests. */
async function serve(routes) {
  const requests = [];
  const server = createServer((request, response) => {
    requests.push({ url: request.url, headers: request.headers });
    const route = routes(request.url, `http://127.0.0.1:${server.address().port}`) ?? { status: 404 };
    response.writeHead(route.status ?? 200, route.headers ?? {});
    response.end(route.body);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  return {
    requests,
    url: (path) => `http://127.0.0.1:${server.address().port}${path}`,
    close: () => server.close(),
  };
}

const manifest = (base, version, packageBytes, extra = {}) =>
  JSON.stringify({
    version,
    url: `${base}/package`,
    signature: signature(packageBytes),
    format: "AppImage",
    notes: "Release notes",
    pub_date: "2025-01-02T03:04:05Z",
    ...extra,
  });

test("checkUpdate resolves with a read-only update description", async () => {
  const bytes = Buffer.from("NEW APP");
  const server = await serve((path, base) =>
    path.startsWith("/feed") ? { body: manifest(base, "v2.0.0", bytes) } : undefined,
  );
  try {
    const update = await checkUpdate("v1.0.0", {
      endpoints: [server.url("/feed?v={{current_version}}&t={{target}}")],
      pubkey,
      headers: [{ name: "x-token", value: "secret" }],
      timeoutMs: 5000,
      installMode: "quiet",
      installerArgs: ["/S"],
    });
    assert.equal(update.currentVersion, "1.0.0");
    assert.equal(update.version, "2.0.0");
    assert.equal(update.notes, "Release notes");
    assert.equal(update.date, "2025-01-02T03:04:05Z");
    assert.equal(update.downloadUrl, server.url("/package"));
    assert.equal(update.format, "appimage");
    assert.equal(typeof update.downloadAndInstall, "function");
    for (const name of ["currentVersion", "version", "notes", "date", "downloadUrl", "format"]) {
      assert.throws(() => {
        update[name] = "changed";
      }, TypeError);
    }
    assert.equal(server.requests[0].url, "/feed?v=1.0.0&t=linux");
    assert.equal(server.requests[0].headers["x-token"], "secret");
    assert.equal(server.requests[0].headers.accept, "application/json");
  } finally {
    server.close();
  }
});

test("checkUpdate resolves null for 204 and for a release that is not newer", async () => {
  const server = await serve((path, base) =>
    path === "/none" ? { status: 204 } : { body: manifest(base, "1.0.0", Buffer.from("x")) },
  );
  try {
    assert.equal(await checkUpdate("1.0.0", { endpoints: [server.url("/none")], pubkey }), null);
    assert.equal(await checkUpdate("v1.0.0", { endpoints: [server.url("/same")], pubkey }), null);
    assert.equal(await checkUpdate("1.0.0", { endpoints: [server.url("/none"), server.url("/same")], pubkey }), null);
    assert.deepEqual(
      server.requests.map((request) => request.url),
      ["/none", "/same", "/none"],
    );
  } finally {
    server.close();
  }
});

test("checkUpdate rejects invalid input before any request", async () => {
  const server = await serve(() => ({ status: 204 }));
  try {
    const good = { endpoints: [server.url("/feed")], pubkey };
    await assert.rejects(checkUpdate("not a version", good), /invalid current version/);
    await assert.rejects(checkUpdate("1.0.0", { ...good, endpoints: [] }), /at least one update endpoint/);
    await assert.rejects(checkUpdate("1.0.0", { ...good, endpoints: ["not a url"] }), /invalid update endpoint/);
    await assert.rejects(
      checkUpdate("1.0.0", { ...good, headers: [{ name: "bad name", value: "v" }] }),
      /invalid header/,
    );
    await assert.rejects(checkUpdate("1.0.0", { ...good, installMode: "silent" }), /invalid install mode/);
    await assert.rejects(checkUpdate("1.0.0", { ...good, timeoutMs: -1 }), /invalid update options/);
    await assert.rejects(checkUpdate("1.0.0"), /at least one update endpoint/);
    assert.deepEqual(server.requests, []);
  } finally {
    server.close();
  }
});

test("checkUpdate rejects with the last error when every endpoint fails", async () => {
  const server = await serve((path) => (path === "/bad" ? { body: "not json" } : { status: 500 }));
  try {
    await assert.rejects(checkUpdate("1.0.0", { endpoints: [server.url("/bad"), server.url("/err")], pubkey }), /malformed release feed/);
    await assert.rejects(checkUpdate("1.0.0", { endpoints: [server.url("/err")], pubkey }), /could not fetch a valid release/);
  } finally {
    server.close();
  }
});

/** Runs the install script with `APPIMAGE` in the child's environment (never in this process). */
async function runInstaller(appImage, endpoint) {
  const child = spawn(process.execPath, [fileURLToPath(new URL("./updater-install-child.mjs", import.meta.url)), endpoint, pubkey], {
    env: { ...process.env, APPIMAGE: appImage },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (chunk) => (output += chunk));
  child.stderr.on("data", (chunk) => (output += chunk));
  const [code] = await once(child, "exit");
  return { code, output };
}

test("downloadAndInstall verifies the signature, replaces the AppImage and keeps running", async () => {
  const directory = mkdtempSync(join(tmpdir(), "vespertine-updater-"));
  const appImage = join(directory, "app.AppImage");
  writeFileSync(appImage, "OLD APP");
  chmodSync(appImage, 0o750);
  const bytes = Buffer.from("NEW APP ".repeat(50_000));
  const tampered = Buffer.from(bytes);
  tampered[10] ^= 1;
  let served = bytes;
  const server = await serve((path, base) => {
    if (path === "/feed") return { body: manifest(base, "2.0.0", bytes) };
    if (path === "/package") return { body: served };
    return undefined;
  });
  try {
    served = tampered;
    const rejected = await runInstaller(appImage, server.url("/feed"));
    assert.notEqual(rejected.code, 0);
    assert.match(rejected.output, /signature does not match the package/);
    assert.equal(readFileSync(appImage, "utf8"), "OLD APP");

    served = bytes;
    const installed = await runInstaller(appImage, server.url("/feed"));
    assert.equal(installed.code, 0, installed.output);
    assert.match(installed.output, /installed by \d+/);
    assert.deepEqual(readFileSync(appImage), bytes);
    assert.equal(statSync(appImage).mode & 0o7777, 0o750);
    assert.deepEqual(readdirSync(directory), ["app.AppImage"]);
  } finally {
    server.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
