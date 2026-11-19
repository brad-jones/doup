# True Inheritance: Hardening the Main Design

Status: **Executable research implementation, not production approval**. Tested on 2026-11-20.

This follows [the original main proposal](galvanize-rs-true-inheritance.md) and addresses the unsafe API defects in
[the defensive review](galvanize-rs-true-inheritance-defensive-review.md). The feature baseline is the implemented
[safe descriptor design](galvanize-rs-true-inheritance-descriptors.md), including its async methods.

## Result

The main design can retain embedded base storage, four-word handles, borrowed ancestor views and function-pointer
dispatch while closing the review's demonstrated safe-client reconstruction hole and generic-variance defect.

This experiment uses **actual macros**, not another handwritten expansion. Its safe-facing API is backed by an
explicitly unsafe runtime/generation contract. The original unsafe prototypes are preserved and labelled as historical,
known-unsound comparison material.

Verified evidence:

- The descriptor behavior scenarios pass against the new generated classes, across a real crate boundary.
- Async dispatch supports suspension, borrowed inputs, mutable inputs, callbacks, generic results, awaited `super`, late
  binding, errors, lazy execution, thread migration, Tokio spawning, and cancellation.
- Twelve compile-fail fixtures reject invalid construction, variance, helper/slot calls, forged metadata, unsupported
  signatures, non-Send storage/futures, and escaping future borrows.
- Thirteen runtime integration tests pass under **Stacked Borrows and Tree Borrows**, with **strict provenance** and
  **eight seeds per model**, both normally and with **randomized layout**.
- The default Miri task runs the new suite as well as the historical spike and all four benchmark startup checks.
- Existing descriptor tests, production class tests, and targeted Clippy checks pass.

**Passing Miri is not a universal proof of soundness.** It checks these executions, not every possible safe program or
generated declaration. The safety argument and adversarial API tests below are essential parts of the result.

## 1. What was built

Three new non-publishable research packages:

| Package                                                            | Responsibility                                                                                               |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------ |
| [main-runtime](../../projects/galvanize-rs/research/main-runtime/) | Private erased representation, invariant typed handles, checked casts, borrowed views, immutable table cache |
| [main-macros](../../projects/galvanize-rs/research/main-macros/)   | Prefix-layout class generation and sync/async dispatch                                                       |
| [main-models](../../projects/galvanize-rs/research/main-models/)   | Generated fixtures, integration/UI tests, runnable example                                                   |

The [shared macro-support package](../../projects/galvanize-rs/research/inheritance-macro-support/) contains the
existing safe generator and common attribute/descriptor parsing, substitution, validation and async-lifetime utilities.
The descriptor proc-macro crate is now a thin entry point into that code. Its runtime generation remains fully safe; its
seven existing UI snapshots still pass unchanged.

Both generated implementations consume the same
[Animal/Dog/Puppy/Beagle, Boxed/Tagged and async declarations](../../projects/galvanize-rs/research/inheritance-fixtures.rs).
This avoids benchmarking a second handwritten imitation of either macro.

Only the models package is a new **dev-dependency** of galvanize-rs. The production class runtime and production `class`
macro are unchanged. GitHub Actions are unchanged.

### Supported syntax

The attributes are the same as the descriptor candidate:

```rust
use galvanize_rs_main_models::{AsyncDog as ImportedDog, class, class_methods};

#[class(extends = ImportedDog)]
pub struct AsyncPuppy {
  pub suffix: String,
}

#[class_methods]
impl AsyncPuppy {
  #[class_override]
  pub async fn speak(&self, prefix: &str) -> Result<String, &'static str> {
    Ok(format!("{}/{}", self.super_speak(prefix).await?, self.suffix()))
  }
}
```

Classes expose nested `XData`, `from_data`, cloning getters and setters, `Clone`, borrowed `Deref` views,
target-inferred ancestor `upcast`, checked `is`/`downcast`, `ptr_eq`, and a minimal identity-oriented `Debug`. The
integration tests declare real downstream classes through renamed imports and republish metadata through another
inheritance level.

