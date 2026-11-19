# True Inheritance: Safe Runtime, Ergonomic Attributes, Generated Descriptors

Status: **Executable research spike**, not a replacement for the current class API. Tested on 2026-11-19.

This implements the third candidate identified in the
[defensive review](galvanize-rs-true-inheritance-defensive-review.md): the
[safe object model](galvanize-rs-true-inheritance-safe.md), with compile-time descriptors supplying the ancestry and
method ownership that its original syntax asked users to repeat.

## Result

**Yes: separate class/method attributes, an immediate-parent-only declaration, and unqualified override markers work
without introducing unsafe code.** This is now demonstrated by real macros and cross-crate tests, not hand-expanded code
pretending the macros exist.

The important qualification is that this is a third **macro/design combination**, not a faster third runtime:

- User-maintained ancestor lists and declaring-class annotations disappear.
- The compiler generates the safe implementation's ancestor traits and forwarding code.
- Handles still grow with depth, and overrides still reconstruct typed owning handles.
- There is no runtime descriptor lookup or generic-vtable registry.
- Measurements land in the same range as the handwritten safe implementation.

The trade is **macro complexity instead of unsafe object-layout machinery**, not free performance.

## 1. The syntax that actually compiles

The initial implementation exposed an issue in the earlier documents: `virtual` and `override` are reserved Rust
keywords. Literal `#[virtual]` and `#[override]` do not parse in these ordinary Rust impl blocks. That problem was
invisible in the hand-expanded prototypes.

Following the requested macro namespace, the surface is:

| Attribute                          | Meaning                                                         |
| ---------------------------------- | --------------------------------------------------------------- |
| `#[class(open)]`                   | Declare an extensible class                                     |
| `#[class(extends = Parent)]`       | Declare a final subclass, naming only its immediate parent      |
| `#[class(open, extends = Parent)]` | Declare an extensible subclass                                  |
| `#[class_methods]`                 | Supply that class's single descriptor-producing impl block      |
| `#[class_virtual]`                 | Declare a virtual method                                        |
| `#[class_override]`                | Override an inherited virtual, with no declaring-class argument |

`class` remains the entry point; its companion attributes use the `class_` prefix. The two method markers are consumed
by `class_methods`, so they do not need separate imports.

For example, the actual [benchmark fixtures](../../projects/galvanize-rs/research/descriptor-models/src/lib.rs) contain:

```rust
#[class(extends = Dog)]
pub struct Puppy {}

#[class_methods]
impl Puppy {
  pub fn new(name: &str) -> Self {
    Self::from_data(PuppyData { base: Dog::init(name) })
  }

  #[class_override]
  pub fn speak(&self) -> String {
    format!("{} (but squeakier)", self.super_speak())
  }

  #[class_override]
  pub fn weight(&self) -> u64 {
    self.super_weight() / 4
  }

  #[class_override]
  pub fn bark_volume(&self) -> u64 {
    self.super_bark_volume() / 2
  }
}
```

The user does not tell the macro that `speak` and `weight` belong to Animal while `bark_volume` belongs to Dog. The
descriptor does.

An empty `#[class_methods] impl Beagle {}` is sufficient for a class with no methods. In the benchmark fixture, Beagle's
block additionally provides its constructor.

The generated API includes nested `XData`, `from_data`, cloning getters and setters, borrowed `Deref` views,
target-inferred `upcast`, checked `is`/`downcast`, and allocation identity comparison:

```rust
let puppy = Puppy::new("Bit");
let animal: Animal = puppy.upcast();
assert!(animal.ptr_eq(&puppy));
assert_eq!(animal.describe(), "Bit says Woof (but squeakier)");
assert!(animal.downcast::<Dog>().is_some());
```

## 2. What was built

Two isolated, non-publishable workspace packages:

- [descriptor-macros](../../projects/galvanize-rs/research/descriptor-macros/): the actual proc-macro implementation.
- [descriptor-models](../../projects/galvanize-rs/research/descriptor-models/): the generated Animal/Dog/Puppy/Beagle
  and Boxed/Tagged fixtures, integration tests, and a runnable example.

The existing [Divan harness](../../projects/galvanize-rs/benches/inheritance.rs) now benchmarks:

1. `MainDesign`: the existing unsafe benchmark fork.
2. `SafeDesign`: the existing handwritten safe benchmark fork.
3. `DescriptorDesign`: the actual macro-generated classes from the research package.

The generated classes are not translated into a second handwritten benchmark fork. Public generated hot-path methods are
inlineable across the fixture crate boundary; the benchmark uses the normal optimized profile, without a special LTO
configuration.

