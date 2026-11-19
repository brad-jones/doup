use galvanize_rs::class;

#[class]
pub struct Base {
  a: i64,
}

#[class(extends = Base, extends = Base)]
struct Duplicate {
  b: i64,
}

#[class(extends = Self)]
struct OwnBase {
  b: i64,
}

#[class(extends = T)]
struct ParamBase<T> {
  b: T,
}

#[class(extends = Base)]
struct ReservedBase {
  base: i64,
}

#[class]
struct ReservedUpcast {
  upcast: i64,
}

fn main() {}