The expected research macro and runtime dependencies must be available under their current crate names in a downstream
declaration crate. Dependency renaming/versioned protocol support is not part of this experiment.

## 2. Repairing the safe/unsafe boundary

### 2.1 Typed construction, even in the declaring module

The old representation allowed this safe operation:

```rust
Dog::from_raw(animal.raw().clone())
```

The new runtime does not expose a raw-handle constructor. Its construction currency is **`Handle<C>`**, invariant in
`C`, with representation fields private to the runtime crate. Generated wrappers are transparent over that carrier:

```text
Animal { Handle<Animal> } --+
Dog    { Handle<Dog>    } --+--> same Arc-owned concrete allocation
Puppy  { Handle<Puppy>  } --+
```

`Class::from_handle` is safe because it accepts only a validated carrier of exactly the requested static class.
`Handle<C>::new` takes `C::Fields` and obtains C's table internally. It accepts neither an arbitrary table nor a builder
callback. Downcasts create a new typed view only after checking the dynamic object's actual ancestry.

Merely making a generated wrapper's field private would not protect it from its own declaration module. Here, even that
module cannot place `Handle<Animal>` into a `LocalDog`. The UI suite checks both trait reconstruction and direct
same-module struct construction.

### 2.2 Generic identity is invariant

Both the carrier and generated generic handles use invariant markers. There is no covariance-only `PhantomData<fn() ->
T>` escape from erased mutable storage.

The review's higher-ranked function-pointer subtype coercions are rejected for both `Boxed` and `Tagged`:

```text
Boxed<fn(&str)>  -X-> Boxed<fn(&'static str)>
Tagged<fn(&str)> -X-> Tagged<fn(&'static str)>
```

The storage and class bounds independently enforce `Send + Sync + 'static`; a variance marker is not used as a
substitute for those bounds. A non-Send field is a compile error.

### 2.3 Detached thunks are not ordinary safe functions

An inherited `speak` slot may contain a Dog override even though the slot's argument is `&Animal`. The narrowed receiver
requirement is reflected in an **unsafe function-pointer type**.

Safe generated methods obtain the slot from their own validated object's table and call it internally. Extracting a Dog
slot and invoking it on a plain Animal requires unsafe code. Generated private thunks and the runtime's unchecked
borrowed-view helper also require unsafe calls, including from the class's own module.

Published tables are immutable. Clients can clone a table value but cannot install a modified table into an object or
overwrite a cache entry through a safe API. `vtable::<C>()` derives its key, table type and builder from C; callers
cannot supply a mismatched key or malicious builder.

### 2.4 Metadata supplies names, not permission to cast

Both candidates use compile-time descriptors. The main generator must not treat a user-invokable descriptor callback as
a trusted proof of ancestry.

The new generated layout uses the **actual declared parent**:

- storage embeds `<Parent as Class>::Fields`;
- tables embed `<Parent as Class>::VTable`;
- data/building use the parent's associated construction types;
- runtime ancestry calls the actual parent's `Class::is_class`;
- borrowed `Deref` targets that actual parent.

Descriptor records select method names and paths, but ordinary type checking must validate the resulting slot
assignments and safe ancestor conversions. A lying record claiming an unrelated Dog ancestor fails the generated safe
reference coercion. Claiming that an imported final Puppy is open fails the independent `OpenClass` bound.

`Class` is an unsafe implementation contract, not a safe extension point for arbitrary ancestry claims. Its ordinary
methods have no additional unchecked caller preconditions. `ClassData` can only return the actual associated field type.
`OpenClass` is an opt-in marker; manually opting in a locally owned class changes extension policy, not the underlying
`Class` representation contract. Full public-helper sealing remains production work.

## 3. Layout, ownership and the safety argument

Each allocation contains the real most-derived nested storage. An erased `Arc` retains that allocation's actual
destructor and allocation layout. The implementation never converts `Arc<DerivedFields>` into `Arc<BaseFields>`.

The carrier retains:

