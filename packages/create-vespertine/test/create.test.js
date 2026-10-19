import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";

import { RequestError, createProject, installWithBun } from "../src/create.js";
import { parseTarGz } from "../src/tar.js";
import { createTarGz, starterEntries, stubFetch, stubUrls } from "./helpers.js";

function temporary() {
  return mkdtempSync(path.join(tmpdir(), "create-vespertine-"));
}

async function withTemporary(body) {
  const directory = temporary();
  try {
    await body(directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test("A2: an existing target fails before any fetch", async () => {
  await withTemporary(async (directory) => {
    const target = path.join(directory, "app");
    mkdirSync(target);
    const fetch = stubFetch();
    await assert.rejects(createProject({ targetDirectory: target, fetch, ...stubUrls }), /already exists/);
    assert.equal(fetch.calls.length, 0);
  });
});

test("A2: archive and metadata failures are distinct errors naming the status", async () => {
  await withTemporary(async (directory) => {
    const target = path.join(directory, "app");
    await assert.rejects(
      createProject({ targetDirectory: target, fetch: stubFetch({ archiveStatus: 404 }), ...stubUrls }),
      (error) => {
        assert.ok(error instanceof RequestError);
        assert.equal(error.request, "archive");
        assert.equal(error.status, 404);
        assert.match(error.message, /Starter archive request failed with status 404/);
        return true;
      },
    );
    await assert.rejects(
      createProject({ targetDirectory: target, fetch: stubFetch({ metadataStatus: 503 }), ...stubUrls }),
      (error) => {
        assert.ok(error instanceof RequestError);
        assert.equal(error.request, "metadata");
        assert.equal(error.status, 503);
        assert.match(error.message, /Adapter package metadata request failed with status 503/);
        return true;
      },
    );
    assert.equal(existsSync(target), false);
  });
});

test("A2: both requests are started before either completes", async () => {
  await withTemporary(async (directory) => {
    const started = [];
    const release = [];
    const fetch = async (url) => {
      started.push(String(url));
      await new Promise((resolve) => release.push(resolve));
      const archive = String(url).includes("archive");
      const buffer = createTarGz(starterEntries());
      return {
        ok: true,
        status: 200,
        arrayBuffer: async () => buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength),
        json: async () => ({ version: archive ? "0.0.0" : "4.5.6" }),
      };
    };
    const pending = createProject({ targetDirectory: path.join(directory, "app"), fetch, ...stubUrls });
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.equal(started.length, 2);
    for (const resolve of release) resolve();
    await pending;
  });
});

test("A2: a missing starter subtree fails with no target directory created", async () => {
  await withTemporary(async (directory) => {
    const target = path.join(directory, "app");
    const archive = createTarGz([{ path: "root/" }, { path: "root/packages/other/package.json", data: "{}" }]);
    await assert.rejects(
      createProject({ targetDirectory: target, fetch: stubFetch({ archive }), ...stubUrls }),
      /no starter project/,
    );
    assert.equal(existsSync(target), false);
  });
});

test("A2: a forced extraction failure removes the partial target", async () => {
  await withTemporary(async (directory) => {
    const target = path.join(directory, "app");
    // `clash` is a file and also the parent of `clash/inner`: extraction fails midway.
    const archive = createTarGz([
      ...starterEntries(),
      { path: "vespertine-starter-main/packages/starter/clash", data: "file" },
      { path: "vespertine-starter-main/packages/starter/clash/inner", data: "file" },
    ]);
    await assert.rejects(createProject({ targetDirectory: target, fetch: stubFetch({ archive }), ...stubUrls }));
    assert.equal(existsSync(target), false);
  });
});

test("A2: a manifest editing failure removes the partial target", async () => {
  await withTemporary(async (directory) => {
    const target = path.join(directory, "app");
    const archive = createTarGz([
      { path: "root/packages/starter/package.json", data: "{ not json" },
      { path: "root/packages/starter/src/main.tsx", data: "x" },
    ]);
    await assert.rejects(createProject({ targetDirectory: target, fetch: stubFetch({ archive }), ...stubUrls }), SyntaxError);
    assert.equal(existsSync(target), false);
  });
});

test("A2: the written manifest has the base name, a caret range and two-space indentation", async () => {
  await withTemporary(async (directory) => {
    const target = path.join(directory, "nested", "my-app");
    await createProject({ targetDirectory: target, fetch: stubFetch({ version: "2.7.1" }), ...stubUrls });
    const text = readFileSync(path.join(target, "package.json"), "utf8");
    const manifest = JSON.parse(text);
    assert.equal(manifest.name, "my-app");
    assert.equal(manifest.dependencies["@vespertine/react"], "^2.7.1");
    assert.equal(text, `${JSON.stringify(manifest, null, 2)}\n`);
    assert.match(text, /^{\n {2}"name": "my-app",\n/);
    assert.ok(text.endsWith("}\n"));
    // Every non-directory entry of the subtree is copied, nothing else.
    assert.ok(existsSync(path.join(target, "src", "main.tsx")));
    assert.ok(existsSync(path.join(target, "scripts", "run.sh")));
    assert.notEqual(statSync(path.join(target, "scripts", "run.sh")).mode & 0o100, 0);
    assert.equal(existsSync(path.join(target, "README.md")), false);
  });
});

test("A2: an installation failure leaves the project in place and rethrows", async () => {
  await withTemporary(async (directory) => {
    const target = path.join(directory, "app");
    await assert.rejects(
      createProject({
        targetDirectory: target,
        fetch: stubFetch(),
        install: async () => {
          throw new Error("install failed");
        },
        ...stubUrls,
      }),
      /install failed/,
    );
    assert.ok(existsSync(path.join(target, "package.json")));
  });
});

test("A2: install runs after the project is on disk", async () => {
  await withTemporary(async (directory) => {
    const target = path.join(directory, "app");
    let seen;
    await createProject({
      targetDirectory: target,
      fetch: stubFetch(),
      install: async (dir) => {
        seen = [dir, existsSync(path.join(dir, "package.json"))];
      },
      ...stubUrls,
    });
    assert.deepEqual(seen, [target, true]);
  });
});

test("A2: the token is sent only on the archive request, as a bearer credential", async () => {
  await withTemporary(async (directory) => {
    const withToken = stubFetch();
    await createProject({ targetDirectory: path.join(directory, "a"), githubToken: "tok", fetch: withToken, ...stubUrls });
    const archive = withToken.calls.find((call) => call.url === stubUrls.archiveUrl);
    const metadata = withToken.calls.find((call) => call.url === stubUrls.registryUrl);
    assert.equal(archive.headers.Authorization, "Bearer tok");
    assert.equal(Object.keys(metadata.headers).some((key) => key.toLowerCase() === "authorization"), false);

    const without = stubFetch();
    await createProject({ targetDirectory: path.join(directory, "b"), fetch: without, ...stubUrls });
    for (const call of without.calls) {
      assert.equal(Object.keys(call.headers).some((key) => key.toLowerCase() === "authorization"), false);
    }
  });
});

test("the tar reader handles long paths, directories and entry types", () => {
  const longPath = `root/${"d".repeat(60)}/${"e".repeat(60)}/file.txt`;
  const entries = parseTarGz(
    createTarGz([{ path: "root/" }, { path: longPath, data: "payload", mode: 0o755 }, { path: "root/empty", data: "" }]),
  );
  assert.deepEqual(
    entries.map((entry) => [entry.path, entry.type]),
    [["root/", "directory"], [longPath, "file"], ["root/empty", "file"]],
  );
  assert.equal(entries[1].data.toString(), "payload");
  assert.equal(entries[1].mode, 0o755);
  assert.throws(() => parseTarGz(Buffer.from("not gzip")));
});

test("installation runs bun install in the project with inherited output", async () => {
  const calls = [];
  const spawn = (program, args, options) => {
    calls.push({ program, args, options });
    const child = new EventEmitter();
    queueMicrotask(() => child.emit("exit", 0, null));
    return child;
  };
  await installWithBun("/some/project", { spawn });
  assert.deepEqual(calls, [{ program: "bun", args: ["install"], options: { cwd: "/some/project", stdio: "inherit" } }]);

  const failing = () => {
    const child = new EventEmitter();
    queueMicrotask(() => child.emit("exit", 3, null));
    return child;
  };
  await assert.rejects(installWithBun("/p", { spawn: failing }), /exit code 3/);
  const missing = () => {
    const child = new EventEmitter();
    queueMicrotask(() => child.emit("error", new Error("spawn bun ENOENT")));
    return child;
  };
  await assert.rejects(installWithBun("/p", { spawn: missing }), /Could not run "bun install"/);
});
