//! Compares the two proposed "true" inheritance designs from `docs/research`:
//!
//! - `MainDesign`: one `RawHandle` view per object, nested `fn`-pointer vtables and a little `unsafe`
//!   (`galvanize-rs-true-inheritance.md`).
//! - `SafeDesign`: one object trait per class, `Arc<dyn …>` handles carrying one view per ancestor level, no
//!   `unsafe` (`galvanize-rs-true-inheritance-safe.md`).
//!
//! Both run on bench-owned forks of the documents' spikes (`inheritance/*.rs`). The forks follow the designs
//! where the spikes took shortcuts, and add allocation-free members so the numbers measure the inheritance
//! machinery rather than `String`s. Their module docs list the differences.
//!
//! The hierarchy is `Animal` → `Dog` → `Puppy` (overrides everything, with `super` calls) and `Dog` → `Beagle`
//! (overrides nothing), plus the generic `Boxed<T>` → `Tagged<T>`.
//!
//! Run with `cargo bench -p galvanize-rs --bench inheritance`, or filter with e.g.
//! `cargo bench -p galvanize-rs --bench inheritance -- weight`.

use divan::{AllocProfiler, Bencher, black_box};
use std::ops::Deref;

#[path = "inheritance/main_design.rs"]
mod main_design;
#[path = "inheritance/safe_design.rs"]
mod safe_design;

// `MainDesign`'s `downcast` comes from its `Class` trait, `SafeDesign`'s is inherent.
use main_design::Class as _;

#[global_allocator]
static ALLOC: AllocProfiler = AllocProfiler::system();

fn main() {
  race::<MainDesign>();
  race::<SafeDesign>();
  verify::<MainDesign>();
  verify::<SafeDesign>();
  divan::main();
}

/// Threads racing to create the first object of each class, and so its vtable. Must run before anything else
/// touches the classes. Mostly for Miri (`task miri`), which runs this but none of the benchmarks.
fn race<D: Design>() {
  let threads: Vec<_> = (0..THREADS[1])
    .map(|_| {
      std::thread::spawn(|| {
        let a = (**D::puppy(NAME)).clone();
        let b = (*D::tagged(7)).clone();
        (D::weight(&a), D::get(&b))
      })
    })
    .collect();
  for t in threads {
    assert_eq!(t.join().unwrap(), (7, 7), "race of {}", std::any::type_name::<D>());
  }
}

/// Both designs must behave identically, or comparing them is meaningless.
fn verify<D: Design>() {
  fn check<D, T: PartialEq + std::fmt::Debug, const N: usize>(what: &str, actual: [T; N], expected: [T; N]) {
    assert_eq!(actual, expected, "{what} of {}", std::any::type_name::<D>());
  }
  let animals = LEVELS.map(animal_view::<D>);
  check::<D, _, 4>("age", animals.each_ref().map(D::age), [3; 4]);
  check::<D, _, 4>("legs", animals.each_ref().map(D::legs), [4; 4]);
  check::<D, _, 4>("weight", animals.each_ref().map(D::weight), [10, 30, 7, 30]);
  check::<D, _, 4>(
    "describe",
    animals.each_ref().map(D::describe),
    [
      "Bit says ...",
      "Bit says Woof",
      "Bit says Woof (but squeakier)",
      "Bit says Woof",
    ]
    .map(String::from),
  );
  check::<D, _, 4>(
    "downcast",
    animals.each_ref().map(|a| D::downcast_dog(a).is_some()),
    [false, true, true, true],
  );
  check::<D, _, 3>(
    "bark_volume",
    DOG_LEVELS.map(dog_view::<D>).each_ref().map(D::bark_volume),
    [8, 4, 8],
  );
  check::<D, _, 2>("get", GENERICS.map(boxed_view::<D>).each_ref().map(D::get), [7, 7]);
}

const NAME: &str = "Bit";

/// Thread counts. `1` measures uncontended cost, `8` shows contention on shared refcounts, field locks and
/// the main design's vtable registry lock.
const THREADS: [usize; 2] = [1, 8];

/// Fixed iterations per sample for benchmarks that allocate. Left to calibrate itself, divan settles on one
/// iteration per sample for these under `t=8`, which makes their results swing wildly.
const ALLOC_SAMPLE_SIZE: u32 = 64;

/// The concrete class of the object being benchmarked.
#[derive(Debug, Clone, Copy)]
enum Level {
  Animal,
  Dog,
  Puppy,
  Beagle,
}

const LEVELS: [Level; 4] = [Level::Animal, Level::Dog, Level::Puppy, Level::Beagle];
const DOG_LEVELS: [Level; 3] = [Level::Dog, Level::Puppy, Level::Beagle];

