#![forbid(unsafe_code)]

use galvanize_rs_main_models::{Animal, Beagle, Boxed, Dog, Puppy, Tagged, class, class_methods};
use galvanize_rs_main_runtime::{Class, ClassData, Handle, view};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[class(open)]
pub struct Empty {}
#[class_methods]
impl Empty {}
#[class(extends = Empty)]
pub struct EmptyChild {}
#[class_methods]
impl EmptyChild {}

#[derive(Clone)]
#[repr(align(128))]
pub struct Aligned(pub u64);

#[class(open, extends = Dog)]
pub struct CallbackDog {
  pub payload: Aligned,
}
#[class_methods]
impl CallbackDog {
  #[class_override]
  pub fn weight(&self) -> u64 {
    self.super_weight() + self.payload().0
  }

  #[class_virtual]
  pub fn call(&self, callback: fn(&Animal) -> u64) -> u64 {
    callback(self)
  }

  #[class_virtual]
  pub fn fail(&self) {
    panic!("intentional method panic");
  }

  #[class_virtual]
  pub async fn async_fail(&self) {
    let held = self.name();
    panic!("intentional async panic: {held}");
  }
}

#[class(extends = CallbackDog)]
pub struct Deeper {}
#[class_methods]
impl Deeper {
  #[class_override]
  pub fn weight(&self) -> u64 {
    self.super_weight() * 2
  }
}

#[test]
fn checked_carriers_and_borrowed_views_cannot_forge_a_descendant() {
  let animal = Animal::new("root");
  assert!(view::<_, Dog>(&animal).is_none());
  assert!(animal.handle().downcast::<Dog>().is_none());
  let puppy = Puppy::new("Bit");
  let base: Animal = puppy.upcast();
  let recovered = view::<_, Puppy>(&base).expect("checked borrowed view");
  assert!(base.ptr_eq(recovered));
  assert_eq!(recovered.weight(), 7);
  assert!(view::<_, Beagle>(&base).is_none());
  assert!(base.downcast::<Boxed<u64>>().is_none());
  let fields = <Animal as ClassData>::build(Animal::init("typed"));
  let valid = Animal::from_handle(Handle::<Animal>::new(fields));
  assert_eq!(valid.name(), "typed");
}

#[test]
fn empty_hierarchies_and_aligned_generic_payloads_keep_valid_prefixes() {
  let child = EmptyChild::from_data(EmptyChildData { base: EmptyData {} });
  let base: Empty = child.upcast();
  drop(child);
  assert!(base.downcast::<EmptyChild>().is_some());
  assert!(base.downcast::<Dog>().is_none());
  let generic = Tagged::new(Aligned(42));
  let base: Boxed<Aligned> = generic.upcast();
  drop(generic);
  assert_eq!(base.get().0, 42);
  base.set_value(Aligned(99));
  assert_eq!(base.downcast::<Tagged<Aligned>>().expect("aligned child").get().0, 99);
  let zero = Tagged::new(());
  let base: Boxed<()> = zero.upcast();
  assert_eq!(base.get(), ());
}

#[test]
fn lexical_super_keeps_its_declaring_parent_and_reentrant_calls_do_not_hold_registry_locks() {
  let object = Deeper::from_data(DeeperData {
    base: CallbackDogData {
      base: Dog::init("callback"),
      payload: Aligned(5),
    },
  });
  let animal: Animal = object.upcast();
  assert_eq!(animal.weight(), 70);
  assert_eq!(
    object.call(|a| a.downcast::<CallbackDog>().expect("callback view").weight()),
    70
  );
  assert!(animal.downcast::<Puppy>().is_none());
  let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| object.fail()));
  assert!(result.is_err());
  assert_eq!(animal.weight(), 70);
  let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
    use std::future::Future;
    let mut future = std::pin::pin!(object.async_fail());
    let _ = future
      .as_mut()
      .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
  }));
  assert!(result.is_err());
  assert_eq!(animal.name(), "callback");
}

#[derive(Clone)]
struct Counted(Arc<AtomicUsize>);
impl Drop for Counted {
  fn drop(&mut self) {
    self.0.fetch_add(1, Ordering::SeqCst);
  }
}

#[test]
fn cold_generic_tables_race_and_mutation_remains_synchronized() {
  #[derive(Clone)]
  struct Cold(u64);
  let threads: Vec<_> = (0..4)
    .map(|i| {
      std::thread::spawn(move || {
        let object = Tagged::new(Cold(i));
        assert_eq!(object.get().0, i);
      })
    })
    .collect();
  for thread in threads {
    thread.join().expect("cold table race");
  }

  let object = Puppy::new("race");
  std::thread::scope(|scope| {
    for i in 0..4 {
      let animal: Animal = object.upcast();
      scope.spawn(move || {
        animal.set_age(i);
        assert!(animal.age() < 4);
        assert_eq!(animal.weight(), 7);
      });
    }
  });
  let drops = Arc::new(AtomicUsize::new(0));
  let object = Tagged::new(Counted(drops.clone()));
  let base: Boxed<Counted> = object.upcast();
  drop(object);
  let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
    let _last_owner = base;
    panic!("unwind last owner");
  }));
  assert!(result.is_err());
  assert_eq!(drops.load(Ordering::SeqCst), 1);
}
