use galvanize_rs::{ArcRwLockPointer, MutablePointer};

#[derive(Clone)]
struct Foo {
  id: ArcRwLockPointer<i32>,
}

impl Foo {
  fn new(bar: i32) -> Self {
    Self {
      id: ArcRwLockPointer::new(bar),
    }
  }

  fn get_id(&self) -> i32 {
    self.id.get()
  }

  fn add_to_id(&self, value: i32) {
    self.id.write_with(|v| *v = *v + value);
  }
}

#[derive(Clone)]
struct Bar {
  foo: ArcRwLockPointer<Foo>,
}

impl Bar {
  fn new(foo: &Foo) -> Self {
    Self {
      foo: ArcRwLockPointer::new(foo.clone()),
    }
  }

  fn change_foo(&self, value: &Foo) {
    self.foo.set(value)
  }
}

fn main() {
  let f = Foo::new(1);
  f.add_to_id(2);
  println!("ID: {}", f.get_id());

  let b = Bar::new(&f);
  println!("ID: {}", b.foo.read_with(|v| v.clone()).get_id());
  f.add_to_id(1);
  println!("ID: {}", b.foo.read_with(|v| v.clone()).get_id());
}
