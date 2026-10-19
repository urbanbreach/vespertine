import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { createTarGz, starterEntries } from "./helpers.js";

const run = promisify(execFile);
const cliPath = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "src", "cli.js");

test("A4: the new command creates a project from a local HTTP server end to end", async () => {
  const requests = [];
  const archive = createTarGz(starterEntries());
  const server = http.createServer((request, response) => {
    requests.push({ url: request.url, authorization: request.headers.authorization });
    if (request.url === "/starter.tar.gz") {
      response.writeHead(200, { "content-type": "application/gzip" });
      response.end(archive);
    } else if (request.url === "/registry/latest") {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ name: "@vespertine/react", version: "0.9.4" }));
    } else {
      response.writeHead(404).end();
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const cwd = mkdtempSync(path.join(tmpdir(), "create-vespertine-e2e-"));
  try {
    const { port } = server.address();
    const env = {
      ...process.env,
      GITHUB_TOKEN: "e2e-token",
      VESPERTINE_STARTER_ARCHIVE_URL: `http://127.0.0.1:${port}/starter.tar.gz`,
      VESPERTINE_REGISTRY_URL: `http://127.0.0.1:${port}/registry/latest`,
    };
    const { stdout } = await run(process.execPath, [cliPath, "new", "demo-app", "--no-install"], { cwd, env });
    assert.match(stdout, /Created demo-app/);
    assert.match(stdout, /cd demo-app/);

    const project = path.join(cwd, "demo-app");
    const manifest = JSON.parse(readFileSync(path.join(project, "package.json"), "utf8"));
    assert.equal(manifest.name, "demo-app");
    assert.equal(manifest.dependencies["@vespertine/react"], "^0.9.4");
    assert.ok(existsSync(path.join(project, "src", "main.tsx")));
    assert.notEqual(statSync(path.join(project, "scripts", "run.sh")).mode & 0o100, 0);
    assert.equal(existsSync(path.join(project, "node_modules")), false);

    assert.equal(requests.find((r) => r.url === "/starter.tar.gz").authorization, "Bearer e2e-token");
    assert.equal(requests.find((r) => r.url === "/registry/latest").authorization, undefined);

    // A second run into the same directory is rejected without any network access.
    const before = requests.length;
    await assert.rejects(run(process.execPath, [cliPath, "new", "demo-app", "--no-install"], { cwd, env }), (error) => {
      assert.match(error.stderr, /already exists/);
      return true;
    });
    assert.equal(requests.length, before);
  } finally {
    server.close();
    rmSync(cwd, { recursive: true, force: true });
  }
});
