# galvanize-rs

Write Rust like a higher-level language. galvanize-rs trades some runtime performance for a simpler model without the
borrow checker pain: every type is a cheaply cloneable, shared handle, like an object reference in Python, Go or C#.
Cloning a handle yields another handle to the _same_ value.

Reads and writes go through closures that you can call from anywhere, you never _(or rarely)_ hold a reference & `mut`
is basically never needed.

I know the purest will cringe at this, but sometimes practicality wins & it's worth the trade-off. Hot paths can still
be optimized with Rust's usual low level control & safety, but everyday code becomes simpler and faster to reason about.

I'm sure in another 1-2 years I will look back this project and think this is utter garbage as I have done for many
other experiments, it's just part of my learning process.

> **Whats with the name?** Rust's name comes from oxidation, so the best names for this idea are about what you do to
> metal to make it easier to handle. Top pick: Galvanize. Galvanizing coats metal in a protective layer so it doesn't
> rust. That's what your framework does: it wraps the borrow checker's sharp edges in a protective runtime layer. It's
> also a verb, so use galvanize::prelude::*; reads well.
>
> _NB: Thats from Claude of course, and it looks like there are a million other crates called galvanize so we might have
> a name change in the future but for now I'm rolling with it._

## Types

| Type                         | Like                   | Notes                                              |
| ---------------------------- | ---------------------- | -------------------------------------------------- |
| `GPointer<T>`                | a shared variable      | `read_with`, `write_with`, `get`, `set`            |
| `GList<T>`                   | `Vec<T>`               | `list![1, 2, 3]`                                   |
| `GDict<K, V>`                | `BTreeMap<K, V>`       | `dict! { "a" => 1 }`                               |
| `GString`, `GOsStr`, `GPath` | `str`, `OsStr`, `Path` | immutable and `Arc` backed, `s!("x")`, `sf!("{}")` |

Elements are stored by value and must be `Clone`, so accessors such as `GList::get` return clones rather than
references. Make the elements themselves handles (another `GList`, `GPointer`, ...) when you want that to be cheap.

```rust
use galvanize_rs::{GList, GPointer, GString, list, sf};

#[derive(Clone)]
struct Counter {
  count: GPointer<i32>,
}

let counter = Counter { count: GPointer::new(0) };
let alias = counter.clone(); // the same counter
alias.count.write_with(|n| *n += 1);
assert_eq!(counter.count.get(), 1);

let names: GList<GString> = list![sf!("{}-{}", "a", 1)];
names.push("b");
```

## Class-like objects

`#[class]` turns a struct into a heap allocated object handle with Java/C#-style identity: cloning it yields another
handle to the same object. It generates a getter and `set_` setter per field, identity `PartialEq`, and `Deref` to the
inner `GPointer` for multi-field `read_with`/`write_with`. See [examples/classes.rs](examples/classes.rs).

```rust
use galvanize_rs::class;

#[class]
#[derive(Default)]
struct Counter {
  count: i32,
}

let a = Counter::default(); // count starts at 0
let b = a.clone();
b.set_count(b.count() + 1);
assert_eq!(a.count(), 1);
assert_eq!(a, b); // identity, not field values
```

## Traits and `#[any]`

A trait can't see the fields of the class implementing it, so a default method that needs data must call a required
method that returns a handle to a class holding it (see [examples/shapes.rs](examples/shapes.rs)). To store different
classes behind one trait in a `GList`, put `#[any]` on the trait. It generates a cloneable `AnyShape` wrapper for `trait
Shape`, so `let shapes: GList<AnyShape> = list![&circle, &rect];` just works.

## Cloning: shared handles vs copies

There are three levels, and it's worth being precise about which one you want:

| Call             | Result                                                                      |
| ---------------- | --------------------------------------------------------------------------- |
| `clone()`        | Another handle to the _same_ object. Cheap (an `Arc` bump).                 |
| `duplicate()`    | A new, independent object with its own copy of the fields (one level deep). |
| _(not provided)_ | A recursive copy of everything reachable from the object.                   |

`duplicate()` is **one level deep**. It clones the object's own fields with `Clone`, and cloning a field that is itself
a handle (another class, a `GList`, a `GDict`, a `GPointer`, ...) just yields another handle to the same thing. So the
copy is independent of the original for plain fields, but still shares everything nested inside it:

