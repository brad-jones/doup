//! Adversarial checks for the adjacent defensive review, importing the original implementations unchanged.
//! The default mode checks valid uses. Run the `forge-*` modes only under Miri: they deliberately exercise
//! suspected undefined behaviour through safe client code. `variance` checks a type-identity invariant.
#![allow(dead_code)]

#[path = "../../projects/galvanize-rs/benches/inheritance/main_design.rs"]
mod main_design;
#[path = "galvanize-rs-true-inheritance-spike.rs"]
mod original_spike;
#[path = "../../projects/galvanize-rs/benches/inheritance/safe_design.rs"]
mod safe_design;

#[allow(unexpected_cfgs, reason = "standalone rustc compile-fail probe")]
mod client {
  #![forbid(unsafe_code)]

  use super::{main_design as main, original_spike as original, safe_design as safe};
  use main::Class as _;
  use std::sync::Arc;
  use std::sync::atomic::{AtomicUsize, Ordering};

  #[derive(Clone)]
  struct DropCounter(Arc<AtomicUsize>);

  impl Drop for DropCounter {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }

  #[repr(align(128))]
  #[derive(Clone)]
  struct Aligned(u64);

  pub fn valid_uses() {
    let puppy = main::Puppy::new("Bit");
    let animal: main::Animal = (**puppy).clone();
    let dog: main::Dog = animal.downcast().expect("a Puppy extends Dog");
    assert!(animal.downcast::<main::Beagle>().is_none());
    assert!(animal.downcast::<main::Boxed<u64>>().is_none());
    drop(puppy);
    drop(dog);
    assert_eq!(animal.describe(), "Bit says Woof (but squeakier)");
    std::thread::spawn(move || {
      assert_eq!(animal.weight(), 7);
      assert_eq!(animal.downcast::<main::Dog>().expect("still a Dog").bark_volume(), 4);
    })
    .join()
    .expect("valid dispatch in another thread");

    let aligned = main::Tagged::new(Aligned(99));
    assert_eq!(aligned.get().0, 99);
    assert!(aligned.downcast::<main::Tagged<u64>>().is_none());
    main::Tagged::new(()).get();

    let drops = Arc::new(AtomicUsize::new(0));
    let tagged = main::Tagged::new(DropCounter(drops.clone()));
    let boxed: main::Boxed<DropCounter> = (*tagged).clone();
    let another = boxed
      .downcast::<main::Tagged<DropCounter>>()
      .expect("preserves the dynamic type");
    drop(tagged);
    drop(another);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(boxed);
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    let puppy = safe::Puppy::new("Bit");
    let animal: safe::Animal = (**puppy).clone();
    drop(puppy);
    assert!(animal.downcast::<safe::Beagle>().is_none());
    assert_eq!(animal.describe(), "Bit says Woof (but squeakier)");
    assert_eq!(
      animal
        .downcast::<safe::Dog>()
        .expect("a Puppy extends Dog")
        .bark_volume(),
      4
    );
    println!("valid ownership, sibling rejection, alignment, ZST payload, drop and thread checks passed");
  }

  pub fn forge_bench() {
    assert!(cfg!(miri), "run the forgery probes under Miri, never natively");
    let animal = main::Animal::new("Not a Dog");
    assert!(animal.downcast::<main::Dog>().is_none());
    let dog = main::Dog::from_raw(animal.raw().clone());
    println!("{}", dog.bark_volume());
  }

  pub fn forge_original() {
    assert!(cfg!(miri), "run the forgery probes under Miri, never natively");
    use original::Class as _;
    let animal = original::Animal::new("Not a Dog");
    assert!(animal.downcast::<original::Dog>().is_none());
    let dog = original::Dog::from_raw(animal.raw().clone());
    println!("{}", dog.fetch());
  }

