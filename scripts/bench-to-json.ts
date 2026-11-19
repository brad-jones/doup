#!/usr/bin/env bun
// Runs `cargo bench` for galvanize-rs and converts divan's terminal tree output into one JSON
// file per benchmark, written next to the benchmark source, eg. benches/pointers.bench.json.
//
// NB: Divan has no machine readable output, so this parses its human readable tree. It is
// brittle by nature & may need updating if divan changes its output format.
//
// Usage: bun scripts/bench-to-json.ts [divan filter args...]
import { $ } from "bun";
import os from "node:os";

const PACKAGE = "galvanize-rs";
const PACKAGE_DIR = `projects/${PACKAGE}`;

type Value = number | string | null;

interface Result {
  /** Full path through the divan tree, eg. ["clone", "ArcMutexPointer<..>"] */
  path: string[];
  /** Time columns (fastest, slowest, median, mean) are normalised to nanoseconds. */
  [column: string]: Value | string[];
}

const NS_PER_UNIT: Record<string, number> = {
  ps: 0.001,
  ns: 1,
  "µs": 1_000,
  us: 1_000,
  ms: 1_000_000,
  s: 1_000_000_000,
};

function parseValue(raw: string): Value {
  const text = raw.trim();
  if (text === "") return null;
  const time = text.match(/^([\d.]+)\s*(ps|ns|µs|us|ms|s)$/);
  if (time) return parseFloat(time[1]) * NS_PER_UNIT[time[2]];
  if (/^\d+$/.test(text)) return parseInt(text, 10);
  return text;
}

function parseBenchOutput(lines: string[]): {
  timerPrecision: string | null;
  results: Result[];
} {
  let timerPrecision: string | null = null;
  let columns: string[] = [];
  const results: Result[] = [];
  // Names of the ancestors of the current row, indexed by depth.
  const stack: string[] = [];

  for (const line of lines) {
    const precision = line.match(/^Timer precision:\s*(.+)$/);
    if (precision) {
      timerPrecision = precision[1].trim();
      continue;
    }

    const segments = line.split("│");

    // Header row, eg. `pointers   fastest │ slowest │ median │ mean │ samples │ iters`
    if (columns.length === 0) {
      if (/\bfastest\s*$/.test(segments[0])) {
        columns = [
          "fastest",
          ...segments.slice(1).map((s) => s.trim()),
        ].filter(Boolean);
      }
      continue;
    }

    // Tree rows have a `├─ ` or `╰─ ` glyph, preceded by 3 characters per level of depth.
    const glyphIndex = line.search(/[├╰]─ /);
    if (glyphIndex < 0) continue;
    const depth = glyphIndex / 3;

    // Everything after the glyph, split into the name+fastest cell and remaining cells.
    const body = line.slice(glyphIndex + 3);
    const cells = body.split("│");
    const first = cells[0].trimEnd();
    const leaf = first.match(/^(.*?)\s{2,}(\S+(?: \S+)?)$/);
    const name = (leaf ? leaf[1] : first).trim();

    stack.length = depth;
    stack[depth] = name;

    const values = [leaf ? leaf[2] : "", ...cells.slice(1)];
    if (values.every((v) => v.trim() === "")) continue; // group row

    const result: Result = { path: stack.slice(0, depth + 1) };
    columns.forEach((column, i) => {
      result[column] = parseValue(values[i] ?? "");
    });
    results.push(result);
  }

  return { timerPrecision, results };
}

async function machineInfo() {
  const cpus = os.cpus();
  const rustc = (await $`rustc --version --verbose`.text()).trim();
  const divan = (await Bun.file("Cargo.lock").text()).match(
    /name = "divan"\nversion = "([^"]+)"/,
  )?.[1];
  return {
    os: `${os.type()} ${os.release()}`,
    platform: os.platform(),
    arch: os.arch(),
    cpu: {
      model: cpus[0]?.model ?? "unknown",
      logicalCores: cpus.length,
      speedMHz: cpus[0]?.speed ?? null,
    },
    memory: {
      totalBytes: os.totalmem(),
      totalGiB: Math.round((os.totalmem() / 1024 ** 3) * 100) / 100,
    },
    rustc,
    divan: divan ?? null,
  };
}

const filter = process.argv.slice(2);
const cargoArgs = ["cargo", "bench", "-p", PACKAGE];
if (filter.length > 0) cargoArgs.push("--", ...filter);

// Merge stderr into stdout so cargo's `Running benches/x.rs` lines stay in order with divan's.
const proc = Bun.spawn(["sh", "-c", 'exec "$@" 2>&1', "sh", ...cargoArgs], {
  stdout: "pipe",
  stdin: "inherit",
});

let output = "";
const decoder = new TextDecoder();
for await (const chunk of proc.stdout) {
  const text = decoder.decode(chunk, { stream: true });
  output += text;
  process.stdout.write(text);
}
const exitCode = await proc.exited;
if (exitCode !== 0) process.exit(exitCode);

// Split the combined output into one section per benchmark binary.
const sections = new Map<string, string[]>();
let current: string[] | null = null;
for (const line of output.split("\n")) {
  const running = line.match(/^\s*Running\s+(benches\/\S+?)\.rs\b/);
  if (running) {
    current = [];
    sections.set(running[1], current);
  } else current?.push(line);
}

if (sections.size === 0) {
  console.error("bench-to-json: no benchmark output found");
  process.exit(1);
}

const machine = await machineInfo();
const commit = (await $`git rev-parse HEAD`.nothrow().text()).trim() || null;
const generatedAt = new Date().toISOString();

for (const [source, lines] of sections) {
  const { timerPrecision, results } = parseBenchOutput(lines);
  if (results.length === 0) continue;
  const name = source.replace(/^benches\//, "");
  const file = `${PACKAGE_DIR}/${source}.bench.json`;
  const json = {
    benchmark: name,
    source: `${PACKAGE_DIR}/${source}.rs`,
    generatedAt,
    gitCommit: commit,
    filter: filter.length > 0 ? filter.join(" ") : null,
    timerPrecision,
    units: "Time values are in nanoseconds.",
    machine,
    results,
  };
  await Bun.write(file, JSON.stringify(json, null, 2) + "\n");
  console.log(`bench-to-json: wrote ${results.length} results to ${file}`);
}