```rust
let followup = task.duplicate();
followup.set_title("Something else".into()); // independent: `task` keeps its title
followup.assignee().unwrap().set_name("Zed".into()); // shared: `task`'s assignee is renamed too
```

If you need independence for a nested handle, copy it yourself and set it on the copy:

```rust
let followup = task.duplicate();
if let Some(user) = task.assignee() {
  followup.set_assignee(Some(user.duplicate()));
}
```

A fully recursive deep clone is deliberately not provided. It would need every field type to opt in to a `DeepClone`
trait (so plain std types and foreign types would need wrappers), and it would have to handle cycles (a task pointing at
a user who points back at the task) and diamonds (two tasks sharing one user should still share one user in the copy),
which means tracking object identity during the copy.

## Pointer strategies

Every type above lives in a `MutablePointer`, chosen by a `PointerStrategy` type parameter that defaults to
`ArcRwLockStrategy`. Pick another to change the trade-off:

- **`ArcRwLockStrategy`** (the default): many concurrent readers, one writer.
- **`ArcMutexStrategy`**: one accessor at a time, readers included.
- **`ParkingLotRwLockStrategy`** / **`ParkingLotMutexStrategy`**: the same shapes using `parking_lot` locks, which never
  poison.
- **`ArcSwapStrategy`**: lock-free reads, but every write copies the value and the write closure may be retried.

```rust
use galvanize_rs::{ArcSwapStrategy, GList, GPointer};

let a: GPointer<i32, ArcSwapStrategy> = GPointer::with_strategy(0);
let b: GList<i32, ArcSwapStrategy> = GList::with_strategy();
```

[benches/pointers.rs](benches/pointers.rs) compares them (`task bench -- pointers`).

### Deadlocks

The locking strategies (`ArcRwLockStrategy`, the default, and `ArcMutexStrategy`) are **not reentrant**. While a
`read_with`, `write_with`, `with_x` or `update_x` closure is running, the object is locked, and so is every other handle
to it. Touching the same object again from inside the closure blocks forever. There is no panic or error, the thread
just hangs.

The catch is that the generated class accessors (`x()`, `set_x()`, `with_x()`, `update_x()`) and `get()`/`set()` all
take the lock too. They look like plain field access, so it's easy to call one by accident:

```rust
#[class]
#[derive(Default)]
struct Counter {
  count: i32,
}

let counter = Counter::default();
let alias = counter.clone(); // a handle to the same object, so it shares the lock

// DEADLOCK: `count()` needs the lock that `write_with` already holds.
counter.write_with(|data| data.count = counter.count() + 1);

// DEADLOCK: so does a setter inside a read, and so does going through any alias.
counter.read_with(|_| alias.set_count(1));

// DEADLOCK: `update_count` holds the lock while `count()` tries to take it.
counter.update_count(|n| *n = counter.count() + 1);
```

The same goes for any call that reaches the object indirectly, such as a method on another class that holds a handle
back to this one. Keep the closure self-contained, either by using only the value the closure is handed:

```rust
counter.write_with(|data| data.count += 1);
counter.update_count(|n| *n += 1);
```

or by reading first, then writing, with the lock released in between:

```rust
let next = counter.count() + 1;
counter.set_count(next);
```

Reading then writing as two steps isn't atomic: another thread can change the value in between. If you need the whole
read-modify-write to be atomic, do it inside one closure using the value it is given, as above.

Calls on _different_ objects are fine, so `a.update_count(|n| *n += b.count())` is safe as long as nothing inside `b`
ends up locking `a`, and as long as no other thread locks `a` then `b` in the opposite order (that's the classic
lock-ordering deadlock).

`ArcSwapStrategy` reads never block, so the examples above don't hang with it. The write closure may still be retried
and sees a snapshot, which is why it shouldn't have side effects. Don't rely on this if you might change strategy later.
See the docs on `ArcRwLockPointer` for more on the underlying pointers.

### Thread safety

The handles are `Send + Sync` whenever the value they hold is, so they work in both threaded and async code. No bounds
are imposed on the value otherwise.
