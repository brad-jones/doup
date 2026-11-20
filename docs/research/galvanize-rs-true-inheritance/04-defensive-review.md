# True Inheritance: A Defensive Review of the Recommendation

> **Archived research.** Part of the [true inheritance research catalogue](README.md), which led to the
> `SafeRuntimeStrategy` and `UnsafeRuntimeStrategy` class runtimes in galvanize-rs. Code links point at commit
> `79bde36`, the last to contain the research crates, and the commands below assume a checkout of that commit.

Date: **2026-11-19**. Reviewed revision: `ca04f04bc07651007f4b64597dfe31915c0b2654`.

Status: **Research review, not an implementation approval**.

This challenges the conclusions in [the comparison](03-comparison.md), using [the main design](01-original-design.md),
[the safe design](02-safe-variant.md), their executable prototypes, and additional experiments. The original documents
and implementations were left unchanged. The new [review spike](spikes/review-spike.rs) imports the original
implementations directly.

## Executive verdict

**The author picked a sensible candidate, but gave it more assurance than the evidence supports.**

The prefix-layout approach is credible, and the main design's advantage for small, non-generic overrides is real.
Neither fact establishes a sound public API, or establishes that unsafe is necessary to obtain the desired syntax.

Most importantly, **both unsafe prototypes permit undefined behaviour through entirely safe client code**. Their
existing Miri checks pass; a new client using their public trait methods fails under both Stacked and Tree Borrows.
There is also a separately demonstrated generic-variance hole in the claimed type-identity invariant.

There is a significant performance qualification, too: **the main design's global generic-vtable registry can cause
contention between completely independent objects**. In the additional eight-thread experiment, the safe implementation
was roughly an order of magnitude faster. This does not invalidate the non-generic results, but it contradicts an
unqualified claim that main-design calls write nothing shared.

My recommendation:

> Keep the main design as an experimental candidate, not the approved default. Harden and test the safe/unsafe API
> boundary and generated code first. Compare it with a safe design using generated ancestry metadata before treating
> ergonomics as the deciding argument. Do not ship either research prototype as production infrastructure.

This is **not** evidence that prefix-layout inheritance is fundamentally impossible or that these bugs are unfixable. It
is evidence that "a small fixed template plus Miri" is not yet a sufficient justification.

## 1. Scope and evidence