The current `galvanize-rs::class` macro and production runtime are unchanged. The fixture package is a
**dev-dependency** of galvanize-rs, not a new production dependency. The two original benchmark implementations are also
unchanged: including the main design here does not repair or approve the soundness defects described in the review.

## 3. How the two attributes cooperate

### 3.1 A compile-time callback, not a global registry

The expansion pipeline is:

1. **`class` records the struct** in a local helper macro. It retains the fields, class options, and type parameters.
2. **`class_methods` supplies the impl** to that helper. The generator now has both items without searching source files
   or storing global proc-macro state.
3. **A subclass invokes its parent's descriptor macro.** The descriptor is re-exported in the macro namespace under the
   class's name. A type and a macro can share that name.
4. **The parent calls back into the generator**, supplying its flattened ancestry.
5. **The generator emits the safe implementation**, selects the declaring ancestor for each override, and publishes the
   subclass's extended descriptor.

A descriptor record contains the defining class's handle, storage, data and object-trait types, plus the names of the
virtual methods it declares. It does **not** duplicate method bodies or signatures. Trait definitions keep their
signatures in the defining crate; Rust checks an override against that trait.

Descriptors exist at macro-expansion time. They do not become an object header, runtime map, lock, or downcast cache.

### 3.2 Cross-crate paths and renamed imports are tested

This works in the integration-test crate:

```rust
use galvanize_rs_descriptor_models::Dog as ImportedDog;

#[class(open, extends = ImportedDog)]
pub struct GuideDog {
  pub badge: String,
}

#[class_methods]
impl GuideDog {
  #[class_override]
  pub fn speak(&self) -> String {
    format!("{}: {}", self.badge(), self.super_speak())
  }
}
```

The import renames the type and its descriptor macro together. The user does not import Animal's trait or list Animal as
an ancestor. A further local subclass of GuideDog is tested too, so ancestry must survive being re-exported through
another descriptor, not merely one direct callback.

This required real hygiene work. A callback can originate in the **parent's** crate. Newly emitted `$crate` paths must
be anchored to the **child's declaration**, while inherited paths retain their original definition sites. The first
cross-crate test caught an incorrect binding; the generator now preserves both origins.

Likewise, generated dispatch/super bodies use the original method receiver's token context. Reusing the signature with
an unrelated generated `self` token initially failed Rust's hygiene checks.

These are manageable problems, but they validate the earlier warning that descriptor machinery is not free.

### 3.3 Generic ancestry is substituted, not guessed

The tests extend an imported `Boxed<T>` as `Labeled<U>`, then extend that as `Collected<C>` with parent
`Labeled<Vec<C>>`. Metadata substitutes the actual parent arguments through every ancestor record.

As a result, the descendant implements the appropriate `Boxed<Vec<C>>` object trait, not a trait accidentally retaining
the original spelling `T` or the intermediate `U`.

This still requires ordinary Rust type checking. It is not a second type system inside the proc macro.

## 4. The runtime remains fully safe

The generated implementation follows the existing safe design:

- Storage nests the parent's storage by value.
- Each storage implements the object traits for its class and ancestors.
- An open-class handle owns an `Arc<dyn ClassObject>` plus its parent handle.
- A final-class handle uses an `Arc<ConcreteFields>` plus its parent handle.
- All those handles point to the same storage allocation.
- `Deref` returns the already-stored parent view.
- Field access uses typed references and `RwLock`; there are no layout casts.
- Checked `Any` writes into `Option<T>` produce downcast results without an extra box allocation.
- An override reconstructs its own typed handle, then executes the user's body on it.
- `super` invokes the immediate parent's storage implementation, using the original object's handle for late binding.

The last point matters for an inherited body that itself makes another virtual call. The tests exercise exactly that: a
parent `score(extra)` calls `self.weight()`, while a descendant overrides both methods and calls `super_score`. The
weight call still reaches the most-derived override.

There are **no raw-pointer dereferences, unsafe blocks/impls, representation casts, or manually maintained vtables** in
this object model. Allocation addresses are only compared or formatted through safe APIs. The macro implementation,
fixtures, tests, and example forbid unsafe code. Since that lint alone is not an audit of proc-macro output, the actual
expanded six-class fixture was also inspected: it contains no `unsafe` keyword.

No Miri run was needed or performed for this spike. A nightly compiler was used only to inspect expanded Rust; the
implementation builds and tests on the pinned **stable Rust 1.100.0** toolchain.

“Safe” here describes this implementation and its generated runtime, not a claim that the standard library or proc-macro
dependencies contain no internal unsafe code.

### What happens to the previous review's defects?

- There is no safe raw-handle reconstruction API capable of reinterpreting Animal storage as Dog storage.
- Generic handles retain the actual typed trait-object/storage relationships. The function-pointer covariance coercions
  that the unsafe prototype accepted are rejected for both generated `Boxed` and `Tagged`.
