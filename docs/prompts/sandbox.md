# Sandbox for cross platform development & testing

I have signed up to Avrea & Namespace both on the cheapest pay as you go offering.

I have been using their runners in our GitHub Actions pipelines, I would now like to explore using their Sandbox
functionality for local development and testing.

- see: <https://avrea.com/llms.txt> / <https://docs.avrea.com/llms.txt>
- also: <https://namespace.so/llms.txt> / <https://namespace.so/docs/llms.txt>

## Problem: Running integration tests on operating systems other than the host OS

For example I'd like to be able to run `TARGET=x86_64-pc-windows-msvc task test:integration` from my Linux host, without
having to set up a full Windows environment locally. The same for MacOS as well.

## Solution: Spin up a new Sandbox VM

When the TARGET can not be run natively on the host OS, we should spin up a new Sandbox VM with the desired OS and
architecture.

Then rsync our local project files to the Sandbox VM. Then we can SSH into the Sandbox VM and run our integration tests
as if we were on the target OS.

### Proof of Concept

I have developed 2 PoC scripts:

- `./scripts/avrea-sandbox.ts`
- `./scripts/namespace-instances.ts`

What I have found is that Avrea only offers Linux & Windows Sandboxes, where as Namespace only offers Linux and MacOS
Sandboxes. At least on the plans I am on currently.

_NB: Avrea has a generous free tier for their Sandboxes, so we should use Avrea for Linux & Windows, and Namespace for
MacOS._

### Final Solution Design

I am thinking that we create a new script `./scripts/sandbox.ts` that abstracts over Avrea and Namespace Sandboxes,
allowing us to spin up the appropriate Sandbox VM based on the rust target triple.

This script would handle:

- Determining which provider to use based on the rust target triple.
- Creating the Sandbox VM with that provider.
  - Make sure to include a TTL so that the Sandbox VM is automatically destroyed after a certain period of time. I think
    1 hour should be sufficient.
- Syncing local project files to the Sandbox VM.
  - We should only sync the actual source code, all dependencies should be downloaded directly on the Sandbox VM as that
    will be a lot faster than relying on our upload link.
    - This should be relatively easy, just download & install pixi, then run `pixi install` on the Sandbox VM to fetch
      all dependencies, along with our `task init` task.
- After initial setup, providing an interface to run commands on the Sandbox VM, such as running integration tests.
- And of course final cleanup of the Sandbox VM once we are done with it.

I'm thinking the sandbox.ts script would be split into a few different subcommands.

- `create`: Create a new Sandbox VM based on the rust target triple.
- `sync`: Sync local project files to a Sandbox VM (or all running Sandbox VMs).
  - I think we should explore rsync initially
  - It should be a one way sync from the local machine to the Sandbox VM.
  - We would never sync files from the Sandbox VM back to the local machine.
  - The idea is that we do our work locally and then push the changes to the Sandbox VM for testing and execution.
- `run`: Run commands on a Sandbox VM (or all running Sandbox VMs), such as integration tests.
- `delete`: Destroy the Sandbox VM once we are done with it.

We should use <https://cliffy.io/llms.txt> for building the CLI interface for our sandbox.ts script.

We should create a new `test:integration:sandbox` task that integrates with the sandbox.ts script, ensuring that the
appropriate Sandbox VMs are created (based on the rust target triples - ie: similar to the ci.yaml matrix build), synced
with local project files, and used to run the integration tests. After the tests are complete, the Sandbox VMs should be
deleted to clean up resources.
