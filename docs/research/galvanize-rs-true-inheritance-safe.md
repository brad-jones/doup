# True Inheritance for galvanize-rs Classes: A Fully Safe Variant

Status: **Research**. Nothing here is scheduled.

An executable [descriptor-based variant](galvanize-rs-true-inheritance-descriptors.md) now demonstrates this safe
runtime with separate class/method attributes, immediate-parent-only declarations, and `#[class_override]` markers
without declaring-class annotations. It is included as a third implementation in the benchmarks.

> **Read [galvanize-rs-true-inheritance.md](galvanize-rs-true-inheritance.md) first.** This document is a companion to
> it. It reuses that document's terminology (Option A, Option B, open classes, `#[class_methods]`, `init`) and only
> describes what changes. Section numbers like "section 4.4" refer to the main document.

A hand-expanded prototype of this variant lives in
[galvanize-rs-true-inheritance-safe-spike.rs](galvanize-rs-true-inheritance-safe-spike.rs). It's standalone, compiles on
our pinned stable toolchain with `#![forbid(unsafe_code)]`, and runs.

Both designs are benchmarked against each other in
[benches/inheritance.rs](../../projects/galvanize-rs/benches/inheritance.rs), on bench-owned forks of the two spikes
that follow the documents where the spikes took shortcuts. Section 3.1 has the results.
[galvanize-rs-true-inheritance-comparison.md](galvanize-rs-true-inheritance-comparison.md) compares both designs at a
high level, against the languages galvanize-rs mimics, with a recommendation.

## TL;DR

The main document's design needs a small amount of `unsafe` because of its **memory layout**:

- it reinterprets a `Puppy` handle's bits as a `Dog` or `Animal` handle;
- it reads ancestor fields through a prefix pointer;
- it implements `Send`/`Sync` by hand for raw pointers.

The design here needs none. It combines two ideas the main document listed as alternatives:

- **The user lists every ancestor** (main document, section 6.5): `#[class(extends = Dog, Animal)]`.
- **One object trait per class** (main document, section 6.2): `AnimalObject`, then `DogObject: AnimalObject`, and so
  on. Each is implemented by every descendant's storage and used as `Arc<dyn …>`.

On its own, 6.2 fails because a subclass's macro can't see the ancestors. Listing them fixes that. The prototype shows
the same behaviour as the main design: late binding, overrides at every level, `super` calls, one identity, `downcast`,
`&Puppy → &Animal` coercion, and threads.

The price is mostly paid by the user and in runtime cost:

- overrides must name the class that declared the method (`#[override(Animal)]`);
- the ancestor list is repeated on every subclass;
- handles grow with hierarchy depth, and so does clone cost;
- every call to an override builds a handle of the overriding class for its body (a checked downcast). That costs about
  25 ns, against about 1 ns for the main design's overrides (section 3.1).

In return there's no `unsafe`, no Miri requirement, and **no vtable registry for generic classes**. Construction is a
little faster for generic classes as a result.

---

## 1. Surface syntax

Compared with section 4.1 of the main document, only two things change:

```rust
#[class(open)]
pub struct Animal {
  pub name: Str,
}

#[class_methods]
impl Animal {
  #[virtual]
  pub fn speak(&self) -> Str {
    "...".into()
  }
}

#[class(open, extends = Animal)]
pub struct Dog {
  pub tricks: List<Str>,
}

#[class_methods]
impl Dog {
  #[override(Animal)] // NEW: names the class that declared `speak`
  pub fn speak(&self) -> Str {
    "Woof".into()
  }

  #[virtual]
  pub fn fetch(&self) -> Str {
    sf!("{} fetches", self.name())
  }
}

#[class(extends = Dog, Animal)] // NEW: every ancestor, nearest first
pub struct Puppy {}

#[class_methods]
impl Puppy {
  #[override(Animal)]
  pub fn speak(&self) -> Str {
    sf!("{} (but squeakier)", self.super_speak())
  }

  #[override(Dog)]
  pub fn fetch(&self) -> Str {
    sf!("{} clumsily", self.super_fetch())
  }
}
```

Everything else stays as in the main document: `open`, `init` and nested `XData` construction, `upcast`, `is`,
`downcast`, and cross-type equality.

## 2. How it works

### 2.1 One trait per open class

Each open class's macro generates a hidden, object-safe trait with:

- a `__parent()` method, returning the parent's storage as that trait, or `None` at the declaring class;
- a fields accessor for that class's own fields;
- one method per virtual method the class declares, taking the handle (`this`) to keep late binding. Its default body
  forwards to `__parent()`, or runs the user's body when there's no parent.

