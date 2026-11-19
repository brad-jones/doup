use galvanize_rs::class;

#[class(unknown = 1)]
struct UnknownClassOption {
  a: String,
}

#[class]
struct UnknownFieldOption {
  #[class_prop(writeonly)]
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
struct Generic<T> {
  a: T,
}

#[class]
struct Tuple(String);

#[class]
struct Reserved {
  ptr_eq: String,
}

fn main() {}
