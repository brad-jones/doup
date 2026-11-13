#!/usr/bin/env bun
import { $ } from "bun";

const TOOLCHAIN_FILE = "rust-toolchain.toml";
const DIFF_FILE = "diff.md";

// NB: Assumes this runs directly after `rustup update`, which updates the `stable` toolchain
// but never touches rust-toolchain.toml. We pin the channel to an exact version so the update
// shows up as a diff in the repo.
const versionOutput = (await $`rustup run stable rustc --version`.text())
  .trim();
const match = versionOutput.match(/^rustc\s+(\d+\.\d+\.\d+)/);
if (!match) {
  throw new Error(`Could not parse rustc version from: ${versionOutput}`);
}
const version = match[1];

const original = await Bun.file(TOOLCHAIN_FILE).text();
const updated = original.replace(
  /^channel\s*=\s*"[^"]*"/m,
  `channel = "${version}"`,
);
if (updated === original) {
  console.log(`rust toolchain already at ${version}`);
  process.exit(0);
}

await Bun.write(TOOLCHAIN_FILE, updated);
console.log(`updated rust toolchain in ${TOOLCHAIN_FILE} to ${version}`);

// The `update` workflow generates diff.md for the PR body; note the Rust bump there too.
if (await Bun.file(DIFF_FILE).exists()) {
  const diff = await Bun.file(DIFF_FILE).text();
  await Bun.write(
    DIFF_FILE,
    `${diff}\n## Rust\n\nUpdated Rust to \`${version}\`.\n`,
  );
  console.log(`updated ${DIFF_FILE} with Rust toolchain note`);
}
