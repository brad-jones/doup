# True Inheritance for galvanize-rs Classes

Status: **Research**. Nothing here is scheduled. It describes what it would take to go from today's `extends`
(composition + `Deref`, "Option A") to inheritance that behaves like Python, Java, C# or Kotlin ("Option B").

> **Historical safety warning:** the [defensive review](galvanize-rs-true-inheritance-defensive-review.md) demonstrated
> UB through the original prototypes' safe APIs despite their existing Miri checks passing. The separate
> [hardened, macro-generated follow-up](galvanize-rs-true-inheritance-hardened.md) repairs those boundaries, adds async
> support and adversarial tests, and measures all four candidates. The original implementation below remains unchanged
> historical material, not an approved safe API.

A hand-expanded prototype of the proposed design lives in
[galvanize-rs-true-inheritance-spike.rs](galvanize-rs-true-inheritance-spike.rs). It's standalone, compiles on our
pinned stable toolchain, and runs. It passes Miri (`task miri`), see [4.10](#410-unsafe-inventory-and-soundness).

A fully safe variant of this design, with no `unsafe` at all, is described in a companion document,
[galvanize-rs-true-inheritance-safe.md](galvanize-rs-true-inheritance-safe.md). It trades `unsafe` for extra
annotations, larger handles and slower overrides. Read this document first, as the companion only describes what
changes. Its section 3.1 benchmarks both designs, and
[galvanize-rs-true-inheritance-comparison.md](galvanize-rs-true-inheritance-comparison.md) compares them at a high
level, with a recommendation.

## TL;DR

It's achievable on **stable Rust**, without specialization or nightly features. The prototype demonstrates every core
piece: one identity across the hierarchy, late-bound virtual methods (including overrides two levels up), `super` calls,
`is`/`downcast`, `&Puppy → &Animal` coercion, and sharing across threads.

The design rests on five ideas:

1. **One allocation per object.** A subclass's storage nests its base's storage _by value_, as the first field of a
   `#[repr(C)]` struct. A pointer to a `Puppy`'s fields is therefore also a valid pointer to its `Dog` and `Animal`
   fields.
2. **Handles are views.** Every class handle is a `#[repr(transparent)]` wrapper around the same `RawHandle`: an owning
   `Arc`, a fields pointer and a vtable pointer. Upcasting just reinterprets the view, and `Deref` chains for free.
3. **Nested vtables of `fn` pointers**, with `Deref`/`DerefMut` from each class's vtable to its base's. An override is
   written as `vt.speak = |this| ...`. Autoderef finds the `speak` slot in whichever ancestor declared it, and the
   closure's parameter type is inferred from the slot. So **the macro never needs to know the ancestors' methods**.
4. **Base method bodies run on the base handle**, which carries the _object's_ vtable. A non-virtual `describe` that
   calls `self.speak()` therefore runs the subclass's override (late binding).
5. **`TypeId` chains** for `is::<Dog>()` / `downcast::<Dog>()`. Each class's storage answers "am I, or do I extend, X?"
   by checking itself and then asking its base.

The costs:

- A small, audited amount of `unsafe` in the runtime crate (pointer views, `Send`/`Sync` impls). This is avoidable, see
  the [fully safe variant](galvanize-rs-true-inheritance-safe.md).
- Open classes need `'static` generics and `Send + Sync` fields. Lifetime parameters aren't possible.
- A second macro for methods (`#[class_methods]`).
- A vtable registry for generic classes.
- Construction changes: the base is passed as _data_, not as an already-built object.

Things that stay out of reach are `protected` visibility, multiple inheritance (use traits), and, without the optional
descriptor mechanism in [section 5](#5-optional-class-descriptors-cross-item-metadata), compile-time checks for abstract
methods.

---

## 1. What "true" inheritance means

| Feature                                   | Python / Java / C# / Kotlin | Today (Option A)                           | Option B (this doc)                 |
| ----------------------------------------- | --------------------------- | ------------------------------------------ | ----------------------------------- |
| Inherited fields and methods              | yes                         | yes, via `Deref`                           | yes, via `Deref`                    |
| Upcast to an ancestor                     | implicit                    | `upcast()`, returns the separate base      | `upcast()`, returns the same object |
| One identity (`a == d` after upcast)      | yes                         | no, the base is its own `Arc`              | yes                                 |
| Virtual methods / overriding              | yes                         | no, same-named methods only shadow         | yes, `#[virtual]` / `#[override]`   |
| Late binding inside base methods          | yes                         | no, `self` in a base method is just a base | yes                                 |
| `super.method()`                          | yes                         | `self.base().method()`                     | `self.super_method()`               |
| Downcast / `isinstance`                   | yes                         | no                                         | `is::<T>()`, `downcast::<T>()`      |
| Abstract classes / methods                | yes                         | no                                         | yes, checked at runtime\*           |
| `final` / `sealed`                        | Java/Kotlin/C#              | n/a                                        | `final` yes, `sealed` maybe         |
| Constructor chaining (`super().__init__`) | yes                         | pass a built base handle                   | pass the base's _data_              |
| `Debug` / `repr` shows the dynamic type   | yes                         | shows the static type                      | yes                                 |
| `protected`                               | Java/C#/Kotlin              | no                                         | no (`pub(crate)` at best)           |
| Multiple inheritance                      | Python, C++                 | no                                         | no, use traits                      |
| Interfaces                                | yes                         | traits                                     | traits                              |

\* Compile-time checks are possible with class descriptors, see
[section 5](#5-optional-class-descriptors-cross-item-metadata).

## 2. Why Option A can't get there

Today `Dog`'s storage holds an `Animal` _handle_, so a `Dog` is really two objects:

```text
Dog handle ──► Arc<__DogFields { base: Animal ─┐, tricks }>
                                               └──► Arc<__AnimalFields { name }>
```

- `dog.upcast::<Animal>()` returns the inner `Animal`. That object has no idea it's part of a `Dog`, so there's nothing
  to downcast back to, and `animal == dog` can't hold.
- When a base method runs, `self` is the inner `Animal`. Anything it calls is resolved against `Animal`, so a subclass
  can never change its behaviour. That's the "shadows but doesn't override" limitation.
- Overriding needs dispatch through something that belongs to the _object_, i.e. a vtable. Nothing in Option A carries
  one.

This isn't fixable incrementally. The object model itself has to change.

## 3. Constraints Rust imposes

These shape every design decision below.

1. **A proc macro sees one item.** `#[class(extends = Animal)] struct Dog` can't see `Animal`'s fields, methods or
   ancestors. Information has to flow through the type system or through generated helper macros (see
   [section 5](#5-optional-class-descriptors-cross-item-metadata)).
2. **No struct inheritance, no specialization.** A blanket impl ("every descendant of `Animal` gets this") can't be
   overridden for one type on stable Rust. That rules out the obvious "one trait per class with default methods that
   forward to the parent" design, unless the macro is told the ancestors, see
   [section 6.2](#62-one-object-trait-per-class).
3. **`Any`/`TypeId` require `'static`.** Downcasting therefore rules out lifetime parameters on open classes, and type
   parameters must be `'static`.
4. **No generic `static`s.** A vtable for `Boxed<T>` can't be a `static`, so generic classes need a registry keyed by
   `TypeId` (or a vtable per object).
5. **`Deref` is the only "inherit methods" mechanism.** It works for method calls and `&Sub → &Base` coercions, but not
   for trait bounds. `fn f(x: &impl Speak)` won't accept a `&Dog` just because `Animal: Speak`.
6. **`Arc<T>` can't be reinterpreted as `Arc<U>`.** An upcast can't turn an `Arc<__DogFields>` into an
   `Arc<__AnimalFields>`. Hence handles carry the owner (as `Arc<dyn …>`) and separate typed pointers into it.

## 4. Proposed design

### 4.1 Surface syntax

```rust
#[class(open)] // may be extended; without `open` a class stays as cheap as today
pub struct Animal {
  pub name: Str,
}

#[class_methods] // required once per class in an open hierarchy, generates the vtable
impl Animal {
  pub fn new(name: &str) -> Self {
    Self::from_data(Self::init(name))
  }

  /// Builds the data, so subclasses can reuse it, like `super().__init__()`.
  pub fn init(name: &str) -> AnimalData {
    AnimalData { name: name.into() }
  }

  #[virtual]
  pub fn speak(&self) -> Str {
    "...".into()
  }

  /// Not virtual, but `self.speak()` is late bound, so subclasses change what it prints.
  pub fn describe(&self) -> Str {
    sf!("{} says {}", self.name(), self.speak())
  }
}

#[class(open, extends = Animal)]
pub struct Dog {
  pub tricks: List<Str>,
}

#[class_methods]
impl Dog {
  pub fn new(name: &str) -> Self {
    Self::from_data(DogData {
      base: Animal::init(name),
      tricks: List::new(),
    })
  }

  #[override]
  pub fn speak(&self) -> Str {
    sf!("Woof (not {})", self.super_speak())
  }

  #[virtual]
  pub fn fetch(&self) -> Str {
    sf!("{} fetches", self.name())
  }
}

#[class(extends = Dog)] // not `open`, so it's final
pub struct Puppy {}

#[class_methods]
impl Puppy {
  #[override] // declared two levels up, the macro doesn't need to know that
  pub fn speak(&self) -> Str {
    "Yip".into()
  }
}

let p = Puppy::from_data(PuppyData { base: DogData { base: Animal::init("Bit"), tricks: List::new() } });
let a: Animal = p.upcast(); // same object
assert_eq!(a.describe(), "Bit says Yip"); // late bound
assert!(a == p && a.is::<Dog>());
let d: Option<Dog> = a.downcast();
```

### 4.2 Memory layout

```text
                         one Arc allocation
                ┌───────────────────────────────────────────┐
Puppy handle ─┐ │ Object { vt ─► &'static PUPPY_VTABLE,     │
Dog handle  ──┼─┼─► fields: __PuppyFields {   #[repr(C)]    │
Animal handle ┘ │       base: __DogFields {   #[repr(C)]    │
                │         base: __AnimalFields { name },    │
                │         tricks },                         │
                │   } }                                     │
                └───────────────────────────────────────────┘
```

Each storage struct is `#[repr(C)]` with `base` first, so its address is also the address of every ancestor's fields.
Each field still lives in its own `MutablePointer`, so strategies (`ArcRwLockStrategy`, etc.) work exactly as today.

### 4.3 Handles

```rust
#[derive(Clone)]
pub struct RawHandle {
  owner: Arc<dyn Storage>, // keeps the object alive, answers `is`
  fields: NonNull<()>,     // the most-derived fields, valid for every ancestor
  vt: NonNull<()>,         // the object's vtable, valid as every ancestor's vtable
}

#[repr(transparent)]
pub struct Animal(RawHandle);
#[repr(transparent)]
pub struct Dog(RawHandle);

impl Deref for Dog {
  type Target = Animal;
  fn deref(&self) -> &Animal {
    unsafe { &*(self as *const Dog as *const Animal) } // same bits, see the soundness notes
  }
}
```

- **Upcast** clones the `RawHandle` (one `Arc` increment) and rewraps it. `&Puppy → &Dog → &Animal` coercion works via
  `Deref`, as today.
- **Identity**: `ptr_eq`, `PartialEq` and `Hash` use `Arc::as_ptr(&owner)`. All views share it, so `impl<C: Class>
  PartialEq<C> for Animal` makes `animal == puppy` work, and mixed sets/maps of upcast handles behave.
- **Field accessors** read `fields` as `&__AnimalFields`. Every view can do this thanks to the prefix layout, with no
  dynamic lookup.

A handle is four words instead of today's one. That fits the library's "trade performance for simplicity" stance.

### 4.4 Virtual methods and vtables

`#[class_methods]` generates one vtable struct per class. It holds a `fn` pointer per virtual method the class
_declares_, plus its base's vtable as the first field:

```rust
#[repr(C)]
#[derive(Clone)]
pub struct __AnimalVTable {
  pub speak: fn(&Animal) -> Str,
}

#[repr(C)]
#[derive(Clone)]
pub struct __DogVTable<B> {
  base: B,
  pub fetch: fn(&Dog) -> Str,
}
impl<B> Deref for __DogVTable<B> {
  type Target = B; /* &self.base */
}
impl<B> DerefMut for __DogVTable<B> {
  /* &mut self.base */
}
```

`Dog`'s macro doesn't know the type of `Animal`'s vtable, so it names it through an associated type,
`__DogVTable<<Animal as OpenClass>::VTable>`.

A class's vtable is built once, by cloning its parent's and assigning its overrides:

```rust
let mut vt = Dog::vtable().clone();
vt.speak = |this| unsafe { Puppy::view_unchecked(this) }.__override_speak();
```

This line is the crux of the design, and the prototype confirms it works:

- `vt.speak` autoderefs through `DerefMut` until it finds the slot, however many levels up `speak` was declared. The
  macro only needs the method's name. A typo gives `no field 'spek' on type __DogVTable<…>`.
- The closure's parameter type (`&Animal`) is inferred from the slot's `fn` pointer type, so the macro never names the
  declaring class. A signature that doesn't match the virtual method is a type error on that line.
- The thunk converts `this` to a `Puppy` view without checking. That's sound because only `Puppy` objects ever point at
  this vtable, or a subclass's copy of it. A checked `downcast` would work too, at the cost of a `TypeId` walk plus an
  `Arc` increment and decrement per call. That's about 5 ns, against about 1 ns for a whole unchecked call. The spike
  takes this shortcut. The benchmark fork doesn't.

The generated method on the handle just dispatches through the vtable:

```rust
pub fn speak(&self) -> Str {
  (self.vt().speak)(self)
}
```

The user's body is renamed (`__speak` / `__override_speak`) and called from the vtable entry.

**`super` calls.** `self.super_speak()` is generated next to each override as `(Parent::vtable().speak)(self)`. Calling
a `fn` pointer is a coercion site, so `&Puppy` coerces to whatever `&Ancestor` the slot takes. Again, the macro doesn't
need to know which ancestor that is.

**Virtual by default?** No. Make it opt-in, like C#, Kotlin and C++: it's explicit, and non-virtual methods keep today's
zero-cost static dispatch. Non-virtual methods still get late binding for any virtual methods they call.

### 4.5 `is`, `downcast` and `Debug`

Each storage type implements:

```rust
fn is_or_extends(id: TypeId) -> bool {
  id == TypeId::of::<Self>() || <<Dog as Class>::Fields as Storage>::is_or_extends(id)
}
```

`owner: Arc<dyn Storage>` exposes it dynamically. `downcast::<C>()` checks it, then rewraps the `RawHandle` as a `C`.

For `Debug`, a hidden root virtual method, `__fmt`, lets an `Animal` view of a `Puppy` print as `Puppy { name: ..,
tricks: .. }`, like Python's `repr`. Field flattening works as it does today.

### 4.6 Construction

The base can no longer be built first and handed over, because the subclass object _is_ the allocation. Instead:

- `XData` nests the base's _data_: `DogData { base: AnimalData, tricks }`.
- `from_data` builds the nested storage recursively. Each class generates a hidden `fn __fields(data: XData) ->
  __XFields`, which calls its base's.
- Classes expose an `init(..) -> XData` helper next to `new(..) -> Self`, so subclasses can reuse the base's
  initialisation logic. This is the `__init__` vs `__new__` split.

This is a **breaking change** from Option A (`base: Animal::new(..)` becomes `base: Animal::init(..)`), and it removes
the ability to share one base between several subclass objects. That sharing was an Option A quirk, not a feature people
expect from inheritance.

### 4.7 Abstract classes and methods

- `#[class(abstract)]` doesn't generate `from_data`, so the class can't be instantiated directly. Its `init` still works
  for subclasses.
- `#[abstract] fn speak(&self) -> Str;` in `#[class_methods]` makes a slot initialised to a "not implemented" marker.
  The macro can't see whether a concrete subclass filled every slot, so the check runs when the vtable is built, i.e. on
  first construction: `Puppy doesn't implement abstract method Animal::speak`. A unit test that constructs each concrete
  class catches it early. Compile-time checking needs descriptors
  ([section 5](#5-optional-class-descriptors-cross-item-metadata)).
- `final`: a class without `open` doesn't implement `OpenClass`, so `extends` on it is a compile error.

### 4.8 Generics

- Vtables of generic classes (`fn(&Boxed<T>) -> T`) can't be `static`s. The prototype uses a `TypeId`-keyed registry of
  leaked `&'static` vtables. Non-generic classes can use a plain `LazyLock` static instead.
- A vtable is built outside the registry's lock, because building a subclass's vtable looks up its parent's. Threads
  racing to build the same one must drop the losers rather than leak them, or Miri reports the leak.
- The registry costs a read lock plus a `HashMap` lookup, about 15 ns, on every construction of a generic class. `super`
  calls into a generic parent pay it too, because they fetch the parent's vtable with `Parent::<T>::vtable()`. Each
  vtable could store a pointer to its parent's to avoid that. The numbers come from the safe variant's section 3.1.
- `TypeId` needs `T: 'static`, and lifetimes on open classes are impossible. Non-open classes keep today's full generics
  support.
- The rest of the existing generics machinery (`Clone` bounds, `XData` generics, strategy parameters) carries over.

### 4.9 Threads, strategies and cycles

- `RawHandle` holds raw pointers, so `Send`/`Sync` must be implemented with `unsafe`. To keep that sound, `Storage`
  requires `Send + Sync`, so **every field of an open class must be `Send + Sync`**. Today that's only required when you
  actually share a handle. A non-thread-safe flavour (`Rc`, no `Sync`) could be added later as an option.
- Strategies are unaffected: each field is still its own `MutablePointer`.
- Cycles behave as today: `Arc` cycles leak, and `Debug` on a cyclic graph recurses.

### 4.10 Unsafe inventory and soundness

All `unsafe` lives in the runtime crate or in code the macro generates from fixed templates:

| Where                                  | Why it's sound                                                                                                            |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `Deref` from a sub view to a base view | Both are `#[repr(transparent)]` over `RawHandle`.                                                                         |
| Reading `fields` as an ancestor's      | `#[repr(C)]` storage with `base` first puts every ancestor at offset 0. `owner` keeps it alive, and `Arc` never moves it. |
| Reading `vt` as an ancestor's vtable   | Same rule: `#[repr(C)]` vtables with `base` first.                                                                        |
| Unchecked view in override thunks      | A vtable is only ever installed in objects of its own class.                                                              |
| `downcast` rewrapping                  | Guarded by the `TypeId` chain check.                                                                                      |
| `unsafe impl Send/Sync for RawHandle`  | `Storage: Send + Sync`, and vtables are `'static` `fn` pointers.                                                          |
| `unsafe trait Class`                   | Only the macro implements it. Hand-written impls would break the invariants.                                              |

Any implementation must run its test suite under **Miri** (`cargo +nightly miri test`) and keep it in CI. The prototype
passes `task miri`, which runs it under both Stacked and Tree Borrows, with strict provenance, across 8 thread
scheduling seeds each. That only covers the paths its `main` exercises (upcasts, overrides, `super` calls, downcasts and
sending a handle to another thread), not every invariant in the table above.

A fully safe variant is possible in two ways:

- **Dynamic lookups.** Each view could look up its fields and vtable through the `dyn Storage` owner on every access (a
  virtual call plus a `TypeId` comparison), and ancestor views would be built rather than reinterpreted. That gives up
  `&Sub → &Base` coercion, since `Deref` has to return a reference to something that already exists.
- **Listed ancestors plus one trait per class** (sections 6.2 and 6.5 combined). This keeps `Deref` and coercion, at the
  cost of `#[override(DeclaringClass)]` annotations and handles that grow with depth. It's prototyped and written up in
  [galvanize-rs-true-inheritance-safe.md](galvanize-rs-true-inheritance-safe.md).

## 5. Optional: class descriptors (cross-item metadata)

The core design deliberately avoids needing to know about ancestors. Some nice-to-haves _do_ need it:

- compile-time abstract checks ("`Puppy` doesn't implement `Animal::speak`");
- `const` vtables instead of `LazyLock`s and registries, because `DerefMut` isn't usable in `const` code, so a `const`
  vtable needs the exact `vt.base.base.speak` path;
- friendlier override errors ("`spek` isn't a virtual method of `Puppy`'s ancestors, did you mean `speak`?");
- generated docs listing inherited virtual methods.

The known stable technique is the one `ambassador` uses for `#[delegatable_trait]`. Each `#[class(open)]` also emits a
`macro_rules!` "descriptor" macro containing its fields, virtual methods and parent. The subclass's expansion invokes
the parent's descriptor with a callback into a second proc macro, which then receives the whole chain as tokens. Macros
and types live in different namespaces, so the descriptor can be re-exported under the class's own name, and `extends =
zoo::Animal` maps to `zoo::Animal!`.

The downsides are `#[macro_export]` naming and hygiene problems, awkward cross-crate paths, slower builds and harder
debugging. Treat it as a later, optional layer, not a prerequisite.

## 6. Alternatives considered

### 6.1 Keep Option A

It's simple, has no `unsafe`, keeps full generics and lifetimes, and traits cover dynamic dispatch (see
[examples/traits.rs](../../projects/galvanize-rs/examples/traits.rs)). This is the right answer if the goal is "reuse
fields and methods" rather than "Java-style polymorphism".

### 6.2 One object trait per class

Make each handle an `Arc<dyn AnimalObject>`, where `AnimalObject` has the virtual methods and `trait DogObject:
AnimalObject` adds Dog's. Trait upcasting (stable since Rust 1.86) gives cheap upcasts, and `Any` gives downcasts.

It breaks down because `Puppy` must `impl AnimalObject for __PuppyFields` and `impl DogObject for __PuppyFields`, and
its macro doesn't know `AnimalObject` exists or what methods it has. A blanket "forward everything" impl can't coexist
with per-class overrides without specialization.

Default methods that forward to the parent's implementation _do_ work, once the macro knows the ancestors. That needs
either descriptors (section 5) or the ancestor list from 6.5, see that section.

### 6.3 Hand-rolled C++/GObject layout everywhere

Put the vtable pointer at a fixed offset inside one `#[repr(C)]` struct and cast raw pointers to and from it. That's
more `unsafe`, including manual refcounting, for little gain over 4.3's views, which keep `Arc` in charge of lifetimes.

### 6.4 Closed hierarchies as enums

`#[class(sealed)]` with an enum of every subclass dispatches with a `match`, needs no `unsafe`, and makes exhaustive
`match` on the subclass possible (Kotlin `sealed`). But the base must list its subclasses, which inverts the dependency,
and adding a subclass edits the base. It's worth considering as an addition for algebraic-data-type-style hierarchies,
not as the general mechanism.

### 6.5 Make the user list every ancestor

`#[class(extends = Dog, Animal)]` gives the macro the chain without descriptors, which is what `glib::wrapper!`'s
`@extends` does. The design in section 4 doesn't need the information.

Listing ancestors doesn't remove section 4's `unsafe` by itself, because that comes from the memory layout, not from
missing information. What it does is make 6.2 workable: every descendant's storage implements every ancestor's trait,
and handles carry one view per level. Together with `#[override(DeclaringClass)]` annotations, that gives a design with
no `unsafe` at all. It's covered, with its own prototype, in
[galvanize-rs-true-inheritance-safe.md](galvanize-rs-true-inheritance-safe.md).

## 7. Prior art

- **gtk-rs / glib subclassing**: `glib::wrapper!` declares the full ancestor list with `@extends`. `IsA<T>` provides
  `upcast()`, `downcast()` and `is::<T>()`. Virtual methods are implemented via `*Impl` traits, which have `parent_*`
  methods for `super` calls. The object model itself is GObject's, implemented in C.
- **Servo's DOM**: `#[dom_struct]` types embed their parent struct as the first field. A `Castable` trait provides `is`,
  `upcast` and `downcast`, with the hierarchy knowledge generated from WebIDL. This is essentially 4.2's prefix layout.
- **godot-rust (gdext)**: `#[class(base = Node2D)]`, `Gd<T>` handles, `upcast()` and `cast()`, with virtual methods
  implemented through generated `I*` traits. The engine's class list comes from codegen over the engine's API JSON.
- **ambassador**: the `macro_rules!` callback technique for passing one item's information to a macro on another
  (section 5).

The common thread is that each of these gets the cross-item information from somewhere outside a single proc macro: the
user, external codegen, or helper macros. The design in section 4 avoids needing it for the core by leaning on
associated types, autoderef on vtable fields and closure signature inference.

## 8. Open questions

1. Section 4's design, or the [fully safe variant](galvanize-rs-true-inheritance-safe.md)? The `unsafe` in section 4 is
   invisible to users, while the safe variant's annotations aren't. At runtime, the designs differ mostly in overrides:
   about 1 ns here against 25–60 ns in the safe variant (its section 3.1). The safe variant's own open questions are
   listed in its section 5.
2. Does Option B **replace** Option A's `extends`, or do both exist (e.g. Option A renamed to `#[class(delegate =
   Base)]`)? Replacing is clearer, keeping both avoids the `Send + Sync + 'static` restrictions for simple reuse.
3. One `#[class_methods]` block per class, or a single function-like `class! { struct …; impl … }` that sees both? The
   latter avoids the "forgot the methods block" failure mode, but rustfmt and IDE support inside function-like macros
   are worse.
4. Should overriding be allowed for _generated accessors_ (a virtual `name()`)? It's possible, but it makes every field
   access an indirect call.
5. Is a non-thread-safe (`Rc`) flavour wanted, to lift the `Send + Sync` requirement?
6. Should `Debug` show the dynamic type (Python-like) or the static one (today)?

## 9. Suggested phases

Each phase is shippable and testable on its own. The phases assume section 4's design. The safe variant follows the same
order, minus Miri and the vtable registry.

1. **Object model.** Add `RawHandle`, `Class`, `Storage` and `OpenClass` to the runtime, plus `#[class(open)]` and
   `extends` on top of them: single identity, `Deref` views, `upcast`, `is`, `downcast`, cross-type `PartialEq`/`Hash`,
   nested `XData` construction and `init`. Add Miri to CI. No virtual methods yet.
2. **Virtual methods.** `#[class_methods]` with `#[virtual]`, `#[override]` and `super_*`, nested vtables, `LazyLock`
   statics for non-generic classes.
3. **Abstract and final.** `#[class(abstract)]`, `#[abstract]` methods with the vtable-build check, `Debug` via the
   dynamic type.
4. **Generics.** The vtable registry, `'static` diagnostics and generic vtable types.
5. **Descriptors (optional).** Compile-time abstract checks, `const` vtables, better errors.

Tests to carry through every phase:

- late binding through non-virtual base methods;
- overrides declared two or more levels up;
- `super` chains across three levels;
- identity and hashing across views;
- `downcast` success and failure, and across threads;
- trybuild cases for bad overrides (unknown name, wrong signature), `extends` on a non-`open` class, and lifetimes on
  open classes.
