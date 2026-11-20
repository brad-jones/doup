use galvanize_rs::{class, class_methods};

#[class]
struct UnusedParam<T> {
  a: String,
}

#[class]
struct OnlyUsedRecursively<T> {
  next: Option<Self>,
}

#[class]
struct Lifetime<'a> {
  a: &'a str,
}

#[class]
struct Const<const N: usize> {
  a: [u8; N],
}

#[class]
struct BoundsOnMethods<T> {
  a: T,
}

#[class_methods]
impl<T: Default> BoundsOnMethods<T> {}

fn main() {}
