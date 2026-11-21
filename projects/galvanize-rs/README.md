# galvanize-rs

## Classes

The `#[class]` attribute turns a struct into a shared handle, like an object reference in Python, Go or C#. Each field
gets a getter, a setter and an updater with the field's visibility. Every class is followed by one `#[class_methods]`
block (which may be empty), where you write its constructor using plain values:

```rust
use galvanize_rs::{GList, GString, class, class_methods, sf};

#[class]
pub struct Person {
  pub name: GString,      // pub fn name(), set_name(), update_name()
  cities: GList<GString>, // private fn cities(), set_cities(), update_cities()
}

#[class_methods]
impl Person {
  pub fn new(first_name: impl Into<GString>, last_name: impl Into<GString>) -> Self {
    Self::from_data(PersonData {
      name: sf!("{} {}", first_name.into(), last_name.into()),
      cities: GList::new(),
    })
  }

  pub fn visit(&self, city: impl Into<GString>) {
    self.cities().push(city);
  }
}
```

Each field lives in its own pointer, chosen by a `PointerStrategy` (`ArcRwLockStrategy` by default). Pick another for
the whole class with `#[class(pointer_strategy = ArcSwapStrategy)]`, or for a single field with
`#[class_prop(pointer_strategy = ...)]`. Mark a public field `#[class_prop(readonly)]` to keep its setter private.

Like in Java or C#, a concrete class that doesn't declare a constructor (a `new`, or any associated function returning
`Self`) gets a default one: `new()`, plus a `Default` impl, which set every field, including inherited ones, to its
`Default::default()`. It's only callable when all those fields implement `Default`.

```rust
#[class]
pub struct Settings {
  pub verbose: bool,
  pub tags: GList<GString>,
}

#[class_methods]
impl Settings {}

let settings = Settings::new(); // verbose: false, tags: []
```

Classes can be generic, e.g. `struct Node<T> { value: T, next: Option<Self> }`. Type parameters are bounded with
`Clone + Send + Sync + 'static` for you, like `#[derive]` does with `Clone`.

### Inheritance

A class declared `#[class(open)]` can be extended with `#[class(extends = Parent)]`, naming only the immediate parent.
Mark methods `#[class_virtual]` to let subclasses override them with `#[class_override]` (with the same signature and
visibility), and call the parent's version with `super_x()`. Calls are late-bound, like in Python, Java or C#:

```rust
#[class(open)]
pub struct Animal {
  pub name: GString,
}

#[class_methods]
impl Animal {
  pub fn init(name: &str) -> AnimalData {
    AnimalData { name: name.into() }
  }

  #[class_virtual]
  pub fn speak(&self) -> GString {
    "...".into()
  }

  pub fn describe(&self) -> GString {
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
  pub fn speak(&self) -> GString {
    sf!("Woof (not {})", self.super_speak())
  }
}

let animal: Animal = Dog::new("Rex").upcast();
assert_eq!(animal.describe(), "Rex says Woof (not ...)");
assert!(animal.downcast::<Dog>().unwrap().good());
```

A subclass dereferences to its parent, so the parent's accessors and methods work on it, and `upcast`, `is` and
`downcast` convert between the classes of a hierarchy. Virtual methods can be `async`.

A `#[class(abstract)]` class can't be instantiated, and may declare `#[class_abstract] fn x(&self);` methods without a
body, which every concrete subclass must implement (checked at compile time). As in Scala, implementing an abstract
method doesn't need `#[class_override]`, but replacing an implementation does, so a method can never silently shadow an
inherited virtual one. With `implements = Trait`, and `#[class_interface]` on the trait, the class's methods named after
the trait's methods implement `Trait`, whether they're ordinary, virtual or abstract:

```rust
#[class_interface]
pub trait Shape {
  fn area(&self) -> f64;
}

#[class(abstract, implements = Shape)]
pub struct Polygon {}

#[class_methods]
impl Polygon {
  #[class_abstract]
  fn area(&self) -> f64;

  #[class_abstract]
  fn corners(&self) -> u64; // not part of `Shape`, which doesn't declare it
}

#[class(extends = Polygon)]
pub struct Square {
  pub side: f64,
}

#[class_methods]
impl Square {
  fn area(&self) -> f64 {
    self.side() * self.side()
  }

  fn corners(&self) -> u64 {
    4
  }
}

let square = Square::new(); // a default constructor, as `Square` declares none
square.set_side(2.0);
```

A proc macro can't see the methods of a trait without `#[class_interface]`, such as one from another crate, so the
class's virtual and abstract methods declared without a visibility, like those of a trait impl, implement it instead.

An `open` class must be declared at the root of its crate (final classes can live anywhere). Classes get single
inheritance; for multiple inheritance, implement traits as interfaces and mixins, as
[examples/traits.rs](examples/traits.rs) shows (`cargo run -p galvanize-rs --example traits`).

### Class-level members

Associated functions (without `self`) and constants take the same markers, and are late-bound like Python's `cls` or
PHP's `static::`: in their bodies, `Self` is the class they're accessed through. Subclasses inherit them as their own
associated items, and an override calls its parent's implementation with `Self::super_x()`:

```rust
#[class(abstract)]
pub struct ConfigFile {}

#[class_methods]
impl ConfigFile {
  #[class_abstract]
  pub const NAMES: &'static [&'static str];

  #[class_virtual]
  pub fn locate(dir: &str) -> String {
    format!("{dir}/{}", Self::NAMES[0])
  }
}

#[class(extends = ConfigFile)]
pub struct BotConfigFile {}

#[class_methods]
impl BotConfigFile {
  pub const NAMES: &'static [&'static str] = &["bot.yaml"]; // implements the abstract constant
}

assert_eq!(BotConfigFile::locate("/etc"), "/etc/bot.yaml");
```

Rust only binds `Self` late in a trait's default methods, so these bodies are generic over the class, and can only reach
other class-level members through `Self` (a final class's overrides, which nothing inherits, are the exception). A
`#[class_interface]` trait may declare class-level members too: an open class implementing one with them implements it
for all its subclasses, so they satisfy `T: Trait`. Subclasses repeat the signatures they inherit, so the types those
name must also resolve where the subclass is declared.

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
