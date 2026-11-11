fn main() {
  // Lets the SSH integration tests show as ignored, rather than silently pass, without a test remote.
  println!("cargo::rustc-check-cfg=cfg(doup_test_ssh)");
  println!("cargo::rerun-if-env-changed=DOUP_TEST_SSH_URL");
  println!("cargo::rerun-if-env-changed=DOUP_TEST_SSH_KEY");
  let set = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
  if set("DOUP_TEST_SSH_URL") && set("DOUP_TEST_SSH_KEY") {
    println!("cargo::rustc-cfg=doup_test_ssh");
  }
}
