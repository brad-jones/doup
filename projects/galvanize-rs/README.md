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
class with `#[class(strategy = ArcSwapStrategy)]`, or for a single field with `#[class_prop(strategy = ...)]`. Mark a
public field `#[class_prop(readonly)]` to keep its setter private.

Classes can be generic, e.g. `struct Node<T> { value: T, next: Option<Self> }`. Type parameters used by fields are
bounded with `Clone` for you, like `#[derive]` does.

A class can extend another with `#[class(extends = Base)]`. The subclass holds a `Base` object, dereferences to it so
`Base`'s accessors and methods work on the subclass, and can be converted back with `base()` or `upcast()`:

```rust
#[class(extends = Person)]
pub struct Employee {
  pub title: Str,
}

impl Employee {
  pub fn new(first_name: impl Into<Str>, last_name: impl Into<Str>, title: impl Into<Str>) -> Self {
    Self::from_data(EmployeeData {
      base: Person::new(first_name, last_name),
      title: title.into(),
    })
  }
}

let ada = Employee::new("Ada", "Lovelace", "Engineer");
ada.visit("London"); // inherited from Person
let person: Person = ada.upcast();
```

Same-named methods on a subclass shadow the base's rather than overriding them, so use traits for dynamic dispatch.
[examples/traits.rs](examples/traits.rs) shows how (`cargo run -p galvanize-rs --example traits`).

See the `class` macro docs for everything that's generated, and [examples/class.rs](examples/class.rs) for a runnable
example (`cargo run -p galvanize-rs --example class`). [examples/generic_class.rs](examples/generic_class.rs) shows
generic classes (`cargo run -p galvanize-rs --example generic_class`), and
[examples/inheritance.rs](examples/inheritance.rs) shows inheritance (`cargo run -p galvanize-rs --example
inheritance`).