1. an `Arc<dyn Storage>` owner;
2. a pointer originating from the live concrete fields;
3. a pointer originating from the concrete immutable table.

Generated storage/table types have `repr(C)` and put the actual parent prefix first. Generated handles and the carrier
are transparent; additional invariant markers are alignment-one zero-sized fields. Shared borrowed ancestor views
preserve the original handle lifetime. There is no whole-object `DerefMut`, integer-pointer reconstruction, or temporary
owner hidden behind a borrowed result.

Field access uses shared references into `RwLock` fields. The getter/setter poison-recovery policy matches the
descriptor spike. Registry poisoning, in contrast, is an explicit panic. No registry lock is held while building a
parent table or executing a user's virtual body.

### Unsafe inventory

| Operation                        | Required invariant and enforcement                                                                                                                             |
| -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Raw carrier `Send`/`Sync`        | Owner and fields are Send + Sync; tables are immutable; pointers/owner cannot be independently replaced through safe APIs                                      |
| Typed field/table pointer reads  | Carrier identity and actual `repr(C)` ancestry establish the initialized, aligned prefix; owner/cache retain its lifetime                                      |
| Borrowed handle reinterpretation | Both handles are transparent over the same representation; dynamic compatibility is checked or established by actual inheritance/table construction            |
| Generated `Class` implementation | Actual parent layout, accurate nominal ancestry, invariant generic identities, and compatible slots; explicit unsafe contract for hand-written implementations |
| Override thunk                   | Receiver belongs to the table's class or a descendant; detached calls require unsafe; the borrowed view cannot outlive its input                               |
| Dispatch and lexical `super`     | Select a slot from the validated object's dynamic table or its cached lexical-parent table while retaining the original receiver                               |

The templates never wrap user method bodies in an unsafe block. Inspecting expanded code confirmed separate safe bodies,
unsafe thunks, narrowly scoped calls and lifetime-preserving async signatures. The handwritten model/test sources forbid
unsafe code; that lint is **not** evidence that generated code is safe. The descriptor expansion was separately
inspected and contains no `unsafe` keyword.

This is a soundness argument for the supported representation and generated surface, supported by tests. It is not a
formal verification of every template, every possible macro input, or arbitrary user-written unsafe trait
implementations.

## 4. Lexical super without a hot-path registry lookup

Each subclass table embeds its parent's table and retains a separate immutable reference to that parent's original
table. An inherited copy retains the declaring class's own parent reference.

For example, the generated generic override executes:

```rust
unsafe { (self.__handle.table().parent.get)(self) }
```

There is no registry lookup in that call and no owning downcast/refcount operation. An ancestor body still sees the
original most-derived receiver, so a virtual call made inside `super` remains late-bound.

This matters at more than one inheritance level: using the dynamic object's immediate parent for every `super` would
select the wrong body or recurse. Both the shared parity tests and the deeper callback/weight scenario exercise lexical
selection.

The cache publishes one table per concrete class/type instantiation. Builds occur outside its lock, and racing losers
are dropped instead of leaked. Tables remain reachable from the registry for the process lifetime.

**Trade-off:** this small implementation uses the same registry for non-generic classes too. Thus construction still
acquires a global cache lock and is not contention-free. Only ordinary dispatch and lexical `super` avoid that lookup.
Specializing non-generic construction to statics is left for a later measured optimization.

## 5. Async methods

The shared lifetime utility lowers a virtual async slot to a form such as:

```rust
for<'f> unsafe fn(
  &'f AsyncAnimal,
  &'f str,
) -> Pin<Box<dyn Future<Output = Result<String, &'static str>> + Send + 'f>>
```

The thunk obtains a borrowed typed receiver with that same lifetime and boxes the user's async body. It does not create
an owning temporary handle whose reference could escape. The public wrapper awaits the returned future normally.

Elided and `'_` captured input lifetimes follow the future lifetime. Function-pointer input types keep their own
lifetime binders. `&mut String` stays exclusively borrowed until completion or cancellation. The compiler rejects an
input or receiver being dropped while its future is still used.