  type AnyLifetime = fn(&str);
  type StaticOnly = fn(&'static str);

  pub fn variance() {
    let original = main::Boxed::<AnyLifetime>::new(|_| {});
    let widened: main::Boxed<StaticOnly> = original;
    assert!(widened.is::<main::Boxed<AnyLifetime>>());
    assert!(!widened.is::<main::Boxed<StaticOnly>>());
    assert!(widened.downcast::<main::Boxed<StaticOnly>>().is_none());
    widened.get()("static input");
    println!("Boxed covariance accepted: is::<its own static class>() is false");

    let original = main::Tagged::<AnyLifetime>::new(|_| {});
    let widened: main::Tagged<StaticOnly> = original;
    assert!(!widened.is::<main::Tagged<StaticOnly>>());
    widened.get()("static input");
    println!("Tagged covariance accepted: is::<its own static class>() is false");
  }

  fn measure<T>(objects: &[T], call: impl Fn(&T) -> u64, expected: u64) -> f64 {
    let start = std::time::Instant::now();
    let mut sum = 0;
    for _ in 0..1024 {
      for object in objects {
        sum += call(std::hint::black_box(object));
      }
    }
    let elapsed = start.elapsed();
    assert_eq!(std::hint::black_box(sum), expected * 1024);
    elapsed.as_nanos() as f64 / (objects.len() * 1024) as f64
  }

  pub fn timing() {
    let mut classes: Vec<_> = (0..1024).map(|i| i % 4).collect();
    let mut state = 42_u64;
    for i in (1..classes.len()).rev() {
      state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
      classes.swap(i, ((state >> 32) as usize) % (i + 1));
    }
    for mixed in [false, true] {
      let kinds: Vec<_> = classes.iter().map(|&kind| if mixed { kind } else { 1 }).collect();
      let expected = kinds.iter().map(|&kind| [10, 30, 7, 30][kind]).sum();
      let main_objects: Vec<main::Animal> = kinds
        .iter()
        .map(|kind| match kind {
          0 => main::Animal::new("Bit"),
          1 => (*main::Dog::new("Bit")).clone(),
          2 => (**main::Puppy::new("Bit")).clone(),
          _ => (**main::Beagle::new("Bit")).clone(),
        })
        .collect();
      let safe_objects: Vec<safe::Animal> = kinds
        .iter()
        .map(|kind| match kind {
          0 => safe::Animal::new("Bit"),
          1 => (*safe::Dog::new("Bit")).clone(),
          2 => (**safe::Puppy::new("Bit")).clone(),
          _ => (**safe::Beagle::new("Bit")).clone(),
        })
        .collect();
      for trial in 0..5 {
        let (main_ns, safe_ns) = if trial % 2 == 0 {
          (
            measure(&main_objects, main::Animal::weight, expected),
            measure(&safe_objects, safe::Animal::weight, expected),
          )
        } else {
          let safe_ns = measure(&safe_objects, safe::Animal::weight, expected);
          (measure(&main_objects, main::Animal::weight, expected), safe_ns)
        };
        println!("mixed={mixed} trial={trial} main={main_ns:.2} ns/call safe={safe_ns:.2} ns/call");
      }
    }
  }

  fn isolated_threads<B>(threads: usize, make: impl Fn(u64) -> B + Sync, get: impl Fn(&B) -> u64 + Sync) -> f64 {
    let barrier = std::sync::Barrier::new(threads);
    let mut times = std::thread::scope(|scope| {
      let workers: Vec<_> = (0..threads)
        .map(|i| {
          let (barrier, make, get) = (&barrier, &make, &get);
          scope.spawn(move || {
            let object = make(i as u64);
            assert_eq!(get(&object), i as u64);
            barrier.wait();
            let start = std::time::Instant::now();
            let mut sum = 0;
            for _ in 0..100_000 {
              sum += get(std::hint::black_box(&object));
            }
            let elapsed = start.elapsed();
            assert_eq!(std::hint::black_box(sum), i as u64 * 100_000);
            elapsed.as_nanos() as f64 / 100_000.0
          })
        })
        .collect();
      workers
        .into_iter()
        .map(|worker| worker.join().expect("independent-object benchmark"))
        .collect::<Vec<_>>()
    });
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
  }

  pub fn generic_timing() {
    for threads in [1, 8] {
      for trial in 0..3 {
        let measure_main = || isolated_threads(threads, main::Tagged::<u64>::new, |o| o.get());
        let measure_safe = || isolated_threads(threads, safe::Tagged::<u64>::new, |o| o.get());
        let (main_ns, safe_ns) = if trial % 2 == 0 {
          (measure_main(), measure_safe())
        } else {
          let safe_ns = measure_safe();
          (measure_main(), safe_ns)
        };
        println!(
          "independent Tagged objects threads={threads} trial={trial} main={main_ns:.2} safe={safe_ns:.2} ns/call"
        );
      }
    }
  }

  #[cfg(review_safe_variance)]
  pub fn safe_variance_must_not_compile() {
    let original = safe::Boxed::<AnyLifetime>::new(|_| {});
    let _widened: safe::Boxed<StaticOnly> = original;
  }
}

fn main() {
  match std::env::args().nth(1).as_deref() {
    None | Some("valid") => client::valid_uses(),
    Some("forge-bench") => client::forge_bench(),
    Some("forge-original") => client::forge_original(),
    Some("variance") => client::variance(),
    Some("timing") => client::timing(),
    Some("generic-timing") => client::generic_timing(),
    Some(other) => panic!("unknown review case: {other}"),
  }
}
