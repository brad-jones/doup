#!/usr/bin/env bun
// Publishes every built doup plugin (see `task build:plugins`) to an OCI registry as `<registry>/<name>:<version>`
// where the name is the crate name without its `doup-` prefix and the version is the crate's version.
//
// We use `wkg oci push` rather than `wkg publish` because the latter can't attach annotations to a wasm file built by
// cargo, which is what links the package to this repo on GitHub, and it would also require `--package` per plugin.
//
// Versions are immutable: a plugin whose version already exists in the registry is skipped, bump the crate version.
//
// Usage: wkg-publish-plugins.ts [--dry-run]
//
// Env:
//   GITHUB_TOKEN        token with the `write:packages` scope, defaults to `gh auth token -u brad-jones`
//   GITHUB_ACTOR        username to authenticate as, defaults to brad-jones
//   DOUP_PLUGIN_REGISTRY where to publish to, defaults to ghcr.io/brad-jones/doup
import { Glob } from "bun";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "..");
const dryRun = process.argv.includes("--dry-run");
const registry = process.env.DOUP_PLUGIN_REGISTRY || "ghcr.io/brad-jones/doup";
const actor = process.env.GITHUB_ACTOR || "brad-jones";
const token = process.env.GITHUB_TOKEN || (await Bun.$`gh auth token -u ${actor}`.text()).trim();

const plugins: { name: string; version: string; wasm: string; description?: string }[] = [];
for await (const manifest of new Glob("projects/plugins/*/*/Cargo.toml").scan({ cwd: root })) {
  const { package: pkg } = Bun.TOML.parse(await Bun.file(join(root, manifest)).text()) as {
    package: { name: string; version: string; description?: string };
  };
  plugins.push({
    name: pkg.name.replace(/^doup-/, ""),
    version: pkg.version,
    description: pkg.description,
    wasm: join(root, "target/wasm32-wasip3/release", `${pkg.name.replaceAll("-", "_")}.wasm`),
  });
}

if (plugins.length === 0) throw new Error("no plugins found");

// ghcr.io accepts a base64 encoded token as a bearer token, so we can probe for an existing tag without a token exchange.
async function exists(name: string, version: string) {
  const [host, ...path] = registry.split("/");
  const res = await fetch(`https://${host}/v2/${path.join("/")}/${name}/manifests/${version}`, {
    method: "HEAD",
    headers: {
      Authorization: `Bearer ${Buffer.from(token).toString("base64")}`,
      Accept: "application/vnd.oci.image.manifest.v1+json",
    },
  });
  if (res.status === 404) return false;
  if (res.ok) return true;
  throw new Error(`unexpected ${res.status} checking for ${name}:${version}`);
}

for (const { name, version, wasm, description } of plugins) {
  const reference = `${registry}/${name}:${version}`;
  if (!(await Bun.file(wasm).exists())) throw new Error(`${wasm} does not exist, run \`task build:plugins\` first`);
  if (await exists(name, version)) {
    console.log(`${reference} already exists, skipping`);
    continue;
  }
  console.log(`${dryRun ? "[dry-run] " : ""}${wasm} -> ${reference}`);
  if (dryRun) continue;

  const annotations = [
    ["org.opencontainers.image.source", "https://github.com/brad-jones/doup"],
    ["org.opencontainers.image.url", "https://github.com/brad-jones/doup"],
    ["org.opencontainers.image.licenses", "MIT"],
    ["org.opencontainers.image.version", version],
    ["org.opencontainers.image.description", description ?? `The doup ${name}`],
  ].flatMap(([key, value]) => ["--annotation", `${key}=${value}`]);

  const proc = Bun.spawn(["wkg", "oci", "push", ...annotations, reference, wasm], {
    stdio: ["inherit", "inherit", "inherit"],
    env: { ...process.env, WKG_OCI_USERNAME: actor, WKG_OCI_PASSWORD: token },
  });
  if ((await proc.exited) !== 0) process.exit(1);
}
