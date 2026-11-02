use anyhow::Result;
use clap::Args;

/// Runs the doup update process
#[derive(Args)]
pub struct UpdateArgs {
  /// Does not make any changes or create any PRs, only reports what needs to be updated
  #[arg(long)]
  dry_run: bool,
}

pub fn update(args: UpdateArgs) -> Result<()> {
  todo!()

  /*
    - Locate a doup.json|yaml file
    - If none exists return a helpful error, suggesting to call the init subcommand
    - Read the file & parse it into the typify types from schema.rs

    - Loop over each platform/ecosystem plugin and download them
      - Follow the wkg reference and download the plugin wasm module
        - Question: Does wkg have something akin to node_modules
          or do we need to save the wasm module into our own store?
        - Question: What about a lockfile?
          We probably need a doup.lock that sits alongside the doup config file.

    - Loop over each ecosystem plugin again and get a list of dependencies
      - Load the wasm module into wasmtime (basic example in plugin_host.rs)
      - Call the plugin: eg: `ecosystem_plugin.call_get_deps(&mut store, working_dir, plugin_config)`
        - Where:
          - store is the wasmtime Store
          - working_dir refers to a dir on the filesystem (probably a git repo we cloned at some point)
            where the plugin (via WASI) can read whatever files it needs to read to extract the list of deps. eg: package.json
          - plugin_config is the untyped bag of config properties from the doup config file for this plugin
      - The plugin then returns a list/map of: dependency name / current version

    - We should end up with a list of:
      - ecosystem plugin
      - dependency name
      - current version

    - In the future we may have some filtering / rate limiting / grouping here

    - Loop over this list and ask the plugin to get the next version & any release notes
      - `ecosystem_plugin.call_get_next_version(&mut store, working_dir, plugin_config, dependency_name, current_version)`
      - `ecosystem_plugin.call_get_release_notes(&mut store, working_dir, plugin_config, dependency_name, next_version)`

    - We make one more call to get the plugin to actually generate the necessary file changes to make the update
      - Our host creates a new git worktree
      - We pass that as the working_dir
      - And ask the plugin to apply the update
      - But keep in mind the wasm module has no way to run untrusted code.
        ie: no ability to fork processes so it must make these changes without
        using native tooling like say running `npm update foo`. It can only
        read/write files to the working dir & download HTTP artifacts in order
        to hash them etc.
      - `ecosystem_plugin.call_generate_update_diff(&mut store, git_tree_dir, plugin_config, dependency_name, current_version, next_version)`

    - Now our list should look like:
      - ecosystem plugin
      - dependency name
      - current version
      - next version
      - optional copy of the release notes text
      - optional link to the source of the release notes
      - git tree dir containing the actual changes needed for the update

    - In the future we may have some filtering / rate limiting / grouping here

    - Loop through our updates list and create PRs for each update using the platform plugin
      - Our host will probably create the new branch & commit as this would be the same across all platforms
        - Probably need some sort of PR body / commit msg / branch name templating config options
      - It's really only the act of creating a new PR that differs across VCS providers
        - `platform_plugin.call_publish_pr(&mut store, git_tree_dir, plugin_config, dependency_name, current_version, next_version, release_notes, release_notes_link)`
      - Question? Where do we handle rebases in the plugin or outside of it?
        Probably the host can handle the actual rebasing part as that's just standard git stuff
  */
}