```rust
pub trait AnimalObject: Any + Send + Sync {
  fn __parent(&self) -> Option<&dyn AnimalObject>;
  fn __animal_fields(&self) -> &__AnimalFields {
    self.__parent().unwrap().__animal_fields()
  }
  fn __is(&self, target: TypeId) -> bool;
  fn __cast(self: Arc<Self>, out: &mut dyn Any);
  fn speak(&self, this: &Animal) -> Str {
    match self.__parent() {
      Some(p) => p.speak(this),
      None => this.__speak(), // the body written in `#[class_methods] impl Animal`
    }
  }
}

pub trait DogObject: AnimalObject {
  fn __dog_parent(&self) -> Option<&dyn DogObject>;
  fn __dog_fields(&self) -> &__DogFields { /* forward */
  }
  fn fetch(&self, this: &Dog) -> Str { /* forward, or Dog's body */
  }
}
```

Default methods forward to the parent's _implementation_. A class that doesn't override `speak` therefore inherits
whichever ancestor last overrode it. That's exactly the behaviour a blanket impl can't express without specialization,
which is why the main document rejected 6.2 when the ancestors are unknown.

### 2.2 Storage implements every ancestor's trait

Storage nests by value, as in the main document's section 4.2, but needs no `#[repr(C)]`. Knowing the full chain,
`Puppy`'s macro implements every ancestor's trait for `__PuppyFields`:

```rust
pub struct __PuppyFields {
  base: __DogFields,
}

impl AnimalObject for __PuppyFields {
  fn __parent(&self) -> Option<&dyn AnimalObject> {
    Some(&self.base)
  }
  fn __is(&self, target: TypeId) -> bool { /* see 2.5 */
  }
  fn __cast(self: Arc<Self>, out: &mut dyn Any) { /* see 2.5 */
  }
  // #[override(Animal)]: the override goes in the trait that declares it
  fn speak(&self, this: &Animal) -> Str { /* builds a `Puppy` from `this` and runs Puppy's body on it, see 2.4 */
  }
}

impl DogObject for __PuppyFields {
  fn __dog_parent(&self) -> Option<&dyn DogObject> {
    Some(&self.base)
  }
  // #[override(Dog)]
  fn fetch(&self, this: &Dog) -> Str { /* ... */
  }
}
```

This is where both new requirements come from:

- **The ancestor list** tells the macro which traits to implement (`AnimalObject`, `DogObject`) and their naming
  convention.
- **`#[override(Animal)]`** tells it _which_ of those impls a method goes in. Knowing the ancestors isn't enough,
  because the macro still can't see which ancestor declared `speak`. The main document sidesteps this by assigning to a
  vtable field and letting autoderef find it (section 4.4). Trait impls have no equivalent.

### 2.3 Handles carry one view per level

Trait objects of different traits have different vtable pointers, so a `Puppy` handle can't be reinterpreted as a `Dog`
one without `unsafe`. Instead each handle stores its ancestor's view, which makes `Deref` safe:

```rust
#[derive(Clone)]
pub struct Animal(Arc<dyn AnimalObject>);

#[derive(Clone)]
pub struct Dog {
  obj: Arc<dyn DogObject>,
  as_animal: Animal, // built with trait upcasting (stable since Rust 1.86)
}

impl Deref for Dog {
  type Target = Animal;
  fn deref(&self) -> &Animal {
    &self.as_animal
  }
}

#[derive(Clone)]
pub struct Puppy {
  obj: Arc<__PuppyFields>, // a final class can hold its concrete storage
  as_dog: Dog,
}
```

Method inheritance and `&Puppy → &Dog → &Animal` coercion work exactly as today.

### 2.4 Virtual calls, late binding and `super`

- A handle method dispatches through its trait: `pub fn speak(&self) -> Str { self.0.speak(self) }`.
- The trait method receives the handle as `this`. Base bodies such as `describe` therefore call back through the
  _object's_ implementations, which preserves late binding.