Tests actually return `Poll::Pending`; they are not only immediately-ready async wrappers. They cover:

- overrides and inherited overrides across crates, awaited `super`, and a base async body calling an overridden sync
  virtual;
- `Option<&str>`, `&mut String`, `fn(&str) -> String`, unit and generic returns, and `Result`/`?`;
- lazy observation of a changed field;
- moving an already suspended borrowed future to another scoped thread;
- Tokio multi-threaded spawning with an owned handle and input;
- dropping unpolled and suspended futures, including a retained payload after an awaited parent call;
- panics while polling and subsequent use of the still-valid object.

Each entered async virtual body has a boxed future; forwarding an inherited slot does not box once per ancestor. Awaited
`super` creates its parent's body future as well. Cancellation releases borrows and owned future values; it does not
roll back earlier side effects. These are Send futures, not necessarily `'static` futures, and their outputs need not
satisfy Tokio's additional Send requirement unless spawned there.

No async timing or allocation-count study was performed. The measurements below are **synchronous only**.

## 6. Test evidence and Taskfile integration

| Evidence                                                                                     | Result                                                                                                                                                                                     |
| -------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| [Shared inheritance scenarios](../../projects/galvanize-rs/research/inheritance-tests.rs)    | Four runtime tests: renamed/cross-crate ancestors, nested generic substitution, casts/identity, mutation/threads, last-owner destruction                                                   |
| [Async tests](../../projects/galvanize-rs/research/main-models/tests/async_inheritance.rs)   | Five deterministic runtime tests plus one native Tokio test                                                                                                                                |
| [Soundness regressions](../../projects/galvanize-rs/research/main-models/tests/soundness.rs) | Four tests: checked carriers/views, completely empty hierarchies, aligned and zero-sized payloads, different sibling layouts, reentrancy/unwinding, cold cache races, mutation/destruction |
| [UI fixtures](../../projects/galvanize-rs/research/main-models/tests/ui/)                    | Twelve fixtures pass against checked-in diagnostics, including both asyncness mismatch directions                                                                                          |
| `task miri:main`                                                                             | All thirteen runtime tests pass, both models, strict provenance, eight seeds each                                                                                                          |
| Randomized-layout `task miri:main`                                                           | Same suite passes, both models and eight seeds                                                                                                                                             |
| Default `task miri`                                                                          | New suite, historical standalone spike, and all four benchmark startup `race`/`verify` checks pass                                                                                         |
| Descriptor regression suite                                                                  | Four async tests, four inheritance tests and its seven existing UI fixtures pass                                                                                                           |
| Production class regressions                                                                 | 25 class tests and the existing five-fixture UI harness pass                                                                                                                               |
| Targeted Clippy                                                                              | Research crates/all targets and benchmark code checked with warnings denied                                                                                                                |

The UI tests correspond directly to the boundaries:

- `forgery`: wrong carrier, same-module wrapper fields, removed raw reconstruction, arbitrary table/builder
  installation;
- `detached_slot`, `unchecked_helpers`, `safe_class_impl`: unchecked dispatch/view/implementation operations cannot be
  invoked or implemented as safe operations;
- `false_ancestry`, `forged_descriptor`: metadata cannot authorize an unrelated cast or make an imported final class
  open;
- `variance`: both higher-ranked function-pointer coercions from the review;
- `future_lifetimes`, `non_send_storage`, `async_not_send`: retained borrows and thread-safety obligations;
- `overrides`, `unsupported`: signature/asyncness/final checks, unsupported virtual forms and generated-member
  collisions.

The [Taskfile](../../Taskfile.yaml) exposes `miri:main` and includes it in no-argument `miri`. Supplying a standalone
`.rs` argument retains the previous standalone-only behavior. Both paths reuse the pinned nightly and existing
`miri:run` flags.

Compiler-driving trybuild tests do not run inside Miri. Tokio spawning runs natively; the same unsafe async dispatch
paths are exercised under Miri through deterministic polling, including actual suspension and cross-thread migration. No
aliasing checks or provenance checks were disabled, and no known-UB case was executed natively.

