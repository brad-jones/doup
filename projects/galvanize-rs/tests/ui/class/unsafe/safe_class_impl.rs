// The layout contract can't be implemented without `unsafe`.
use galvanize_rs::__private::unsafe_runtime::{Class, Handle};
use std::any::TypeId;

#[derive(Clone)]
struct Forged;

impl galvanize_rs::__private::Class for Forged {
  fn __identity(&self) -> *const () {
    std::ptr::null()
  }
}

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
