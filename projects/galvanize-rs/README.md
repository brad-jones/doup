# galvanize-rs

## Classes

The `#[class]` attribute turns a struct into a shared handle, like an object reference in Python, Go or C#. Each field
gets a getter, a setter and an updater with the field's visibility. Every class is followed by one `#[class_methods]`
block (which may be empty), where you write its constructor using plain values:

```rust
use galvanize_rs::{List, Str, class, class_methods, sf};

#[class]
pub struct Person {
  pub name: Str,     // pub fn name(), set_name(), update_name()
  cities: List<Str>, // private fn cities(), set_cities(), update_cities()
}

#[class_methods]
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

Each field lives in its own pointer, chosen by a `PointerStrategy` (`ArcRwLockStrategy` by default). Pick another for
the whole class with `#[class(pointer_strategy = ArcSwapStrategy)]`, or for a single field with
`#[class_prop(pointer_strategy = ...)]`. Mark a public field `#[class_prop(readonly)]` to keep its setter private.

Classes can be generic, e.g. `struct Node<T> { value: T, next: Option<Self> }`. Type parameters are bounded with
`Clone + Send + Sync + 'static` for you, like `#[derive]` does with `Clone`.

### Inheritance

A class declared `#[class(open)]` can be extended with `#[class(extends = Parent)]`, naming only the immediate parent.
Mark methods `#[class_virtual]` to let subclasses override them with `#[class_override]`, and call the parent's version
with `super_x()`. Calls are late-bound, like in Python, Java or C#:

```rust
#[class(open)]
pub struct Animal {
  pub name: Str,
}

#[class_methods]
impl Animal {
  pub fn init(name: &str) -> AnimalData {
    AnimalData { name: name.into() }
  }

  #[class_virtual]
  pub fn speak(&self) -> Str {
    "...".into()
  }

  pub fn describe(&self) -> Str {
    sf!("{} says {}", self.name(), self.speak())
  }
}

#[class(extends = Animal)]
pub struct Dog {
  pub good: bool,
}

#[class_methods]
impl Dog {
  pub fn new(name: &str) -> Self {
    Self::from_data(DogData { base: Animal::init(name), good: true })
  }

  #[class_override]
  pub fn speak(&self) -> Str {
    sf!("Woof (not {})", self.super_speak())
  }
}

let animal: Animal = Dog::new("Rex").upcast();
assert_eq!(animal.describe(), "Rex says Woof (not ...)");
assert!(animal.downcast::<Dog>().unwrap().good());
```

A subclass dereferences to its parent, so the parent's accessors and methods work on it, and `upcast`, `is` and
`downcast` convert between the classes of a hierarchy. Virtual methods can be `async`.

An `open` class must be declared at the root of its crate (final classes can live anywhere). Classes get single
inheritance; for multiple inheritance, implement traits as interfaces and mixins, as
[examples/traits.rs](examples/traits.rs) shows (`cargo run -p galvanize-rs --example traits`).

### Runtime strategies

How a hierarchy is implemented is chosen by its root, with `#[class(runtime_strategy = ...)]`. Both strategies generate
the same API, so switching is a one-line change:

- **`SafeRuntimeStrategy`** (the default) generates no `unsafe` code. Its handles grow with the depth of the hierarchy,
  and calling an override clones an `Arc`, which contends when many threads share an object.
- **`UnsafeRuntimeStrategy`** uses a small, Miri-tested `unsafe` kernel with C++ style layouts and vtables. Its handles
  are four words at any depth, and virtual calls take about a nanosecond without contending.

[benches/inheritance.rs](benches/inheritance.rs) compares them (`task bench -- inheritance`), and
[the research behind them](../../docs/research/galvanize-rs-true-inheritance/README.md) explains the trade-offs.

See the `class` macro docs for everything that's generated and its limitations, and the runnable examples:
[examples/class.rs](examples/class.rs) (`cargo run -p galvanize-rs --example class`),
[examples/generic_class.rs](examples/generic_class.rs) (`--example generic_class`) and
[examples/inheritance.rs](examples/inheritance.rs) (`--example inheritance`).