The shipped [class runtime](https://github.com/brad-jones/doup/blob/79bde36/projects/galvanize-rs/src/class/mod.rs) and
[macro](https://github.com/brad-jones/doup/blob/79bde36/projects/galvanize-rs/macros/src/class.rs) still implement
composition-based inheritance. They do not implement the proposed `RawHandle` object model or `#[class_methods]`. **The
soundness findings below concern research prototypes and the proposed design, not a demonstrated flaw in the current
shipped class API.**

Reviewed executable evidence:

- [Original unsafe spike](spikes/original-spike.rs).
- [Main benchmark implementation](spikes/bench-forks/main_design.rs).
- [Safe benchmark implementation](spikes/bench-forks/safe_design.rs).
- [Divan harness](https://github.com/brad-jones/doup/blob/79bde36/projects/galvanize-rs/benches/inheritance.rs).
- [Taskfile](https://github.com/brad-jones/doup/blob/79bde36/Taskfile.yaml), current class machinery, pointer
  implementation, and CI workflows.

New tests ran on Linux x86-64, an Intel i7-13800H, Rust **1.100.0**, and the project's pinned **nightly-2026-11-19**
Miri toolchain, using Pixi. No dependencies or toolchain pins were changed.

| Check                                                             | Result                                                                             |
| ----------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| Existing `task miri`: original spike and benchmark startup checks | Pass, both aliasing models, eight seeds each                                       |
| New valid-use checks                                              | Pass, both models, eight seeds each                                                |
| Valid-use checks with `-Zrandomize-layout`                        | Pass, both models, eight seeds each                                                |
| Safe client forges `Dog` from a plain `Animal`, original spike    | **UB**, both models                                                                |
| Same forgery, benchmark implementation                            | **UB**, both models                                                                |
| Generic function-pointer subtype coercion                         | Accepted; contradicts the handle's own nominal type identity                       |
| Same generic coercion in the safe implementation                  | Rejected with E0308                                                                |
| Variance probe under Miri                                         | Reproduces the invariant failure without a UB diagnostic, both models, eight seeds |
| Current class integration tests                                   | 25 pass; trybuild harness and its five compile-fail fixtures pass                  |
| Existing dispatch benchmarks and two new timing workloads         | Completed; results discussed below                                                 |

The valid-use additions cover retaining only a base view after dropping derived handles, successful and failed casts,
sibling/unrelated-type rejection, a 128-byte-aligned generic payload, a zero-sized payload, exactly-once payload
destruction, and dispatch after transfer to another thread. A zero-sized _payload_ is not a test of a completely
zero-sized storage/vtable hierarchy.

The new spike's **client module forbids unsafe code**. Its imports deliberately contain the library-side unsafe code
under review. The two forgery modes refuse to run natively, so accidental execution does not perform native UB.

## 2. Findings, ordered by significance

| Finding                                                                   | Classification                                                 | Evidence                                                               |
| ------------------------------------------------------------------------- | -------------------------------------------------------------- | ---------------------------------------------------------------------- |
| F1. Safe raw-handle reconstruction bypasses type checks                   | **Release-blocking soundness defect**                          | UB reproduced against both unchanged unsafe prototypes                 |
| F2. Generic handles are covariant despite type-erased mutable storage     | **Broken safety invariant; must resolve before generics ship** | Safe coercion and incorrect nominal type identity reproduced           |
| F3. Generated-code privacy and callable vtable slots need a real boundary | **Unresolved design obligation**                               | API/layout inspection; no actual Option B macro exists to validate     |
| F4. Generic `super` creates global contention                             | **Material counterexample to the performance summary**         | Separate-object multithreaded benchmark                                |
| F5. Syntax advantage is not inherently an unsafe advantage                | **Incomplete architectural comparison**                        | Both design documents already identify metadata as an alternative      |
| F6. Coverage and CI do not support the stated level of assurance          | **Validation gap**                                             | Passing baseline versus failing adversarial tests; workflow inspection |

### F1. `unsafe trait` does not make its methods unsafe to call

Both unsafe prototypes expose this shape:

```rust
pub unsafe trait Class: Clone + 'static {
  type Fields: Storage;
  fn raw(&self) -> &RawHandle;
  fn from_raw(raw: RawHandle) -> Self;
  // ...
}
```

The `unsafe` qualifier imposes obligations on **implementors of the trait**. It does not impose a safety obligation on
callers of ordinary safe methods.

This client is accepted without unsafe:

```rust
use main_design::{Animal, Class, Dog};

let animal = Animal::new("Not a Dog");
assert!(animal.downcast::<Dog>().is_none());
let dog = Dog::from_raw(animal.raw().clone());
dog.bark_volume();
```

The legitimate downcast correctly says no. The public reconstruction method then makes exactly the invalid handle that
the type check was meant to prevent. Its vtable still belongs to an `Animal`; using it as a `Dog` vtable reads a
nonexistent slot.

Observed Miri results:

- **Original spike, both models:** an invalid `&__DogVTable<__AnimalVTable>` extending beyond its allocation.
- **Benchmark, Stacked Borrows:** invalid retag when extending the accessible Animal-vtable region to a Dog vtable.
- **Benchmark, Tree Borrows:** an uninitialized-memory read when fetching `bark_volume`.

The benchmark's vtable lives inside a `LazyLock`, which explains why its diagnostic differs from the original
heap-allocated vtable. The fundamental error is the same.

This is not a malicious handwritten `unsafe impl Class`, and not a caller breaking an unsafe-function contract. It uses
the supplied implementations and safe methods from a sibling module.

**Required direction:** make unchecked reconstruction an explicitly unsafe operation with a precise contract, or make
the safe constructor check compatibility and reject the mismatch. Then ensure tuple constructors and other generated
helpers cannot bypass that boundary. A hidden-doc attribute, obscure name, or sealed trait alone does not make a safe
method with unchecked preconditions sound.

This fix need not slow ordinary virtual calls: validation belongs at handle creation/reconstruction, not necessarily on
every dispatch.

### F2. `'static` does not eliminate subtyping

The main benchmark represents both generic handles using:

```rust
PhantomData<fn() -> T>
```

That marker is **covariant** in `T`. The actual `RwLock<T>` is hidden behind erased pointers, so its normal invariance
does not propagate into the public handle.

These are distinct types, both satisfying the prototype's bounds:

```rust
type AnyLifetime = fn(&str);
type StaticOnly = fn(&'static str);
```

An `AnyLifetime` function can accept a static string, so it is a subtype of `StaticOnly`. Consequently this compiles:

```rust
let original = main_design::Boxed::<AnyLifetime>::new(|_| {});
let widened: main_design::Boxed<StaticOnly> = original;
```

But the actual storage and vtable remain those of `Boxed<AnyLifetime>`. The new probe verifies:

- `widened.is::<Boxed<AnyLifetime>>()` is true;
- `widened.is::<Boxed<StaticOnly>>()` is false;
- downcasting to its own static class returns `None`.

The same problem reproduces for `Tagged`. The safe implementation rejects the corresponding coercion because it retains
the type relationship rather than erasing it.

**Important distinction:** this probe demonstrates a broken invariant, not a second independently demonstrated UB
execution. Its getter calls also run under Miri without a diagnostic. The existing generic benchmark only exposes
reading/cloning `T`; a getter-only covariance example is not enough to claim a dangling-reference exploit.

Nevertheless, the proposed classes are mutable. Adding the intended typed setters to a covariantly erased handle can let
a widened alias install a `StaticOnly` callback into storage another alias treats as `AnyLifetime`. Such a callback
could safely retain its allegedly static input. That is precisely the kind of lifetime guarantee the representation must
not erase.

**Required direction:** start with invariant generic handles, for example an appropriately audited `PhantomData<fn(T) ->
T>` marker, and compile-fail tests for higher-ranked function-pointer subtyping. Audit strategy parameters and every
other parameter participating in storage/vtable identity too. Separately check auto-trait and drop-check behaviour;
choosing a variance marker is not the whole generic soundness argument.

The standard library's
[TypeId documentation](https://doc.rust-lang.org/std/any/struct.TypeId.html#danger-of-improper-variance) explicitly
warns about this exact `'static` subtyping trap.

### F3. Fixed generated templates still need encapsulation

There are two boundaries the hand-expanded benchmark does not establish for the eventual macro.

**First: generated types live somewhere.** If a proc macro emits `struct Dog(RawHandle)` directly in the user's module,
that module can use its private tuple constructor. Changing `Class::from_raw` to unsafe would not prevent safe code in
that same module from writing `Dog(animal.raw().clone())`.

The current composition macro emits its handle in the surrounding module. That is not itself an unsafe bug in the
current design, but mechanically copying that placement into the new representation would be dangerous.

The new representation needs a real privacy boundary, such as a generated implementation module with controlled exports,
or a representation whose accessible construction operations cannot produce mismatched handles. Cross-crate subclassing
and a separately expanded `#[class_methods]` must both work through that boundary.

**Second: an unchecked thunk is not an arbitrary safe function.** The proposed public slot:

```rust
pub speak: fn(&Animal) -> String
```

may hold a Dog override that unchecked-casts its argument to `&Dog`. The type says it accepts any `&Animal`; the
implementation actually requires a Dog or descendant.

The statement "only Dog objects carry this vtable" is insufficient if safe callers can obtain the slot and invoke it
independently with a plain Animal. In the benchmark, the vtable getters/statics are private, so this is **not a separate
public-slot exploit demonstrated against that sibling-module API**. But the real design needs cross-crate access to
parent vtables and override slots, and the proposed types expose public function-pointer fields.

Possible solutions include unsafe slot types/operations with explicit receiver contracts, tightly encapsulated dispatch,
or checked entry points. Whichever is chosen must protect both **installing a vtable** and **calling one**.

Treat the following as required contracts, not comments saying "the macro does it":

- The owner, fields pointer, concrete class, and vtable describe the same allocation and compatible hierarchy.
- Every reported ancestor corresponds to an actual prefix, with compatible alignment and initialized contents.
- Handle reinterpretation preserves validity, lifetime, and generic identity, not merely byte size.
- Unchecked thunks only receive compatible dynamic objects, including through `super`.
- No safe reconstruction, field access, helper, or same-module constructor can violate these conditions.
- Field access exposes only the intended synchronized interior mutation, not mutable access to an entire aliased
  base/storage/handle.
- Erased storage and all reachable data justify the manual `Send`/`Sync` implementations.

### F4. Generic `super` contends globally, even without shared objects

The comparison correctly identifies shared refcounts as a weakness of safe overrides. It overlooks an equally important
qualification to its main-design conclusion.

In the main implementation, `Tagged<T>::super_get()` calls `Boxed::<T>::vtable()`. That performs a read-lock acquisition
and lookup in the **one global registry on every call**. A read lock still updates shared synchronization state. Those
updates can contend even when every thread owns a completely different object.

The new `generic-timing` mode gives each worker its own `Tagged<u64>`, warms it before a barrier, and times 100,000
calls. There is no shared object/refcount or field lock between workers. It alternates which design runs first.

Across two executions, three trials per configuration:

| Separate objects, `Tagged<u64>::get()` | Main              | Safe              |
| -------------------------------------- | ----------------- | ----------------- |
| One thread                             | 20.4–36.5 ns/call | 33.0–36.1 ns/call |
| Eight threads                          | 560–676 ns/call   | 41.7–60.3 ns/call |

Each trial reports the upper median of the workers' average elapsed time per call, **not aggregate throughput or
individual-call latency percentiles**. This shared development machine was not CPU-pinned; the ranges are observations,
not portable guarantees. The direction and large size of the reversal repeated.

The main document already mentions caching parent-vtable references. That is worth investigating before accepting this
cost. It must preserve _lexical_ `super` semantics across multiple overrides, not accidentally use the most-derived
object's immediate parent for every body.

This result does not say the safe design always scales better. It says there are at least two distinct scenarios:

1. Shared non-generic objects: the safe design's repeated refcounts hurt.
2. Independent generic objects using `super`: the main design's shared registry hurts.

A production decision needs both, not a universal "main calls write nothing shared" assertion.

### F5. The syntax comparison excludes a documented alternative

The recommendation's decisive claim is that unsafe buys `extends = Dog` and bare `#[override]`. More precisely, **this
particular no-metadata unsafe implementation buys those forms**.

The safe document's variations explicitly say descriptors can provide the ancestor chain **and** the declaring class.
That removes both extra annotations without requiring the unsafe object representation.

The actual trade space is:

| Candidate                                     | Simple annotations | Main extra cost                                                    |
| --------------------------------------------- | ------------------ | ------------------------------------------------------------------ |
| Unsafe prefix layout, inferred slot selection | Yes                | Soundness/encapsulation work, registry, unsafe maintenance         |
| Safe layout, explicit ancestry                | No                 | User-maintained annotations, larger handles and override refcounts |
| Safe layout, generated ancestry descriptors   | Potentially yes    | Metadata hygiene, diagnostics, cross-crate support, compile time   |

The third option is not proven production-ready by these documents, and I did not implement a full descriptor macro in
this review. It deserves a bounded cross-crate spike before rejecting it. Metadata is also useful for the main design's
abstract checks and const-vtable construction, so this is not simply "complexity only the safe design would need".

It is reasonable to decide descriptor machinery costs more than a reviewed unsafe kernel. It is not reasonable to say
the language-like syntax itself establishes that result without testing either generated implementation.

An intermediate option also deserves attention: a checked **borrowed** downcast/view. A four-word owning handle does not
intrinsically require cloning an `Arc` just to validate a borrowed receiver. That option still needs unsafe in its
implementation, but the comparison should not equate every checked view with the owned-downcast benchmark cost.

### F6. Passing Miri is necessary evidence, not an acceptance criterion

The original documents do acknowledge that only exercised paths are checked. The high-level conclusion nevertheless goes
too far when it says Miri and trybuild should catch a regression before it ships.

This review provides a direct counterexample: **the baseline passed immediately before a safe client demonstrated UB
against the same implementations**. Eight scheduling seeds do not generate new API sequences, class declarations, type
parameters, or module boundaries.

The [Miri documentation](https://github.com/rust-lang/miri#readme) explicitly distinguishes checking executions from
establishing soundness for arbitrary safe callers. Testing both aliasing models is useful; agreement is not a formal
proof or exhaustive concurrency exploration.

Also, at the reviewed revision:

- [The Taskfile](https://github.com/brad-jones/doup/blob/79bde36/Taskfile.yaml) provides reproducible local Miri checks.
- [PR checks](https://github.com/brad-jones/doup/blob/79bde36/.github/workflows/pr-checks.yaml) run ordinary
  unit/integration tests and linting, not Miri.
- [The CI workflow](https://github.com/brad-jones/doup/blob/79bde36/.github/workflows/ci.yaml) also does not invoke
  Miri.
- Existing trybuild fixtures cover the current composition macro, not an implemented Option B macro.
- Under Miri, the inheritance benchmark exercises its explicit `race`/`verify` startup checks, not timed benchmark
  bodies.

Thus "keep Miri in CI" is a future requirement, not an existing safeguard. Even reaching every unsafe source location is
weaker than testing all the invariants of those locations.

## 3. What survives the devil's-advocate review

There is a credible low-level design here:

- `#[repr(C)]` with the actual parent storage as the first field provides the prefix offset and alignment relationship.
  This follows the [Rust layout rules](https://doc.rust-lang.org/reference/type-layout.html#the-c-representation), not
  merely an observation about one compiler.
- Transparent wrappers over the same `RawHandle` can support borrowed ancestor views without manufacturing a new owner.
- The owner stays an `Arc<dyn Storage>` for the real allocation. The implementation does **not** pretend an
  `Arc<DerivedFields>` is an `Arc<BaseFields>`, which would jeopardize destruction and allocation layout.
- Pointers originate from the live full storage and static vtable, rather than from integer-address reconstruction.
- Storage access uses shared references and synchronized interior mutation; there is no inherited `DerefMut`.
- Immutable vtables and explicit `Send + Sync` storage are a plausible basis for thread-safe handles.

The additional lifetime/drop/alignment tests support these choices on the tested paths. They did not find a fundamental
prefix-layout or owner-lifetime failure.

Servo and GObject are useful precedents for embedding a parent prefix. They do not validate this design's generic
variance, erased-owner coupling, safe constructors, proc-macro privacy, or callable Rust function-pointer contracts.
Those are independent parts of its soundness argument.

## 4. Reassessing the measurements and extrapolations

### 4.1 The non-generic dispatch advantage is real

Re-running the existing Divan harness with fixed sample sizes produced these medians:

| Operation                            | Main, one thread | Safe, one thread | Main, eight threads | Safe, eight threads |
| ------------------------------------ | ---------------- | ---------------- | ------------------- | ------------------- |
| Dog `weight` override                | 1.04 ns          | 27.5 ns          | 0.94 ns             | 286 ns              |
| Puppy `weight` override plus `super` | 1.90 ns          | 59.3 ns          | 1.66 ns             | 920 ns              |
| Generic Tagged `get` plus `super`    | 27.4 ns          | 40.9 ns          | 522 ns              | 490 ns              |

The eight-thread rows here share an object, as the existing harness intends. Treat them as the harness's reported
per-iteration timing statistics, not a promised speedup or whole-application throughput.

The order of magnitude in the original single-thread comparison is credible. Its "most rows rise by 10–40%" summary does
not describe these particular allocation-free override rows with longer fixed samples. Contention is potentially much
more consequential than the original prose suggests.

### 4.2 Rust is not immune to mixed dynamic targets

The existing Rust harness benchmarks one concrete dynamic class per call site at a time. Its type parameters and
`black_box` do not turn that into a randomized mixture of receiver classes.

The new `timing` mode instead walks 1,024 base views repeatedly, comparing all-Dog objects with an equal,
deterministically shuffled mixture of Animal, Dog, Puppy, and Beagle. It checks the resulting sums and excludes
construction/destruction from timing.

In its first execution, the main implementation's median of five trials changed from **1.06 ns/call** to **2.03
ns/call**; the safe implementation was **30.21** versus **30.58 ns/call**. A second execution was noisier: main **0.97**
versus **3.81 ns/call**, safe **25.46** versus **31.03 ns/call**.

These are different object mixes, not a controlled experiment attributing every nanosecond to branch prediction. They do
show that the original "Rust's costs don't change" assertion has not been established. Target prediction, method bodies,
object layout, and cache locality still matter in native code.

The main design remains faster in this workload. The correction is to the universality of the nanosecond claim, not to
the existence of an advantage.

### 4.3 The forks do not measure the proposed field strategies faithfully

Both benchmark forks embed bare `RwLock<T>` fields. The actual default
[ArcRwLockPointer](https://github.com/brad-jones/doup/blob/79bde36/projects/galvanize-rs/src/pointers/arc_rw_lock_pointer.rs)
wraps **`Arc<RwLock<T>>`**, and the current macro constructs a pointer for each field.

If the new design preserves those strategies as proposed, "one allocation per object" means one **hierarchy-owner**
allocation, not one total allocation: fields and values such as strings may allocate separately. It also introduces
different indirection/cache behaviour from these forks.

This affects both candidates, so it need not reverse their dispatch ranking. It does mean the construction numbers,
field-read numbers, memory model diagrams, and realistic-workload extrapolations are not end-to-end measurements of the
promised generated classes.

### 4.4 Not every main-design cost is flat

Flat handle size and a direct lookup of a known virtual slot are genuine advantages. The broader table entry "costs
scale flat with hierarchy depth" is too broad:

- The implementation's runtime `TypeId` ancestry check can traverse the hierarchy.
- A `super` chain executes the participating bodies; its cost cannot be independent of their number.
- Building a class table copies its inherited table.
- If each of N classes in a chain declares one new virtual slot, the main design's copied tables contain `1 + 2 + ... +
  N` slots overall: **quadratic total slot storage**, despite there being only one table per class.

Likewise, the safe design's quadratic count of ancestor trait implementations is not a direct measurement of final
binary size: optimization and code folding matter. Neither implementation has a measured deep-hierarchy/code-size study
here.

The blanket "everything else costs the same" claim should also be withdrawn: the comparison's own source tables show
different root-handle sizes, cloning, downcasts, and generic construction costs.

### 4.5 Language comparisons and application budgets remain illustrative

No Java, C#, or Kotlin implementation was measured. A JavaScriptCore result cannot establish their performance. The
repository has no retained Python/JavaScript benchmark harness beside these Rust experiments to verify matching work,
warm-up, escaping allocations, or object mixes. I did not independently reproduce those language numbers.

Multiplying a hot-call microbenchmark by 50,000 is useful budget arithmetic, but it is not a UI-frame benchmark. A
collection of distinct cold objects, contention, allocation, callbacks, and useful body work may change the result.

The user's escape-hatch argument remains good: hot paths can use idiomatic Rust. But migrating a hot path may require
extracting data from this object model and changing dispatch boundaries, not just rewriting a function body. Conversely,
application-level benefit is required before treating sub-nanosecond differences as an adoption criterion.

Finally, allowing generated unsafe under `#![forbid(unsafe_code)]` is a **lint/span behaviour**, not an assurance about
the dependency's soundness. It should not be presented as reducing the adopter's audit burden.

## 5. Acceptance gates before choosing or shipping the main design

I would require the following, in order:

1. **Close construction and dispatch escape hatches.** Fix F1, define generic invariance, and write contracts for the
   erased representation. Test both safe rejections and legitimate upcasts/downcasts. Do not merely add comments to the
   current unchecked safe methods.
2. **Implement a minimal actual macro across crate boundaries.** Put a root, subclass, and further subclass in separate
   crates. Test same-module safe callers as well as downstream consumers. Include renamed imports, visibility, and
   generics. Generated encapsulation is part of the design, not packaging to defer until later.
3. **Test semantic constraints and diagnostics.** Unknown/wrong-signature overrides, method/slot-name collisions,
   final/abstract classes, inherited overrides, lexical `super` with late-bound calls inside a base body, and successful
   and failing sibling casts. Define what virtual generic methods, `async fn`, `Self` arguments/returns, and non-`&self`
   receivers support or reject; the present fn-pointer example does not establish all Rust method forms.
4. **Expand memory-safety tests around real generated fields.** Include concurrent mutation, reentrant callbacks,
   panic/unwind paths, last-owner drop, sibling classes with different layouts, empty roots, highly aligned fields,
   generic subtyping, and all intended strategies. Keep aliasing/layout tests and ordinary compile-fail tests separate.
5. **Wire targeted Miri into CI.** Both models, strict provenance, multiple seeds, plus selected randomized-layout and
   additional-target checks. Keep a written soundness argument and require review when its assumptions change.
6. **Evaluate the missing safe/metadata candidate.** A small cross-crate descriptor prototype should determine whether
   simple annotations, diagnostics, and maintenance are practical. No need to implement an entire competing framework.
7. **Benchmark generated implementations and relevant workloads.** Include actual pointer strategies, shared versus
   independent objects, generic `super`, mixed receivers, deep/wide hierarchies, memory use, and compile/binary size.
   Re-evaluate after removing registry hot-path lookups if that optimization is adopted.

These are research/implementation gates, not changes made by this review. For an immediate stable release, retain the
current composition model rather than smuggling unreviewed "true inheritance" into it.

## 6. Answer to the original question

**Has the original author reached a sensible conclusion?**

- **Yes**, as a provisional judgment that the unsafe prefix-layout candidate merits investigation. It has a coherent
  layout/ownership foundation, meaningful flat-handle benefits, and a demonstrated non-generic dispatch advantage.
- **No**, if the conclusion means the present evidence establishes a manageable, adequately guarded unsafe surface, or
  that unsafe is the only way to achieve the preferred syntax.

The new findings are fixable in principle, which is a reason to continue researching the main design, not to ignore
them. Equally, waiting for an _unfixable_ Miri failure is the wrong threshold for considering the safe design:
maintenance cost, encapsulation complexity, and actual workload behaviour can justify choosing safe even if every known
unsafe bug has a possible repair.

**I would approve a hardened experiment, not the original unconditional recommendation.** The next decision should be
based on a sound generated API and a fair architectural comparison, not on the current prototypes passing Miri.

## Appendix: Reproduction

Run from the repository root. The commands use the existing Pixi and Taskfile setup. The standalone script-mode edition
warning is inherited from the existing Miri workflow.

### Baseline and positive probes

```sh
pixi run --locked task miri
pixi run --locked task miri -- docs/research/galvanize-rs-true-inheritance/spikes/review-spike.rs
pixi run --locked env RUSTFLAGS=-Zrandomize-layout task miri -- docs/research/galvanize-rs-true-inheritance/spikes/review-spike.rs
pixi run --locked task miri -- docs/research/galvanize-rs-true-inheritance/spikes/review-spike.rs -- variance
pixi run --locked cargo test -p galvanize-rs --test class --test class_ui
```

The `variance` mode intentionally asserts the **observed broken invariant**, so its exit status is zero when it
reproduces. It is an evidence probe, not a regression test asserting desired future behaviour.

### Expected UB failures

Run each separately; each is expected to exit nonzero with a UB diagnostic:

```sh
pixi run --locked env CARGO_UNSTABLE_SCRIPT=true MIRIFLAGS="-Zmiri-strict-provenance" cargo +nightly-2026-11-19 miri run -q --manifest-path docs/research/galvanize-rs-true-inheritance/spikes/review-spike.rs -- forge-bench
pixi run --locked env CARGO_UNSTABLE_SCRIPT=true MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-tree-borrows" cargo +nightly-2026-11-19 miri run -q --manifest-path docs/research/galvanize-rs-true-inheritance/spikes/review-spike.rs -- forge-bench
pixi run --locked env CARGO_UNSTABLE_SCRIPT=true MIRIFLAGS="-Zmiri-strict-provenance" cargo +nightly-2026-11-19 miri run -q --manifest-path docs/research/galvanize-rs-true-inheritance/spikes/review-spike.rs -- forge-original
pixi run --locked env CARGO_UNSTABLE_SCRIPT=true MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-tree-borrows" cargo +nightly-2026-11-19 miri run -q --manifest-path docs/research/galvanize-rs-true-inheritance/spikes/review-spike.rs -- forge-original
```

Explicit commands are used because the normal task stops on the first model's failure.

### Expected safe-implementation compile failure

After creating the local output directory if needed:

```sh
mkdir -p target/tmp
pixi run --locked rustc --edition 2024 --cfg review_safe_variance docs/research/galvanize-rs-true-inheritance/spikes/review-spike.rs -o target/tmp/inheritance-review-safe-variance
```

Expected: E0308 on the attempted `safe::Boxed<AnyLifetime>` to `safe::Boxed<StaticOnly>` coercion.

### Timing

```sh
pixi run --locked cargo bench -p galvanize-rs --bench inheritance -- weight generic_get --threads 1 --sample-count 1000 --sample-size 1000
pixi run --locked cargo bench -p galvanize-rs --bench inheritance -- weight generic_get --threads 8 --sample-count 100 --sample-size 1000
pixi run --locked rustc --edition 2024 -O -D warnings docs/research/galvanize-rs-true-inheritance/spikes/review-spike.rs -o target/tmp/inheritance-review
pixi run --locked target/tmp/inheritance-review timing
pixi run --locked target/tmp/inheritance-review generic-timing
```

The supplemental timing modes use `Instant`, black-boxed receivers, checked sums, preconstructed objects, and
alternating execution order. They are deliberately small controlled workloads, not substitutes for a full statistical
benchmark suite. Run them without other validation jobs for less noise.
