# galvanize-rs

## Classes

The `#[class]` attribute turns a struct into a shared handle, like an object reference in Python, Go or C#. Each field
gets a getter, a setter and an updater with the field's visibility. You write the constructor, using plain values:

```rust
use galvanize_rs::{List, Str, class, sf};

#[class]
pub struct Person {
  pub name: Str,     // pub fn name(), set_name(), update_name()
  cities: List<Str>, // private fn cities(), set_cities(), update_cities()
}

impl Person {
  pub fn new(first_name: impl Into<Str>, last_name: impl Into<Str>) -> Self {
    Self::from_data(PersonData {
      name: sf!("{} {}", first_name.into(), last_name.into()),
      cities: List::new(),
    })
  }

  pub fn visit(&self, city: impl Into<Str>) {
    self.cities().push(city);
  }
}
```

Each field lives in its own pointer, chosen by a `Strategy` (`ArcRwLockStrategy` by default). Pick another for the whole
class with `#[class(strategy = ArcSwapStrategy)]`, or for a single field by putting the same attribute on it. Mark a
public field `#[class(readonly)]` to keep its setter private.

See the `class` macro docs for everything that's generated, and [examples/class.rs](examples/class.rs) for a runnable
example (`cargo run -p galvanize-rs --example class`).