- There is no generic-vtable registry, so generic construction and `super` do not acquire its global lock.

This does not make the generated API production-hardened. Public hidden helper traits can still be misimplemented in
safe code, leading to logical failures or panics. They cannot grant a raw-pointer cast permission that this runtime does
not possess.

## 5. Measurements

Linux x86-64, i7-13800H, Rust 1.100.0. All figures below are observations on the development machine, not portable
guarantees. It was not CPU-pinned; allocation and scheduling noise are visible.

### 5.1 Single thread

Ranges are the two runs' **medians**, not confidence intervals. Each run used 500 samples of 256 iterations.

| Operation                            | Main         | Handwritten safe | Generated descriptors |
| ------------------------------------ | ------------ | ---------------- | --------------------- |
| Construct Dog                        | 37.5–43.1 ns | 36.4–36.7 ns     | 36.5–37.4 ns          |
| Clone Puppy                          | 5.1–7.9 ns   | 13.1–14.6 ns     | 12.5–13.3 ns          |
| Inherited `legs`, Puppy              | 1.20–1.22 ns | 1.13–1.22 ns     | 1.11–1.29 ns          |
| Dog `weight` override                | 0.72–0.77 ns | 23.6–24.4 ns     | 24.4–29.8 ns          |
| Puppy `weight` override plus `super` | 1.35–1.41 ns | 63.4–68.2 ns     | 58.3–69.4 ns          |
| Successful Animal-to-Dog downcast    | 5.4–5.8 ns   | 15.7–17.3 ns     | 17.0–19.0 ns          |
| Construct generic Tagged             | 35.7–36.6 ns | 17.7–21.0 ns     | 17.7–20.4 ns          |
| Generic Tagged `get` plus `super`    | 24.6–25.7 ns | 35.2–41.4 ns     | 34.3–37.4 ns          |

The generated and handwritten safe implementations have the same broad cost profile. Small differences are not evidence
of a new runtime advantage: they are not byte-for-byte identical code, and run-to-run noise overlaps.

The [example](../../projects/galvanize-rs/research/descriptor-models/examples/inheritance.rs) reports generated handle
sizes of **2 / 4 / 5 machine words** for Animal / Dog / Puppy, matching the safe variant's shape. Descriptors do not
shrink the handles or remove their per-level reference-count work.

All three fixtures deliberately use bare `RwLock` fields for like-for-like comparison. They do not benchmark the
production class macro's `Arc<RwLock<_>>` field strategies. “One hierarchy allocation” does not include allocations
inside values: constructing these named animals allocates for the object and its string.

### 5.2 Eight threads: shared objects

Longer samples matter for this test. With only 128 iterations, some very short samples showed little contention.
Repeating the shared-object rows with **2,048 iterations and 200 samples** produced:

| Shared-object operation           | Main    | Handwritten safe | Generated descriptors |
| --------------------------------- | ------- | ---------------- | --------------------- |
| Dog `weight`                      | 0.93 ns | 404 ns           | 410 ns                |
| Puppy `weight` plus `super`       | 1.65 ns | 997 ns           | 996 ns                |
| Generic Tagged `get` plus `super` | 779 ns  | 673 ns           | 595 ns                |

These are Divan's per-iteration timing statistics, not aggregate throughput or end-user latency percentiles. The safe
implementations still contend on repeated `Arc` operations during overrides. The descriptor mechanism does not change
that. Shared generic field reads additionally contend on their field lock in all designs.

### 5.3 Eight threads: independent objects

A new `generic_get_independent` benchmark gives every timed batch a private input object, constructed outside the timed
closure using Divan's input generator. The closure performs **64 reads**, and borrows rather than consumes its input.
The displayed Divan time is therefore **per 64-call batch**.

Dividing its Tagged medians by 64:

| Independent Tagged objects                | Main              | Handwritten safe  | Generated descriptors |
| ----------------------------------------- | ----------------- | ----------------- | --------------------- |
| One thread, two runs                      | 25.7–26.4 ns/call | 36.6–37.7 ns/call | 35.4–37.0 ns/call     |
| Eight threads, 200 samples of 128 batches | 539 ns/call       | 40 ns/call        | 40 ns/call            |

This reproduces the review's important counterexample in the common benchmark harness: independent objects avoid shared
field/refcount traffic, but the main design's generic `super` still hits its global registry.

The takeaway is not that the descriptor design beats the unsafe design everywhere. It removes the safe variant's
annotation burden while preserving **both its strengths and its weaknesses**.

## 6. Tests and limitations

### Verified