The historical probe still passes because its exercised paths are valid. The review already established that its broader
safe API is not. Retaining that check does not weaken the new suite's acceptance criteria or repair the old API.

## 7. Measurements

Linux x86-64, Intel i7-13800H, stable Rust **1.100.0** (`0ef1bc386`), normal optimized benchmark profile. Measurements
ran without concurrent validation commands, but on a shared, non-CPU-pinned development machine. Values are one run's
medians, not confidence intervals or portable promises. `MainDesign` below means the **legacy known-unsound** candidate.

### Single thread

500 samples, 256 iterations per sample:

| Operation                         | Legacy main | Handwritten safe | Descriptors | Hardened main |
| --------------------------------- | ----------- | ---------------- | ----------- | ------------- |
| Construct Dog                     | 36.74 ns    | 36.36 ns         | 42.63 ns    | 54.17 ns      |
| Clone Puppy                       | 5.287 ns    | 12.75 ns         | 13.33 ns    | 5.404 ns      |
| Inherited Puppy `legs`            | 0.896 ns    | 0.990 ns         | 0.978 ns    | 0.771 ns      |
| Dog `weight` override             | 0.982 ns    | 23.93 ns         | 23.86 ns    | 0.959 ns      |
| Puppy `weight` plus `super`       | 1.810 ns    | 57.05 ns         | 58.12 ns    | 1.600 ns      |
| Dog's Animal view downcast to Dog | 4.795 ns    | 14.39 ns         | 17.38 ns    | 5.092 ns      |
| Construct Tagged                  | 38.89 ns    | 19.81 ns         | 19.79 ns    | 37.97 ns      |
| Tagged `get` plus `super`         | 24.58 ns    | 36.96 ns         | 36.82 ns    | 11.48 ns      |

The construction cost is an explicit regression relative to the legacy non-generic static-table path, not something the
dispatch results excuse. Conversely, typed construction and invariant handles do not require a checked owning downcast
on every virtual call.

### Eight threads, shared objects

200 samples, 2,048 iterations per sample:

| Operation                   | Legacy main | Handwritten safe | Descriptors | Hardened main |
| --------------------------- | ----------- | ---------------- | ----------- | ------------- |
| Dog `weight`                | 1.070 ns    | 350.8 ns         | 323.5 ns    | 1.166 ns      |
| Puppy `weight` plus `super` | 1.858 ns    | 885.9 ns         | 924.9 ns    | 1.872 ns      |
| Tagged `get` plus `super`   | 494.9 ns    | 565.2 ns         | 533.2 ns    | 212.1 ns      |

These are Divan per-iteration statistics, not aggregate throughput. Shared generic reads still contend on the field's
RwLock even though the new `super` no longer consults the registry.

### Independent generic objects

The harness times batches of **64 reads**. The following values divide the reported Tagged medians by 64:

| Threads                          | Legacy main    | Handwritten safe | Descriptors   | Hardened main |
| -------------------------------- | -------------- | ---------------- | ------------- | ------------- |
| One (500 samples, 256 batches)   | 24.66 ns/call  | 35.47 ns/call    | 34.95 ns/call | 10.04 ns/call |
| Eight (200 samples, 128 batches) | 489.06 ns/call | 40.89 ns/call    | 42.09 ns/call | 10.74 ns/call |

The eight-thread raw medians were 31.3 us, 2.617 us, 2.694 us and 687.3 ns per batch respectively. This directly
exercises the review's independent-object contention case. Cached lexical parent tables remove the specific registry
traffic; they do not establish that arbitrary inheritance workloads have flat costs.

### Size and allocations

The runnable example reports **32 bytes / four machine words** for Animal, Dog, Puppy and Tagged on this target,
independent of hierarchy depth. Cloning retains one owning Arc rather than one per ancestor.

The warmed construction samples report two allocations for named Dog objects (the hierarchy allocation and name string)
and one for Tagged<u64>. Hardened Dog requested 107 bytes total and Tagged requested 64, matching the corresponding safe
fixtures; the old main requested 115 and 72. The new allocation does not store the redundant vtable header word: the
carrier already retains the table pointer. These counts exclude cold registry/table initialization.

