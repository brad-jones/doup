use galvanize_rs::class;

#[derive(Clone, Debug)]
struct NotAClass;

#[class(extends = NotAClass)]
struct Foo {
  a: i64,
}

fn main() {}
