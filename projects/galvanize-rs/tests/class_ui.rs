//! Compile-fail tests for the class macros. Regenerate the expected output with
//! `TRYBUILD=overwrite cargo test -p galvanize-rs --test class_ui`, then review it.

#[test]
fn class_compile_errors() {
  let t = trybuild::TestCases::new();
  t.compile_fail("tests/ui/class/*.rs");
  t.compile_fail("tests/ui/class/unsafe/*.rs");
}
