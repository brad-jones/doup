// Final classes can be declared in any module, and keep their private accessors and constructor private.
mod model {
  use galvanize_rs::{GString, class, class_methods};

  #[class]
  pub struct Foo {
    #[class_prop(readonly)]
    pub id: GString,
    secret: GString,
  }

  #[class_methods]
  impl Foo {
    pub fn new() -> Self {
      Self::from_data(FooData {
        id: "1".into(),
        secret: "x".into(),
      })
    }
  }
}

fn main() {
  let foo = model::Foo::new();
  let _ = foo.id();
  foo.set_id("2");
  foo.update_id(|_| ());
  foo.secret();
  let _ = model::Foo::from_data;
}
