#!/usr/bin/env bun

import { $ } from "bun";
import { unlink } from "node:fs/promises";
import randomName from "@scaleway/random-name";
import { z } from "zod";

const name = randomName();
const keyPath = `${process.env["HOME"]}/.ssh/keys/${name}`;
await $`ssh-keygen -t ed25519 -N "" -f ${keyPath}`;

try {
  await $`chmod 0600 ${keyPath}`;
  const { endpoints: { ssh: { username, external_ip, external_port } } } = z
    .object({
      endpoints: z.object({
        ssh: z.object({
          external_ip: z.ipv4(),
          external_port: z.number(),
          username: z.string(),
        }),
      }),
    }).parse(
      //await $`avr vm create --name ${name} --os linux --arch x86_64 --size 4-vcpu --ssh-key @${keyPath}.pub --ttl 8h --wait --json`.json(),
      await $`avr vm create --name ${name} --os windows --arch x86_64 --size 4-vcpu --ssh-key @${keyPath}.pub --ttl 8h --wait --json`
        .json(),
    );

  try {
    //await $`/usr/bin/ssh -o StrictHostKeyChecking=accept-new -p ${external_port} ${username}@${external_ip} -i ${keyPath} "cat /etc/os-release"`;

    // avrea does not support macos
    //await $`/usr/bin/ssh -o StrictHostKeyChecking=accept-new -p ${external_port} ${username}@${external_ip} -i ${keyPath} "sw_vers"`;

    await $`/usr/bin/ssh -o StrictHostKeyChecking=accept-new -p ${external_port} ${username}@${external_ip} -i ${keyPath} ${`powershell -NoProfile -Command "Get-CimInstance Win32_OperatingSystem | Format-List Caption,Version,BuildNumber"`}`;
  } finally {
    await $`avr vm delete -y ${name} --wait`;
  }
} finally {
  await unlink(keyPath);
  await unlink(`${keyPath}.pub`);
}
