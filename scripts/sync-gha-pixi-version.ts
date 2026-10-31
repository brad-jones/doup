#!/usr/bin/env bun
import { existsSync } from "node:fs";
import { readdir } from "node:fs/promises";
import { join } from "node:path";
import { $ } from "bun";

const WORKFLOWS_DIR = ".github/workflows";
const DIFF_FILE = "diff.md";

// NB: Assumes this runs directly after `pixi self-update`.
const versionOutput = (await $`pixi --version`.text()).trim();
const match = versionOutput.match(/^pixi\s+(\d+\.\d+\.\d+)/);
if (!match) {
  throw new Error(`Could not parse pixi version from: ${versionOutput}`);
}
const versionTag = `v${match[1]}`;

let versionChanged = false;
if (existsSync(WORKFLOWS_DIR)) {
  for (const name of await readdir(WORKFLOWS_DIR)) {
    if (!/\.ya?ml$/.test(name)) continue;
    const path = join(WORKFLOWS_DIR, name);
    const original = await Bun.file(path).text();
    const updated = original.replace(
      /pixi-version:\s*v\d+\.\d+\.\d+/g,
      `pixi-version: ${versionTag}`,
    );
    if (updated !== original) {
      await Bun.write(path, updated);
      console.log(`updated pixi version in ${path} to ${versionTag}`);
      versionChanged = true;
    }
  }
}

// The `update` workflow generates diff.md for the PR body; note the Pixi bump there too.
if (versionChanged && existsSync(DIFF_FILE)) {
  const diff = await Bun.file(DIFF_FILE).text();
  await Bun.write(
    DIFF_FILE,
    `${diff}\n## Pixi\n\nUpdated Pixi to \`${versionTag}\`.\n`,
  );
  console.log(`updated ${DIFF_FILE} with Pixi self-update note`);
}
