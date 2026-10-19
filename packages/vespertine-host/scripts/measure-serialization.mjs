// DEV-008 R1: the JavaScript-side serialization cost of the recorded mutation queue. Reads the
// same fixture the native benchmark (`mutation_import_bench`) parses, checks that re-serializing
// the operations reproduces the recorded bytes exactly, and times JSON.stringify.
//
//   node scripts/measure-serialization.mjs [fixture.ndjson] [iterations]
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const path = process.argv[2] ?? join(here, "../../../crates/vespertine_host_tree/fixtures/react_mutation_queue.ndjson");
const iterations = Number(process.argv[3] ?? 5);

const lines = readFileSync(path, "utf8").split("\n").filter(Boolean);
const batches = lines.map((line) => JSON.parse(line));
for (const [index, line] of lines.entries()) {
  assert.equal(JSON.stringify(batches[index]), line, `batch ${index} does not reserialize to the recorded bytes`);
}
const bytes = lines.reduce((sum, line) => sum + Buffer.byteLength(line), 0);
const operations = batches.reduce((sum, batch) => sum + batch.length, 0);

const samples = [];
for (let i = 0; i < iterations; i++) {
  const started = performance.now();
  for (const batch of batches) JSON.stringify(batch);
  samples.push(performance.now() - started);
}
samples.sort((a, b) => a - b);
console.log(
  `${path}: ${batches.length} batches, ${operations} operations, ${bytes} bytes; ` +
    `JSON.stringify median ${samples[Math.floor(samples.length / 2)].toFixed(3)} ms over ${iterations} iterations`,
);
