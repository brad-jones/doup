mod model {
  use galvanize_rs::{Str, class};

  #[class]
  pub struct Foo {
    #[class(readonly)]
    pub id: Str,
    secret: Str,
  }

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