- All original benchmark operations and startup result checks run against all three designs.
- Cross-crate subclassing through a renamed parent import.
- An additional descendant that consumes and republishes inherited metadata.
- Overrides declared two or more levels up, inherited methods, and typed override bodies using subclass fields.
- Parameter forwarding and lexical `super` with late-bound calls inside the parent body.
- Renamed and nested generic type arguments.
- Shared mutation through ancestor views, retained dynamic identity, failed sibling casts, thread transfer, and
  exactly-once storage destruction without an internal ownership cycle.
- Compile-fail fixtures for an unknown override, incompatible signature, extending a final class, unsupported async
  virtual method, and the generic covariance coercions from the review.
- The existing composition class tests and their compile-fail fixtures still pass.

### Deliberate spike boundaries

1. **Exported classes must be declared at crate root.** Descriptor paths currently use `$crate::Class` and generated
   companion names. Arbitrary nested-module placement is not solved. Supporting it ergonomically is the next important
   metadata/hygiene experiment.
2. **One paired methods block.** The struct must precede its `class_methods` block in the same module. Even a class with
   no methods needs that block. Additional ordinary impl blocks are possible, but do not contribute virtual metadata.
3. **The helper protocol is experimental.** Consumers need the experimental macro dependency under its expected crate
   name. Dependency renaming/version skew and a public, versioned descriptor protocol are not handled.
4. **Only type generics.** No class lifetime or const parameters; all type parameters get `Clone + Send + Sync +
   'static` bounds. Bounds belong on the struct, and parent arguments must be supplied explicitly.
5. **A limited virtual-method surface.** Public synchronous, safe, non-generic `&self` methods with named parameters are
   supported. No async/generic virtuals, borrowed-return or `Self`-dependent virtual API design, abstract methods,
   virtual accessors, or protected visibility is claimed.
6. **Not the complete production class macro.** No configurable field strategies, readonly/update accessors,
   lifetime-preserving closed classes, dynamic field-rich Debug, heterogeneous equality/hash, or full attribute
   forwarding. Field attributes are currently rejected; class structs accept doc attributes only.
7. **Safe does not mean impossible to misuse logically.** Hidden helpers are public for generated cross-crate code. API
   sealing, diagnostics, invariants around custom trait implementations, and lock/reentrancy behaviour still need
   production design work. User-created strong-reference cycles can still leak.
8. **Compile-time/code-size costs remain unmeasured.** Each descendant implements ancestor traits and republishes
   ancestry. This is additional generated code; a large/deep hierarchy study is still needed.

These are genuine limits, not reasons to fall back to unsafe immediately. The spike answers the specific question “can
the missing metadata be supplied while keeping separate attributes and safe runtime code?” affirmatively.

## 7. What this changes about the recommendation

The original syntax-versus-safety trade-off is no longer merely hypothetical:

> Users can name only the immediate parent and mark overrides without declaring-class annotations, while the runtime
> stays safe.

I would now treat **safe runtime + descriptors as a serious candidate for the default implementation**, provided nested
modules and the remaining public-macro ergonomics can be resolved. Its runtime costs have not become worse merely
because metadata is generated instead of handwritten.

The unsafe design still has meaningful advantages for handle size, cloning, and especially shared-object overrides.
Those need to justify their soundness/maintenance burden on actual workloads. Syntax alone is no longer a sufficient
reason to prefer it.

## 8. Reproduce

All commands run from the repository root through Pixi. No Miri is involved.

```sh
# Actual generated objects, including measured handle sizes.
pixi run --locked cargo run -p galvanize-rs-descriptor-models --example inheritance

# Cross-crate behaviour and compile-fail contracts.
pixi run --locked cargo test -p galvanize-rs-descriptor-models

# Run every benchmark once with startup result checks.
pixi run --locked cargo test -p galvanize-rs --bench inheritance

# Common harness; all three designs.
pixi run --locked cargo bench -p galvanize-rs --bench inheritance -- --threads 1 --sample-count 500 --sample-size 256

# Longer shared-object samples.
pixi run --locked cargo bench -p galvanize-rs --bench inheritance -- weight generic_get --skip generic_get_independent --threads 8 --sample-count 200 --sample-size 2048

# Private-object batches; divide displayed Tagged times by 64 for ns/call.
pixi run --locked cargo bench -p galvanize-rs --bench inheritance -- generic_get_independent --threads 8 --sample-count 200 --sample-size 128

# Just the third design, or use `pixi run task bench -- DescriptorDesign`.
pixi run --locked cargo bench -p galvanize-rs --bench inheritance -- DescriptorDesign
```

For expansion inspection only, the already-installed nightly can emit the generated Rust:

```sh
pixi run --locked cargo +nightly-2026-11-19 rustc -p galvanize-rs-descriptor-models --lib -- -Zunpretty=expanded
```
