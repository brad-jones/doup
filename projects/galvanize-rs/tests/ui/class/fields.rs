use galvanize_rs::{class, class_methods};
use std::rc::Rc;

#[derive(Debug)]
struct NotClone;

#[class]
struct Uncloneable {
  a: NotClone,
}

#[class_methods]
impl Uncloneable {}

#[class(runtime_strategy = galvanize_rs::SafeRuntimeStrategy)]
struct SafeNotSend {
  a: Rc<()>,
}

#[class_methods]
impl SafeNotSend {}

#[class(runtime_strategy = galvanize_rs::UnsafeRuntimeStrategy)]
struct UnsafeNotSend {
  a: Rc<()>,
}

#[class_methods]
impl UnsafeNotSend {}

fn main() {}
