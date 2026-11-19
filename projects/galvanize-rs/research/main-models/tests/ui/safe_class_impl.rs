use galvanize_rs_main_runtime::{Class, Handle};
use std::any::TypeId;

#[derive(Clone)]
struct Forged;
impl Class for Forged {
  type Fields = ();
  type VTable = ();
  fn handle(&self) -> &Handle<Self> {
    panic!("no actual carrier")
  }
  fn from_handle(_: Handle<Self>) -> Self {
    Forged
  }
  fn is_class(_: TypeId) -> bool {
    true
  }
  fn build_vtable() {}
}
fn main() {}
