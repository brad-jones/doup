#[test]
fn class_compile_errors() {
  trybuild::TestCases::new().compile_fail("tests/ui/class/*.rs");
}