- `super_speak()` calls the parent storage's implementation with the same `this`: `AnimalObject::speak(&self.base,
  this)`.
- A non-overridden virtual method walks `__parent()` until it finds an implementation. The walk costs nothing in
  practice. Each storage type gets its own copy of the forwarding default, and `__parent()` returns a field of a known
  type, so the compiler inlines the whole walk into one dynamic call.
- **An override must build a handle of its class.** The user's body is written against `&self: &Puppy`, but the trait
  method only receives `this: &Animal`. Without `unsafe`, the only way to get a `&Puppy` from it is to build a `Puppy`,
  i.e. `this.downcast::<Puppy>()`. That means a `TypeId` check plus one `Arc` increment and decrement per level of the
  overriding class: about 25 ns for a `Dog` override. A `super` call that lands on another override pays again.
  Base-class bodies don't pay this, because `this` is already their own handle type. The spike's overrides never use
  `self`, so it doesn't show this cost.

### 2.5 `is`, `downcast` and identity

- `__is(TypeId)` and `__cast` are generated per class from the ancestor list:
  - `__is` compares the requested handle type against the class and each ancestor, which gives `is::<T>()`.
  - `__cast(self: Arc<Self>, out: &mut dyn Any)` builds the matching handle. It writes it into the caller's `Option<C>`,
    found with `downcast_mut`.
- `downcast::<C>()` checks `__is` first and only then clones the `Arc` and calls `__cast`. A failed downcast therefore
  touches no refcounts, a successful one doesn't allocate, and the whole path is safe.
- The spike uses an earlier version, where `__cast` returns a `Box<dyn Any>` for the caller to unbox with
  `Box::downcast`. It's simpler, but it clones the `Arc` even on failure and allocates on success. That made failed
  downcasts about 10× slower (about 13 ns against 1.3 ns) and successful ones about 20% slower.
- Identity compares allocation addresses with `std::ptr::addr_eq(Arc::as_ptr(..), Arc::as_ptr(..))`, which ignores
  vtable metadata, so every view of an object compares equal.

### 2.6 Mistakes are compile errors

| Mistake                                          | Result                                                            |
| ------------------------------------------------ | ----------------------------------------------------------------- |
| An ancestor is missing from `extends`            | `DogObject: AnimalObject` isn't satisfied for the storage.        |
| The ancestors are listed in the wrong order      | The parent's storage doesn't implement a listed ancestor's trait. |
| `#[override(Dog)]` on a method `Animal` declared | `speak` isn't a member of trait `DogObject` (E0407).              |
| An override's signature doesn't match            | The trait impl's method doesn't match its declaration.            |

None of them can cause unsoundness. The worst case is a confusing error pointing at generated code, which spans and
`#[diagnostic::on_unimplemented]` can improve.

## 3. Comparison with the main design

|                                               | Main document (section 4)                                    | This variant                                                         |
| --------------------------------------------- | ------------------------------------------------------------ | -------------------------------------------------------------------- |
| `unsafe`                                      | a small, audited amount; Miri required in CI                 | none (`#![forbid(unsafe_code)]`)                                     |
| `extends`                                     | the parent only                                              | every ancestor, nearest first                                        |
| Overrides                                     | `#[override]`                                                | `#[override(DeclaringClass)]`                                        |
| Handle size                                   | 4 words                                                      | 2 words per open level, plus 1 for a final class, e.g. 5 for `Puppy` |
| Cloning a handle                              | 1 atomic increment                                           | 1 per level                                                          |
| Virtual call                                  | 1 indirect call                                              | 1 dynamic call; overrides also build a handle of their class (2.4)   |
| Field access                                  | direct                                                       | direct, if the macro generates the path from the chain\*             |
| Generic classes                               | vtable registry, read lock on construction and `super` calls | no registry, the compiler builds the vtables                         |
| `#[repr(C)]` layout                           | required                                                     | not required                                                         |
| Generated code size                           | one vtable per class                                         | one trait impl per ancestor per class, so it grows with depth²       |
| `Send + Sync` fields, `'static`, no lifetimes | required                                                     | required (`Any`, `dyn … + Send + Sync`)                              |
| Abstract method check                         | runtime, at vtable build                                     | runtime (the root default panics)\*\*                                |

\* The prototype uses the default forwarding accessors. Since the macro knows the chain, it can override them with
direct paths (`&self.base.base`), as the benchmark fork does.

\*\* An abstract method can't be a required trait method. Every descendant would then have to implement it, including
those that inherit an ancestor's override, and the macro can't tell which those are. So it gets a forwarding default
like every other virtual method, and the root's default panics. Compile-time checks still need descriptors (main
document, section 5).

### 3.1 Measurements

[benches/inheritance.rs](../../projects/galvanize-rs/benches/inheritance.rs) runs both designs (`cargo bench -p
galvanize-rs --bench inheritance`). It doesn't run the spikes. It runs bench-owned forks of them, which follow the
documents where the spikes took shortcuts:

- the main design's override thunks use unchecked views, and its non-generic vtables are `LazyLock` statics;
- this variant uses direct field paths, builds a handle for every override body (section 2.4), and uses the
  allocation-free downcast (section 2.5).

The forks also add allocation-free members, so the numbers measure the inheritance machinery rather than `String`s:

- `legs`, a virtual method nobody overrides;
- `weight`, overridden by `Dog`, overridden again with a `super` call by `Puppy`, and inherited by `Beagle`, a `Dog`
  subclass that overrides nothing;
- `bark_volume`, declared by `Dog`;
- the `age` field;
- a generic `Boxed<T>` with a subclass `Tagged<T>`.

A startup check asserts that both forks return the same results before anything is timed.

