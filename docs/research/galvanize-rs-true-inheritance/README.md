# True Inheritance for galvanize-rs

This folder catalogues the research behind galvanize-rs's class inheritance: classes that extend one another, virtual
methods that subclasses override, `super` calls, and checked downcasts, like Python, Java, C# or Kotlin.

## Outcome

Two designs survived the research. Both are now part of galvanize-rs and share the same macro syntax:

```rust
#[class(open)]
pub struct Animal {
  pub name: Str,
}

#[class_methods]
impl Animal {
  #[class_virtual]
  pub fn speak(&self) -> Str {
    "...".into()
  }
}

#[class(extends = Animal)]
pub struct Dog {}

#[class_methods]
impl Dog {
  #[class_override]
  pub fn speak(&self) -> Str {
    sf!("Woof, not {}", self.super_speak())
  }
}
```

Each hierarchy chooses how it is implemented with `#[class(runtime_strategy = ...)]`:

- **`SafeRuntimeStrategy`** (the default) is the [safe descriptor design](05-descriptors.md). It has no `unsafe`.
  Handles grow with hierarchy depth, and every override call clones an `Arc`, which contends when threads share an
  object.
- **`UnsafeRuntimeStrategy`** is the [hardened main design](06-hardened.md). It uses a small, Miri-tested `unsafe`
  kernel with prefix-layout storage and function-pointer vtables. Handles are four words at any depth, and virtual calls
  run at JIT-like speed without writing shared state.

## Reading order

| # | Document                                   | What it covers                                                                                                |
| - | ------------------------------------------ | ------------------------------------------------------------------------------------------------------------- |
| 1 | [Original design](01-original-design.md)   | What true inheritance means, why composition + `Deref` can't get there, and the prefix-layout `unsafe` design |
| 2 | [Safe variant](02-safe-variant.md)         | A fully safe alternative with one trait per class, at the cost of listing every ancestor in the syntax        |
| 3 | [Comparison](03-comparison.md)             | The two designs against each other and against CPython and JavaScript, recommending the `unsafe` design       |
| 4 | [Defensive review](04-defensive-review.md) | A devil's-advocate review that found UB reachable through the original prototype's safe API                   |
| 5 | [Descriptor design](05-descriptors.md)     | The safe runtime with generated compile-time descriptors, giving it the same syntax as the `unsafe` design    |
| 6 | [Hardened main design](06-hardened.md)     | The `unsafe` design repaired: invariant typed handles, protected dispatch, async support and Miri coverage    |

## Supporting material

- [spikes/](spikes/): standalone, hand-expanded prototypes. Each compiles with plain `rustc`; see its header.
  - [original-spike.rs](spikes/original-spike.rs) and [safe-spike.rs](spikes/safe-spike.rs): the first two designs.
  - [review-spike.rs](spikes/review-spike.rs): the defensive review's adversarial checks. Its `forge-*` modes
    deliberately exercise undefined behaviour and must only be run under Miri.
  - [bench-forks/](spikes/bench-forks/): the benchmark's forks of the first two designs. The legacy main design is
    **known unsound** and kept only as a negative control.
- [benchmarks/](benchmarks/): the final four-design results (legacy main, handwritten safe, descriptors, hardened main),
  as [JSON](benchmarks/inheritance.bench.json) and an [HTML report](benchmarks/inheritance.bench.html).

The research crates (`projects/galvanize-rs/research/`) were removed when the designs were productionised. Code links in
these documents are permalinks to commit
[`79bde36`](https://github.com/brad-jones/doup/tree/79bde36/projects/galvanize-rs/research), the last to contain them.
The current benchmark only compares the two shipped runtime strategies.
