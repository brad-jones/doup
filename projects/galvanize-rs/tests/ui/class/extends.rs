use galvanize_rs::{class, class_methods};
use galvanize_rs_fixtures_safe::{Animal, Dog, Puppy};

#[class(extends = Animal, extends = Animal)]
struct Duplicate {
  b: i64,
}

#[class(extends = Self)]
struct OwnParent {
  b: i64,
}

#[class(extends = T)]
struct ParamParent<T> {
  b: T,
}

#[class(extends = Animal)]
struct ReservedBase {
  base: i64,
}

#[class_methods]
impl ReservedBase {}

#[class(extends = Dog, runtime_strategy = galvanize_rs::UnsafeRuntimeStrategy)]
struct OtherRuntime {}

#[class_methods]
impl OtherRuntime {}

#[class(extends = Puppy)]
struct FinalParent {}

#[class_methods]
impl FinalParent {}

#[class(extends = galvanize_rs_fixtures_safe::Boxed)]
struct MissingArguments {}

#[class_methods]
impl MissingArguments {}

fn main() {}
