#!/usr/bin/env bun

import { $ } from "bun";
import { chmod } from "node:fs/promises";

const lines = (await $`git ls-files --stage`.text()).split("\n");

await Promise.all(
  lines
    .filter((line) => line.includes("scripts/") || line.includes(".lefthook/"))
    .filter((line) => line.startsWith("100644"))
    .map((line) => line.match(/\d+\s[a-z0-9]+\s\d\s+(.*)/)?.at(1))
    .filter((file): file is string => typeof file === "string")
    .map((file) =>
      process.platform === "win32"
        ? $`git update-index --add --chmod=+x ${file}`
        : chmod(file, 0o755)
    ),
);