The generated fixtures intentionally use bare RwLock fields for comparison, not production `Arc<RwLock<_>>` field
strategies. These measurements do not establish production end-to-end memory or construction costs.

## 8. Boundaries and recommendation

This remains a research API. It matches the descriptor spike's implemented surface, not every feature in the original
proposal:

- Exported declarations remain at crate root, with one paired methods block and explicit parent generic arguments.
- Only type generics, with Clone + Send + Sync + 'static bounds, are supported.
- Virtual methods are public, safe, non-generic `&self` methods. No supported design for method lifetime generics,
  borrowed-return or Self-dependent virtual APIs, abstract methods, protected access or virtual accessors is claimed.
- There are no configurable production field strategies, local-only futures, field-rich dynamic Debug, or heterogeneous
  equality/hash APIs.
- Hand-written unsafe Class implementations remain the author's safety responsibility. The experimental
  helper/descriptor protocol is not versioned or fully sealed.
- Lock poisoning recovery matches the safe prototype; blocking field locks are not an async synchronization abstraction.
  Reentrant virtual dispatch is exercised, not arbitrary reentrant user Clone/Drop behavior while a field lock is held.
- User-created strong cycles can leak. The vtable cache intentionally retains each instantiated class table for the
  process lifetime. Cold builds/registry memory growth are not free.
- Flat handles do not imply flat ancestry checks, constant-cost super chains, or constant total table/code size. Copying
  ancestor tables can still yield quadratic aggregate slot storage across deep hierarchies.
- No extra architecture/target matrix, exhaustive concurrency proof, async benchmark study, or CI Miri job was added.

The main candidate now has a substantially stronger basis for further evaluation: real macro boundaries, explicit
contracts, safe-client rejection tests, and Miri coverage of generated sync/async behavior. Its small-handle and
borrowed dispatch advantages survive the repairs, while generic-super contention improves.

The safe descriptor candidate still avoids this unsafe maintenance obligation altogether. Syntax is now comparable. The
choice should therefore depend on application workloads, construction/dispatch trade-offs and willingness to audit the
unsafe kernel and templates, not on the original claim that ergonomics require unsafe.

## 9. Reproduce

From the repository root, through Pixi. Native tests use the pinned stable toolchain; Miri uses **nightly-2026-11-19**.

```sh
pixi run --locked cargo run --locked -p galvanize-rs-main-models --example hardened-inheritance

# Native parity, async compatibility, and UI diagnostics.
pixi run --locked cargo test --locked -p galvanize-rs-main-runtime -p galvanize-rs-main-macros -p galvanize-rs-main-models -p galvanize-rs-inheritance-macro-support -p galvanize-rs-descriptor-models
pixi run --locked cargo test --locked -p galvanize-rs --test class --test class_ui --bench inheritance

# Actual generated runtime executions, both models and eight seeds.
pixi run --locked task miri:main
pixi run --locked env RUSTFLAGS=-Zrandomize-layout task miri:main
pixi run --locked task miri

# Four-design synchronous measurements.
pixi run --locked cargo bench --locked -p galvanize-rs --bench inheritance -- --threads 1 --sample-count 500 --sample-size 256
pixi run --locked cargo bench --locked -p galvanize-rs --bench inheritance -- weight generic_get --skip generic_get_independent --threads 8 --sample-count 200 --sample-size 2048
pixi run --locked cargo bench --locked -p galvanize-rs --bench inheritance -- generic_get_independent --threads 8 --sample-count 200 --sample-size 128

# Inspect actual expansions; this is not required for normal stable builds.
pixi run --locked cargo +nightly-2026-11-19 rustc -p galvanize-rs-main-models --lib -- -Zunpretty=expanded
pixi run --locked cargo +nightly-2026-11-19 rustc -p galvanize-rs-descriptor-models --lib -- -Zunpretty=expanded
```
