#!/usr/bin/env bun

import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { createComputeClient } from "@namespacelabs/sdk/api/compute";
import { $ } from "bun";
import { unlink, writeFile } from "node:fs/promises";

const client = createComputeClient();

const instance = await client.compute.createInstance({
  shape: {
    os: "linux",
    machineArch: "amd64",
    virtualCpu: 2,
    memoryMegabytes: 4 * 1024,
  },
  deadline: timestampFromDate(new Date(Date.now() + 30 * 60 * 1000)),
});

//const instance = await client.compute.createInstance({
//  shape: {
//    os: "macos",
//    machineArch: "arm64",
//    virtualCpu: 4,
//    memoryMegabytes: 7 * 1024,
//    selectors: [{ name: "macos.version", value: "26.x" }],
//  },
//  deadline: timestampFromDate(new Date(Date.now() + 30 * 60 * 1000)),
//});

// Namespace does not allow the Developer plan to start windows instances.
// const instance = await client.compute.createInstance({
//   shape: {
//     os: "windows",
//     machineArch: "amd64",
//     virtualCpu: 2,
//     memoryMegabytes: 4 * 1024,
//   },
//   deadline: timestampFromDate(new Date(Date.now() + 30 * 60 * 1000)),
// });

const instanceId = instance.metadata!.instanceId;
console.log(`Created instance: ${instanceId}`);

try {
  await client.compute.waitInstanceSync(
    { instanceId },
    { timeoutMs: 10 * 60 * 1000 },
  );

  console.log(`Instance is running`);

  const { sshPrivateKey, endpoint, username } = await client.compute
    .getSSHConfig(
      { instanceId },
    );

  const keyPath = `${process.env["HOME"]}/.ssh/keys/${instanceId}`;
  try {
    await writeFile(keyPath, sshPrivateKey);
    await $`chmod 0600 ${keyPath}`;
    await $`/usr/bin/ssh ${username}@${endpoint} -i ${keyPath} "cat /etc/os-release"`;
    //await $`/usr/bin/ssh ${username}@${endpoint} -i ${keyPath} "sw_vers"`;
    //await $`/usr/bin/ssh ${username}@${endpoint} -i ${keyPath} "cmd /c ver"`;
  } finally {
    await unlink(keyPath);
  }
} finally {
  await client.compute.destroyInstance({ instanceId });
  console.log(`Destroyed instance: ${instanceId}`);
}
