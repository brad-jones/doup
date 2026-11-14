# Strategy Pattern with Async Traits: Which Approach Should doup Use?

Status: **Recommendation**. Written for someone new to Rust, so it explains the "why" as well as the "what".

The runnable examples this document is based on live in [projects/cli/src/example](../../projects/cli/src/example).

## TL;DR

Use an **enum that holds each strategy, with a trait that has native `async fn` methods**, and let a `match` forward
each call to the right variant. Write the forwarding `match` by hand to start with. If it becomes tedious, replace it
with the `enum_dispatch` or `spire_enum` macro (both were tested with async methods, see
[the experiment](#what-i-actually-tested)).

Reach for `Box<dyn Trait>` + `async_trait` only if the set of strategies must be **open** (code you do not control can
add new ones at runtime).

Do **not** use the `delegation` crate for this: it does not work with `async fn`.

Adopted: `Git` in [projects/git/src/lib.rs](../../projects/git/src/lib.rs) is a `spire_enum` delegated enum (see
[spire_enum_async_speaker.rs](../../projects/cli/src/example/spire_enum_async_speaker.rs)).

---

## 1. Background you need first

### 1.1 What is the strategy pattern?

You have one job (say "speak") that can be done in several interchangeable ways (dog, cat). The calling code does not
care which way is used; it just says "do the job". Which way is used is picked at runtime, for example from a config
file.

In Rust the "job" is a **trait**, and each "way" is a **struct that implements the trait**:

```rust
trait Speak {
  fn say(&self);
}
struct Dog;
struct Cat;
impl Speak for Dog {
  /* ... */
}
impl Speak for Cat {
  /* ... */
}
```

The hard part, and the reason there are 8 example files, is this: **`match` arms must all produce the same type**.

```rust
let speaker = match name {
  "dog" => Dog,   // type Dog
  "cat" => Cat,   // type Cat  <- compile error, arms have different types
  _ => panic!(),
};
```

So we need a _single type_ that can stand for "either a `Dog` or a `Cat`". Rust gives us two ways to build one:

| Approach           | The single type is...    | Called                          |
| ------------------ | ------------------------ | ------------------------------- |
| Trait object       | `Box<dyn Speak>`         | **dynamic dispatch**            |
| Enum wrapping each | `enum SpeakKind { ... }` | **static dispatch** (of a sort) |

### 1.2 Dynamic dispatch: `Box<dyn Speak>`

`Box<dyn Speak>` means "a pointer to _some_ value, on the heap, that implements `Speak`; I forget which concrete type".
At runtime Rust looks the method up in a hidden table (a _vtable_) attached to the pointer, then calls it.

- Pro: works for any type that implements the trait, including types written later by someone else.
- Con: one heap allocation per value, a lookup per call, and **the trait must be "dyn compatible"** (see next section).

### 1.3 Enum dispatch

You list every strategy as a variant of an enum. Each variant owns one strategy. The enum itself also implements the
trait, and does the forwarding with a `match`:

```rust
enum SpeakKind {
  Dog(Dog),
  Cat(Cat),
}

impl Speak for SpeakKind {
  fn say(&self) {
    match self {
      SpeakKind::Dog(d) => d.say(),
      SpeakKind::Cat(c) => c.say(),
    }
  }
}
```

- Pro: no heap allocation, no vtable, the compiler knows every possible type. If you add a variant and forget to handle
  it in the `match`, **the compiler stops you** (exhaustive matching). Everything else in your program just sees
  `SpeakKind`.
- Con: the set of strategies is **closed**. Adding one means editing the enum. And you write the forwarding code for
  every method on the trait (this is the "boilerplate" the macro crates remove).

---

## 2. Why `sync_speaker.rs` stops working once the trait is async

Since Rust 1.75 you can write `async fn` directly in a trait:

```rust
trait Speak {
  async fn say(&self);
}
```

What does `async fn say(&self)` really mean? It is shorthand for:

```rust
fn say(&self) -> impl Future<Output = ()>;
```

Each implementation (`Dog`, `Cat`) returns its _own, unnamed_ future type, and those types have different sizes. For
`Box<dyn Speak>` to work, Rust has to build the vtable ahead of time, and a vtable entry needs a single, known return
type with a known size. "Some future of unknown size" does not fit. The compiler therefore says the trait is **not dyn
compatible** (older docs call this "not object safe"), and `Box<dyn Speak>` is rejected.

There are two ways out:

1. **Make the future a known type**: box it. That is what the `async_trait` crate does. It rewrites your trait to return
   `Pin<Box<dyn Future<Output = ()> + Send>>`. One type, known size, vtable is happy. The price is a heap allocation for
   _every call_, on top of the one for the `Box<dyn Speak>`.
2. **Avoid `dyn` entirely**: use the enum. The `match` calls a concrete `Dog` or `Cat`, so the compiler knows the exact
   future type in each arm. Nothing needs to be boxed.

Option 2 is what the recommendation is built on.

---

## 3. The candidates

All of these were compiled against doup's pinned toolchain. See [the experiment](#what-i-actually-tested).

### 3.1 `Box<dyn Trait>` + `async_trait` ([async_speaker.rs](../../projects/cli/src/example/async_speaker.rs))

```rust
#[async_trait]
trait AsyncSpeak {
  async fn say(&self);
}
let speaker: Box<dyn AsyncSpeak> = AsyncDog {}.into();
speaker.say().await;
```

Good: simplest to read, open set of strategies, one extra dependency.

Watch out:

- **The `Send` trap.** If you ever pass the speaker to `tokio::spawn` (or hold it across threads), `Box<dyn AsyncSpeak>`
  is **not `Send`** unless you declare it: `trait AsyncSpeak: Send + Sync`. I hit this exact compile error while
  testing. Forgetting this gives a confusing error far from the cause.
- Hidden heap allocation per call. Irrelevant for doup (git and network calls dwarf it), but it is a cost.
- It is a macro rewriting your trait, so error messages mention types you never wrote.

### 3.2 Hand-written enum + native `async fn` ([enum_dispatch_speaker.rs](../../projects/cli/src/example/enum_dispatch_speaker.rs) pattern, made async)

```rust
trait Speak {
  async fn say(&self);
}

enum SpeakKind {
  Dog(Dog),
  Cat(Cat),
}

impl Speak for SpeakKind {
  async fn say(&self) {
    match self {
      SpeakKind::Dog(d) => d.say().await,
      SpeakKind::Cat(c) => c.say().await,
    }
  }
}
```

Good: no macros, no extra crates, no heap allocation, the compiler checks you handled every variant, and the code is
exactly what it looks like. It compiled and could be passed to `tokio::spawn`.

Watch out: boilerplate is `methods x variants` lines. With 1 or 2 methods and 2 to 4 strategies that is a few lines;
with 10 methods and 8 strategies it is painful.

### 3.3 `enum_dispatch` crate ([macro_enum_dispatch.rs](../../projects/cli/src/example/macro_enum_dispatch.rs))

You list the types; the macro writes the `match` (and the `From` impls) for you:

```rust
#[enum_dispatch(SpeakKind)]
trait Speak {
  async fn say(&self);
}

#[enum_dispatch]
enum SpeakKind {
  Dog,
  Cat,
}
```

- **Works with `async fn`** (tested, including `tokio::spawn`).
- **Fails** if the trait is written in the explicit form `fn say(&self) -> impl Future<Output = ()> + Send` (conflicting
  trait implementations error). That matters because that explicit form is what
  [backend.rs](../../projects/git/src/backend.rs) uses, see
  [section 4](#4-the-send-question-and-why-gitbackend-looks-different).
- Least code of all options; the trade-off is that the generated code is invisible, so a beginner can't read what it
  does.

### 3.4 `ambassador` crate ([ambassador_speaker.rs](../../projects/cli/src/example/ambassador_speaker.rs))

- **Works with both** `async fn` and `-> impl Future + Send` in the trait (tested, including `tokio::spawn`).
- Gotcha: it generates a helper macro named after the trait (`_ambassador_impl_Speak`). Two traits with the same name in
  one crate collide (I hit this while testing, because the example files all use the name `Speak`). Only an issue in
  this examples folder, not in real code with unique trait names.
- The existing file already notes that rust-analyzer shows false errors on it, which hurts the editor experience while
  you are learning.

### 3.5 `spire_enum` crate ([spire_enum_speaker.rs](../../projects/cli/src/example/spire_enum_speaker.rs))

- **Works with `async fn`** (tested, including `tokio::spawn`). You write `async fn say(&self);` with no body in a
  `#[delegate_impl]` block and it fills it in. The trait stays untouched, `impl_conversions` writes the `From` impls.
- It is the least widely known of the macro crates here. I have not evaluated its long-term maintenance.

### 3.6 `delegation` crate ([delegation_speaker.rs](../../projects/cli/src/example/delegation_speaker.rs))

- **Does not work with `async fn`.** Compiling it produced "mismatched types: expected `()`, found future" errors. Rule
  it out for this task.

### 3.7 Summary

| Approach                   | Works with async methods   | Heap alloc per call | Open set of strategies | Hidden code          |
| -------------------------- | -------------------------- | ------------------- | ---------------------- | -------------------- |
| `Box<dyn>` + `async_trait` | Yes (with `Send + Sync`)   | Yes                 | **Yes**                | Macro rewrites trait |
| Hand-written enum          | **Yes**                    | No                  | No                     | None                 |
| `enum_dispatch`            | Yes (`async fn` form only) | No                  | No                     | Yes                  |
| `ambassador`               | Yes (both forms)           | No                  | No                     | Yes                  |
| `spire_enum`               | Yes                        | No                  | No                     | Yes                  |
| `delegation`               | **No**                     | n/a                 | n/a                    | n/a                  |

---

## 4. The `Send` question, and why `GitBackend` looks different

In [backend.rs](../../projects/git/src/backend.rs) the trait is written like this, not with plain `async fn`:

```rust
fn clone_repo(&self, options: &CloneOptions) -> impl Future<Output = Result<(), GitError>> + Send;
```

Why? A future is only allowed to move between threads (which `tokio::spawn` and tokio's multi-threaded runtime do) if it
is `Send`. With a plain `async fn` in a trait, code that is _generic_ over the trait (`fn foo<B: GitBackend>(b: B)`)
cannot know the future is `Send`, so spawning it fails. Writing `+ Send` in the trait promises it. Implementers can
still write ordinary `async fn` (as `GitCli` and `Grit` do); the compiler checks the promise.

Rust also emits the `async_fn_in_trait` warning for **public** traits using plain `async fn`, to nudge you towards this
form.

Practical rule:

- Trait is `pub` and used in generic code that may spawn: write `-> impl Future<Output = T> + Send`.
- Trait is private to one crate, and you only use the concrete enum: plain `async fn` is fine (this is what I tested
  with `tokio::spawn` on the concrete enum).

Whichever you choose, the enum approach never needs `dyn`, so you avoid the `Box<dyn ...>` `Send + Sync` trap in section
3.1.

---

## 5. Recommendation and reasoning

**Use the hand-written enum with a native async trait.**

1. **It fits the problem.** doup picks a strategy from a fixed list in configuration (`GitBackendKind` in
   [bot.rs](../../projects/cli/src/config/bot.rs) is already an enum of the choices). The set of strategies is closed
   and known at compile time, which is exactly where an enum shines.
2. **It is consistent.** `Git` already does this, so there is one pattern to learn and one to maintain.
3. **The compiler helps you.** Add a variant and forget a `match` arm? Compile error. With `Box<dyn>` a forgotten case
   is a runtime surprise.
4. **No async pitfalls.** No `Box<dyn Future>`, no `Send + Sync` supertrait trap, no macro rewriting your trait.
5. **It is the best way to learn.** The `match` is ordinary Rust you can read, step through in a debugger, and
   understand. Macros are better adopted _after_ you know what they generate.
6. **Performance is not the deciding factor.** Avoiding the heap allocation is a nice bonus, but doup is dominated by
   network and git I/O, so do not choose on speed.

### When to switch to something else

- **The forwarding boilerplate becomes painful** (roughly: many trait methods and many variants). Switch the
  hand-written `impl` to `enum_dispatch` or `spire_enum`. The trait and callers don't change, only the enum's `impl`.
  Prefer `ambassador` if the trait uses the `-> impl Future + Send` form; `enum_dispatch` can't handle that form.
- **You need an open set** (a user or another crate must be able to add a strategy without editing doup). Use `Box<dyn
  Trait>` + `async_trait`, and declare `trait Name: Send + Sync`.
- **Never** `delegation` for async traits.

### Suggested shape for the real trait

```rust
use std::future::Future;

/// One way of doing the thing; pick one when building the caller.
pub trait Strategy: Send + Sync {
  fn run(&self, input: &Input) -> impl Future<Output = Result<Output, Error>> + Send;
}

pub struct StrategyA;
pub struct StrategyB;

impl Strategy for StrategyA {
  async fn run(&self, input: &Input) -> Result<Output, Error> { /* ... */
  }
}
impl Strategy for StrategyB {
  async fn run(&self, input: &Input) -> Result<Output, Error> { /* ... */
  }
}

/// A strategy chosen at runtime; one concrete type, so it can come out of a `match`.
pub enum AnyStrategy {
  A(StrategyA),
  B(StrategyB),
}

impl Strategy for AnyStrategy {
  async fn run(&self, input: &Input) -> Result<Output, Error> {
    match self {
      Self::A(s) => s.run(input).await,
      Self::B(s) => s.run(input).await,
    }
  }
}

impl From<StrategyA> for AnyStrategy {
  fn from(s: StrategyA) -> Self {
    Self::A(s)
  }
}
impl From<StrategyB> for AnyStrategy {
  fn from(s: StrategyB) -> Self {
    Self::B(s)
  }
}
```

Callers then write:

```rust
let strategy: AnyStrategy = match kind {
  Kind::A => StrategyA.into(),
  Kind::B => StrategyB.into(),
};
strategy.run(&input).await?;
```

Beginner notes on that snippet:

- `Send + Sync` after `Strategy:` means "implementers must be safe to share across threads". Tokio's multi-threaded
  runtime needs that.
- `.await` is required on every inner call; forgetting it returns an un-run future instead of a result (the compiler
  warns about this).
- `Self::A(s)` is shorthand for `AnyStrategy::A(s)` inside the `impl`.

---

## What I actually tested

The `async` variants were compiled (`cargo check -p cli` through pixi) in a temporary scratch module, which has since
been removed. Each case defined a trait with an async method, one variant, and a call that moved the value into
`tokio::spawn` (which requires `Send`).

| Case                                                            | Result                                                  |
| --------------------------------------------------------------- | ------------------------------------------------------- |
| Hand-written enum, `async fn` in trait                          | Compiles, spawn OK                                      |
| `enum_dispatch`, `async fn` in trait                            | Compiles, spawn OK                                      |
| `enum_dispatch`, `-> impl Future + Send` in trait               | **Fails** (conflicting implementations, `From` missing) |
| `ambassador`, `async fn` in trait                               | Compiles, spawn OK                                      |
| `ambassador`, `-> impl Future + Send` in trait                  | Compiles, spawn OK                                      |
| `spire_enum`, `async fn` in trait                               | Compiles, spawn OK                                      |
| `delegation`, `async fn` in trait                               | **Fails** (`expected (), found future`)                 |
| `Box<dyn Trait>` + `async_trait`, spawn, no `Send` on the trait | **Fails** (future cannot be sent between threads)       |
| `Box<dyn Trait>` + `async_trait`, `trait X: Send + Sync`        | Compiles, spawn OK                                      |

Not tested: traits with generics or lifetimes in method signatures, multiple methods, runtime behaviour, and
performance. Macro crates can behave differently once a trait gets more complicated, so re-check with the real trait
before committing to one.

## Cleaning up the examples folder

Once a direction is chosen, the losing examples in [projects/cli/src/example](../../projects/cli/src/example) can be
deleted (along with their now-unused dependencies in [Cargo.toml](../../projects/cli/Cargo.toml): `ambassador`,
`delegation`, `enum_dispatch`, `spire_enum`, and `async-trait` if unused). `cargo check` already warns about unused
dependencies.