/// The concrete class of the generic object being benchmarked.
#[derive(Debug, Clone, Copy)]
enum Generic {
  Boxed,
  Tagged,
}

const GENERICS: [Generic; 2] = [Generic::Boxed, Generic::Tagged];

/// The common surface of both designs, so each benchmark is written once.
trait Design {
  type Animal: Clone + Send + Sync;
  type Dog: Clone + Send + Sync + Deref<Target = Self::Animal>;
  type Puppy: Clone + Send + Sync + Deref<Target = Self::Dog>;
  type Beagle: Clone + Send + Sync + Deref<Target = Self::Dog>;
  type Boxed: Clone + Send + Sync;
  type Tagged: Clone + Send + Sync + Deref<Target = Self::Boxed>;

  fn animal(name: &str) -> Self::Animal;
  fn dog(name: &str) -> Self::Dog;
  fn puppy(name: &str) -> Self::Puppy;
  fn beagle(name: &str) -> Self::Beagle;
  fn boxed(value: u64) -> Self::Boxed;
  fn tagged(value: u64) -> Self::Tagged;
  fn age(this: &Self::Animal) -> u64;
  fn legs(this: &Self::Animal) -> u64;
  fn weight(this: &Self::Animal) -> u64;
  fn describe(this: &Self::Animal) -> String;
  fn bark_volume(this: &Self::Dog) -> u64;
  fn downcast_dog(this: &Self::Animal) -> Option<Self::Dog>;
  fn get(this: &Self::Boxed) -> u64;
}

/// Both forks expose the same API, so their `Design` impls are identical apart from the module.
macro_rules! impl_design {
  ($design:ident, $m:ident) => {
    struct $design;

    impl Design for $design {
      type Animal = $m::Animal;
      type Dog = $m::Dog;
      type Puppy = $m::Puppy;
      type Beagle = $m::Beagle;
      type Boxed = $m::Boxed<u64>;
      type Tagged = $m::Tagged<u64>;

      fn animal(name: &str) -> Self::Animal {
        $m::Animal::new(name)
      }
      fn dog(name: &str) -> Self::Dog {
        $m::Dog::new(name)
      }
      fn puppy(name: &str) -> Self::Puppy {
        $m::Puppy::new(name)
      }
      fn beagle(name: &str) -> Self::Beagle {
        $m::Beagle::new(name)
      }
      fn boxed(value: u64) -> Self::Boxed {
        $m::Boxed::new(value)
      }
      fn tagged(value: u64) -> Self::Tagged {
        $m::Tagged::new(value)
      }
      fn age(this: &Self::Animal) -> u64 {
        this.age()
      }
      fn legs(this: &Self::Animal) -> u64 {
        this.legs()
      }
      fn weight(this: &Self::Animal) -> u64 {
        this.weight()
      }
      fn describe(this: &Self::Animal) -> String {
        this.describe()
      }
      fn bark_volume(this: &Self::Dog) -> u64 {
        this.bark_volume()
      }
      fn downcast_dog(this: &Self::Animal) -> Option<Self::Dog> {
        this.downcast()
      }
      fn get(this: &Self::Boxed) -> u64 {
        this.get()
      }
    }
  };
}

impl_design!(MainDesign, main_design);
impl_design!(SafeDesign, safe_design);

/// An `Animal` handle to an object of the given concrete class, as a caller holding the base type would see it.
fn animal_view<D: Design>(level: Level) -> D::Animal {
  match level {
    Level::Animal => D::animal(NAME),
    Level::Dog => (*D::dog(NAME)).clone(),
    Level::Puppy => (**D::puppy(NAME)).clone(),
    Level::Beagle => (**D::beagle(NAME)).clone(),
  }
}

/// A `Dog` handle to an object of the given concrete class.
fn dog_view<D: Design>(level: Level) -> D::Dog {
  match level {
    Level::Animal => unreachable!("an Animal isn't a Dog"),
    Level::Dog => D::dog(NAME),
    Level::Puppy => (*D::puppy(NAME)).clone(),
    Level::Beagle => (*D::beagle(NAME)).clone(),
  }
}

/// A `Boxed` handle to a generic object of the given concrete class.
fn boxed_view<D: Design>(generic: Generic) -> D::Boxed {
  match generic {
    Generic::Boxed => D::boxed(7),
    Generic::Tagged => (*D::tagged(7)).clone(),
  }
}

