use galvanize_rs::class;

#[class(unknown = 1)]
struct UnknownClassOption {
  a: String,
}

#[class]
struct UnknownFieldOption {
  #[class(writeonly)]
  pub a: String,
}

#[class]
struct ReadonlyPrivateField {
  #[class(readonly)]
  a: String,
}

#[class]
struct DuplicateFieldOption {
  #[class(readonly)]
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
