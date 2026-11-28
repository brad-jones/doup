use assert_cmd::prelude::*;
//use assert_fs::prelude::*;
use anyhow::Result;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn test_cli_help_flag() -> Result<()> {
  let mut cmd = Command::cargo_bin("doup")?;

  cmd.arg("--help");

  cmd.assert().success().stdout(predicate::str::contains("Usage: doup"));

  Ok(())
}

#[test]
fn test_cli_lock_reports_what_it_locked() -> Result<()> {
  use assert_fs::prelude::*;

  let repo = assert_fs::TempDir::new()?;
  repo.child(".git").create_dir_all()?;
  Command::cargo_bin("doup")?
    .args(["lock", "--dir"])
    .arg(repo.path())
    .assert()
    .failure()
    .stderr(predicate::str::contains("no doup config file found"));

  repo.child("doup.yaml").write_str("platform: { use: ./platform.wasm }\necosystems: [{ use: ./eco.wasm }]\n")?;
  Command::cargo_bin("doup")?
    .args(["lock", "--dir"])
    .arg(repo.path())
    .assert()
    .success()
    .stdout(predicate::str::contains("no package plugins to lock"));
  repo.child("doup.lock").assert(predicate::path::missing());

  Ok(())
}

/*
#[test]
fn test_cli_missing_argument_fails() -> Result<()> {
  let mut cmd = Command::cargo_bin("my_cli")?;

  // Invoke without arguments when one is required
  cmd
    .assert()
    .failure() // Verifies exit code is non-zero
    .code(2) // Optional: verify specific exit code (e.g., Clap's missing arg code)
    .stderr(predicate::str::contains("error: a value is required"));

  Ok(())
}

#[test]
fn test_cli_file_processing() -> Result<()> {
  // Create a temporary sandbox directory for file mutations
  let temp_dir = assert_fs::TempDir::new()?;
  let input_file = temp_dir.child("input.txt");
  input_file.write_str("Hello, E2E Testing!")?;

  let mut cmd = Command::cargo_bin("my_cli")?;
  cmd.arg("-i").arg(input_file.path());

  // Execute the command
  cmd.assert().success();

  // Verify side effects (e.g., if your CLI modifies the file or generates another)
  input_file.assert(predicate::str::contains("Processed: Hello, E2E Testing!"));

  Ok(())
}
*/
