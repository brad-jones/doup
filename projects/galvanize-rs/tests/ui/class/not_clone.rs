use galvanize_rs::class;

#[derive(Debug)]
struct NotClone;

#[class]
struct Foo {
  a: NotClone,
}

fn main() {}