Single-thread medians, from two runs on an i7-13800H. Allocation-free rows reproduce within a few percent. Rows that
allocate vary by about ±10–15%.

| Operation                                                 | Main design        | This variant    |
| --------------------------------------------------------- | ------------------ | --------------- |
| Handle size, `Animal` / `Dog` / `Puppy` (words)           | 4 / 4 / 4          | 2 / 4 / 5       |
| Clone, `Animal` / `Dog` / `Puppy`                         | 5.4 / 5.4 / 5.4 ns | 5.4 / 9 / 13 ns |
| Construct, `Animal` / `Dog` / `Puppy`                     | 35 / 34 / 34 ns    | 25 / 34 / 38 ns |
| Field read (`RwLock<u64>`), any level                     | 11 ns              | 11 ns           |
| Virtual call, base body or inherited (`legs`), any level  | 1.0 ns             | 0.8 ns          |
| Virtual call, `Dog`'s override (`weight` on a `Dog`)      | 0.8 ns             | 26 ns           |
| Same, inherited by `Beagle`                               | 1.0 ns             | 26 ns           |
| Virtual call, `Puppy`'s override + `super` to `Dog`'s     | 1.4 ns             | 63 ns           |
| `Dog`-declared virtual call, `Puppy`'s override + `super` | 1.6 ns             | 34 ns           |
| `describe` (`format!`, late-binds to `speak`), `Dog`      | 53 ns              | 76 ns           |
| `downcast::<Dog>()`, failing / succeeding                 | 1.5 / 5.5 ns       | 1.3 / 16.5 ns   |
| Generic: construct `Boxed` / `Tagged`                     | 28 / 33 ns         | 13 / 18 ns      |
| Generic: virtual call, base body / override + `super`     | 12 / 26 ns         | 12 / 37 ns      |

What this shows:

- **Overrides are where the designs differ.** The main design's are as cheap as any other indirect call. This variant's
  pay for building a handle of the overriding class (section 2.4), so they cost 25–60 ns. A `super` chain that crosses
  several overrides pays for each.
- **Forwarding is free.** Forwarding through `__parent()` compiles down to a single dynamic call, however deep the
  hierarchy (section 2.4). So an inherited method costs what it costs in the class that defines it: `Beagle` pays
  exactly what `Dog` does.
- **Clones and handles grow with depth here and stay flat in the main design**, as section 3 predicts. It's a few
  nanoseconds per level.
- **The main design's vtable registry costs about 15 ns.** That's the read lock plus a `HashMap` lookup, paid on every
  construction of a generic class and every `super` call into a generic parent. For non-generic classes, the two
  designs' construction costs are within about 10 ns of each other.
- With 8 threads sharing one object, the comparisons keep their shape. Most rows rise by 10–40%. The worst case is
  `describe` on a `Puppy` in this variant, which roughly doubles because it builds two handles per call.

## 4. Variations

- **Descriptors instead of the ancestor list.** The main document's section 5 descriptors could supply the chain _and_
  each method's declaring class. That would remove both new annotations, at the cost of the descriptor machinery's own
  downsides.
- **Lazy views.** Handles could store only `obj` and build ancestor views on demand. That shrinks handles and clones,
  but `Deref` has to return a reference to something that already exists, so inherited methods and `&Sub → &Base`
  coercion would be lost. That's the same trade-off as the safe variant sketched at the end of the main document's
  section 4.10.
- **Mixing the designs.** Both share the same surface syntax apart from the annotations. The runtime could start with
  this variant and later switch to the main document's layout, behind the same macros, if profiling shows that override
  cost, handle size or clone cost matters. Users would only need to drop the now-unneeded annotations.

## 5. Open questions

In addition to the main document's section 8:

1. Is "no `unsafe`, no Miri, simpler generics" worth `#[override(Class)]` and the ancestor lists? For a library whose
   pitch is "Rust like a higher-level language", the extra annotations cut against the grain. The `unsafe`, on the other
   hand, is invisible to users.
2. Should the ancestor list be optional, so `extends = Dog` alone works for direct children of a root class (depth 1),
   and the full list only becomes required deeper down?
3. Should field access always go through generated direct paths, or is forwarding through `__parent()` acceptable given
   the library's performance stance? It probably makes little difference. Forwarding virtual methods compiles down to
   one dynamic call (section 3.1), and the field accessors forward the same way, but only direct paths were measured.
4. Is the cost of building a handle for every override body (section 2.4) acceptable? It's 25–60 ns per call, about the
   cost of one or two small `String` allocations. That fits the library's "trade performance for simplicity" stance, but
   it's 20–40× the main design's cost. Possible mitigations:
   - skip the handle when the override's body never mentions `self`, which the macro can see;
   - run override bodies against the storage, with generated field accessors on it, which changes what `self` is inside
     an override.