/// Allocating an object, including fetching its vtable.
#[divan::bench(types = [MainDesign, SafeDesign], args = LEVELS, threads = THREADS, sample_size = ALLOC_SAMPLE_SIZE)]
fn construct<D: Design>(bencher: Bencher, level: Level) {
  match level {
    Level::Animal => bencher.bench(|| D::animal(black_box(NAME))),
    Level::Dog => bencher.bench(|| D::dog(black_box(NAME))),
    Level::Puppy => bencher.bench(|| D::puppy(black_box(NAME))),
    Level::Beagle => bencher.bench(|| D::beagle(black_box(NAME))),
  }
}

/// Cloning a handle of the object's own class. The safe design bumps one refcount per ancestor level.
#[divan::bench(types = [MainDesign, SafeDesign], args = LEVELS, threads = THREADS)]
fn clone<D: Design>(bencher: Bencher, level: Level) {
  match level {
    Level::Animal => {
      let h = D::animal(NAME);
      bencher.bench(|| black_box(&h).clone());
    }
    Level::Dog => {
      let h = D::dog(NAME);
      bencher.bench(|| black_box(&h).clone());
    }
    Level::Puppy => {
      let h = D::puppy(NAME);
      bencher.bench(|| black_box(&h).clone());
    }
    Level::Beagle => {
      let h = D::beagle(NAME);
      bencher.bench(|| black_box(&h).clone());
    }
  }
}

/// Reading a base-class field through an `Animal` view.
#[divan::bench(types = [MainDesign, SafeDesign], args = LEVELS, threads = THREADS)]
fn field<D: Design>(bencher: Bencher, level: Level) {
  let a = animal_view::<D>(level);
  bencher.bench(|| D::age(black_box(&a)));
}

/// A virtual method nobody overrides. The safe design forwards through `__parent()` once per level.
#[divan::bench(types = [MainDesign, SafeDesign], args = LEVELS, threads = THREADS)]
fn legs<D: Design>(bencher: Bencher, level: Level) {
  let a = animal_view::<D>(level);
  bencher.bench(|| D::legs(black_box(&a)));
}

/// A virtual method with the base body, `Dog`'s override, `Puppy`'s override plus a `super` call, and
/// `Beagle` inheriting `Dog`'s override.
#[divan::bench(types = [MainDesign, SafeDesign], args = LEVELS, threads = THREADS)]
fn weight<D: Design>(bencher: Bencher, level: Level) {
  let a = animal_view::<D>(level);
  bencher.bench(|| D::weight(black_box(&a)));
}

/// A virtual method declared one level down, through a `Dog` view: `Dog`'s body, `Puppy`'s override plus a
/// `super` call, and `Beagle` inheriting `Dog`'s body.
#[divan::bench(types = [MainDesign, SafeDesign], args = DOG_LEVELS, threads = THREADS)]
fn bark_volume<D: Design>(bencher: Bencher, level: Level) {
  let d = dog_view::<D>(level);
  bencher.bench(|| D::bark_volume(black_box(&d)));
}

/// A realistic non-virtual base method that reads a `String` field and late-binds to `speak`.
#[divan::bench(types = [MainDesign, SafeDesign], args = LEVELS, threads = THREADS, sample_size = ALLOC_SAMPLE_SIZE)]
fn describe<D: Design>(bencher: Bencher, level: Level) {
  let a = animal_view::<D>(level);
  bencher.bench(|| D::describe(black_box(&a)));
}

/// `Animal` → `Dog` downcast: fails for an `Animal`, succeeds otherwise.
#[divan::bench(types = [MainDesign, SafeDesign], args = LEVELS, threads = THREADS)]
fn downcast<D: Design>(bencher: Bencher, level: Level) {
  let a = animal_view::<D>(level);
  bencher.bench(|| D::downcast_dog(black_box(&a)));
}

/// Allocating a generic object. The main design fetches its vtable from the registry.
#[divan::bench(types = [MainDesign, SafeDesign], args = GENERICS, threads = THREADS, sample_size = ALLOC_SAMPLE_SIZE)]
fn generic_construct<D: Design>(bencher: Bencher, generic: Generic) {
  match generic {
    Generic::Boxed => bencher.bench(|| D::boxed(black_box(7))),
    Generic::Tagged => bencher.bench(|| D::tagged(black_box(7))),
  }
}

/// A generic virtual method: the base body, and `Tagged`'s override plus a `super` call, which in the main
/// design fetches the parent's vtable from the registry.
#[divan::bench(types = [MainDesign, SafeDesign], args = GENERICS, threads = THREADS)]
fn generic_get<D: Design>(bencher: Bencher, generic: Generic) {
  let b = boxed_view::<D>(generic);
  bencher.bench(|| D::get(black_box(&b)));
}
