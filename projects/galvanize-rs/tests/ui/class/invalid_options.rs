use galvanize_rs::{class, class_methods};

#[class(unknown = 1)]
struct UnknownClassOption {
  a: String,
}

#[class(strategy = galvanize_rs::ArcSwapStrategy)]
struct RenamedStrategy {
  a: String,
}

#[class(runtime_strategy = FastRuntimeStrategy)]
struct UnknownRuntime {
  a: String,
}

#[class(open, open)]
struct DuplicateOption {
  a: String,
}

#[class]
struct UnknownFieldOption {
  #[class_prop(writeonly)]
  pub a: String,
}

#[class]
struct RenamedFieldStrategy {
  #[class_prop(strategy = galvanize_rs::ArcSwapStrategy)]
  pub a: String,
}

#[class]
struct ReadonlyPrivateField {
  #[class_prop(readonly)]
  a: String,
}

#[class]
struct DuplicateFieldOption {
  #[class_prop(readonly)]
  #[class_prop(readonly)]
  pub a: String,
}

#[class]
struct ClassOnField {
  #[class(readonly)]
  pub a: String,
}

#[class]
struct Tuple(String);

#[class]
struct Reserved {
  ptr_eq: String,
  __hidden: String,
}

#[class_methods(open)]
impl Reserved {}

fn main() {}
