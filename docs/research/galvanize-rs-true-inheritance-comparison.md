# True Inheritance for galvanize-rs: Which Design?

Status: **Research**. Nothing here is scheduled.

This is a high-level comparison of the two proposed designs, plus a finger-in-the-air recommendation. Neither design
needs to be understood in detail to read it:

- the **main design**, [galvanize-rs-true-inheritance.md](galvanize-rs-true-inheritance.md), with one view per object,
  `fn`-pointer vtables and a small amount of `unsafe`;
- the **safe variant**, [galvanize-rs-true-inheritance-safe.md](galvanize-rs-true-inheritance-safe.md), with one trait
  per class, handles that grow with depth, and no `unsafe`.

It assumes the main design passes Miri. Without that, the question is moot. The prototype does (`task miri`), so far for
the paths its `main` exercises.

## TL;DR

- **Both designs are fast enough** for the languages galvanize-rs mimics. The main design's virtual calls run at JIT
  speed (Java, C#, Kotlin, JavaScript). The safe variant's overrides cost about what a method call costs in CPython.
  Everything else costs the same in both.
- **You'll rarely notice the difference.** It shows up when objects are shared across threads, in work with a frame
  budget (UIs, games), and in bulk loops over millions of objects. Those are exactly the hot paths you'd rewrite in
  idiomatic Rust anyway.
- **Performance alone doesn't justify the `unsafe`, but ergonomics do.** The main design is the one whose syntax looks
  like the languages we mimic: `extends = Dog` and a plain `#[override]`. The safe variant needs every ancestor listed
  and every override annotated with the class that declared the method. No mainstream language asks for either. Since
  making Rust feel like a higher-level language is the library's whole pitch, that's what tips it.
- **Recommendation: the main design.** Its `unsafe` is contained to fixed templates, follows the same prefix-layout
  approach as Servo's DOM and GObject, and is invisible to users. Keep the safe variant as the fallback if Miri finds a
  hole that can't be fixed.

## 1. Against the languages we mimic

Single-thread medians on the same machine (an i7-13800H):

- **galvanize-rs:** the benchmark forks in [benches/inheritance.rs](../../projects/galvanize-rs/benches/inheritance.rs),
  whose results are in the safe variant's section 3.1.
- **CPython 3.14:** `timeit` on the same hierarchy.
- **JavaScript:** Bun 1.4 (the JavaScriptCore JIT), warm loops.

Java, C# and Kotlin weren't installed, so they weren't measured. Their JITs behave like the JavaScript column. The
Python and JavaScript numbers include a nanosecond or so of loop overhead.

| Operation                                   | Main design | Safe variant | CPython 3.14 | JS (Bun JIT) |
| ------------------------------------------- | ----------- | ------------ | ------------ | ------------ |
| Construct a `Dog`                           | 34 ns       | 34 ns        | 85 ns        | 11 ns        |
| Read a field                                | 11 ns       | 11 ns        | 6 ns         | 0.9 ns       |
| Virtual call, base body or inherited        | 1 ns        | 0.8 ns       | 14 ns        | —            |
| Virtual call, an override                   | 0.8 ns      | 26 ns        | 15 ns        | 0.9 ns       |
| Virtual call, an override plus a `super`    | 1.4 ns      | 63 ns        | 44 ns        | 1.3 ns       |
| `describe()`: string formatting + a virtual | 53 ns       | 76 ns        | 50 ns        | 9 ns\*       |
| Downcast or `isinstance`                    | 5.5 ns      | 16.5 ns      | 15 ns        | 3 ns\*       |

\* With four classes mixed at one call site. A JIT's single-class call sites are faster still. Mixing classes slows its
virtual calls to about 9 ns, while Rust's costs don't change.

Reading across:

- **The main design dispatches at JIT speed.** Construction is slower than a JIT's but faster than CPython's.
- **The safe variant's overrides sit between CPython's plain method call and its `super` call.** Everything else matches
  the main design.
- **Field reads are slower than Python's in both designs.** That isn't inheritance. It's galvanize-rs's default of one
  `RwLock` per field, which costs about 11 ns. In typical code, that and allocation will dominate long before dispatch
  does. It's also a separate decision from this one.

## 2. Will a real app notice?

Some back-of-the-envelope workloads, using the override costs above (main design 1 ns, safe variant 26–63 ns):

| Workload                                                    | Override calls | Main design | Safe variant |
| ----------------------------------------------------------- | -------------- | ----------- | ------------ |
| A web request or CLI command touching a few hundred objects | ~10k           | 0.01 ms     | 0.3–0.6 ms   |
| One 60 fps frame: 10k UI or game nodes, 5 overrides each    | 50k            | 0.05 ms     | 1.3–3.2 ms   |
| A batch job over 100M objects                               | 100M           | 0.1 s       | 2.6–6.3 s    |

- **I/O-bound and interactive apps won't notice.** Half a millisecond is lost next to a single network round trip or
  disk read. CPython apps live with the same costs every day.
- **Frame-budgeted work starts to.** In the second row, the safe variant spends 8–19% of the 16.7 ms frame on dispatch
  alone.
- **Bulk computation does.** The gap is seconds, but a loop over 100M objects is the textbook case for an idiomatic Rust
  rewrite: enums or traits, plain structs and no locks. That escape hatch works the same with either design.

There's one cost that isn't a constant factor: **contention**. Every call to a safe-variant override clones an `Arc` and
drops it again. That writes to the object's shared reference count, so threads calling methods on the same object fight
over one cache line. In the benchmarks, 8 threads roughly doubled the cost of `describe()` on a `Puppy`. Calls in the
main design write nothing shared. Only 8 threads were measured, and more cores would likely make it worse. For a library
that requires `Send + Sync` so objects can be shared across threads, this is the performance difference most likely to
surprise someone in production.

## 3. Beyond performance

|                                         | Main design                                                    | Safe variant                                       |
| --------------------------------------- | -------------------------------------------------------------- | -------------------------------------------------- |
| What users write                        | `extends = Dog`, `#[override]`, like Java, C# or Kotlin        | every ancestor, plus `#[override(DeclaringClass)]` |
| Inserting a class mid-hierarchy         | change one `extends`                                           | update every descendant's ancestor list            |
| Moving a virtual method to a base class | no changes to overrides                                        | update every override's annotation                 |
| `unsafe`                                | in the runtime and macro templates                             | none                                               |
| Users with `#![forbid(unsafe_code)]`    | unaffected, because the lint ignores proc-macro output         | unaffected                                         |
| If there's a bug in the library         | possibly undefined behaviour: memory corruption, hard to trace | a compile error or a panic                         |
| Ongoing maintenance                     | Miri in CI (nightly), careful review of the generated code     | nothing extra                                      |
| Generic classes                         | a vtable registry, about 15 ns per construction                | the compiler builds the vtables                    |
| How costs scale with hierarchy depth    | flat                                                           | handles, clones and override calls grow            |
| Generated code size                     | one vtable per class                                           | grows with depth²                                  |

`#![forbid(unsafe_code)]` was checked on our pinned toolchain (1.100): the lint fires on `unsafe` written in a crate,
but not on `unsafe` a proc macro generates into it. Tools that count `unsafe`, such as `cargo-geiger`, will still see
it.

## 4. Recommendation

**Is the `unsafe` worth it?** On performance alone, for most apps, probably not:

- both designs land within the range of the languages galvanize-rs imitates;
- most apps are bound by I/O or by allocation and locking, not dispatch;
- the hot paths where it would matter are the ones you'd rewrite in plain Rust anyway.

But performance isn't the main thing the `unsafe` buys. It buys the syntax. The main design lets users write inheritance
the way they would in Kotlin or C#: name the parent, mark the override, done. The safe variant makes them maintain
information the compiler normally works out, on every subclass, forever. For a library whose pitch is "Rust like a
higher-level language", that friction costs more than the nanoseconds do. The flat costs and the absence of contention
come on top.

The `unsafe` is a manageable risk:

- It's a fixed, small set of patterns, generated from templates the library controls, never written by users.
- The soundness argument is the well-trodden prefix-layout one that Servo's DOM and GObject rely on.
- With Miri and trybuild in CI, a regression should be caught before it ships rather than in a user's app.

So: **build the main design**, and keep the safe variant documented as the fallback. Choose the safe variant instead if:

- **Miri finds a hole without a fix.**
- **"Zero `unsafe`" becomes a selling point for the people adopting galvanize-rs.** Some teams audit dependencies for
  it.
- **You want a cautious first release.** Moving from the safe variant to the main design later is mostly a matter of
  users deleting annotations. Moving the other way is a breaking change. But this path asks users to write annotations
  that are destined to become noise.
