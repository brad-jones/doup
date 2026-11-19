use models::{Animal, Boxed as ImportedBox, BoxedData, Dog as ImportedDog, Puppy, Tagged};
use models::{class, class_methods};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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

  #[class_override]
  pub fn weight(&self) -> u64 {
    self.super_weight() + self.age()
  }

  #[class_virtual]
  pub fn score(&self, extra: u64) -> u64 {
    self.weight() + extra
  }
}

#[class(extends = GuideDog)]
pub struct SmallGuide {}

#[class_methods]
impl SmallGuide {
  #[class_override]
  pub fn weight(&self) -> u64 {
    self.super_weight() / 3
  }

  #[class_override]
  pub fn score(&self, extra: u64) -> u64 {
    self.super_score(extra) * 2
  }
}

#[class(open, extends = ImportedBox<U>)]
pub struct Labeled<U> {
  pub label: String,
}

#[class_methods]
impl<U> Labeled<U> {
  #[class_override]
  pub fn get(&self) -> U {
    assert_eq!(self.label(), "typed");
    self.super_get()
  }
}

#[class(extends = Labeled<Vec<C>>)]
pub struct Collected<C> {}

#[class_methods]
impl<C> Collected<C> {
  #[class_override]
  pub fn get(&self) -> Vec<C> {
    self.super_get()
  }
}

#[test]
fn descriptors_cross_crates_and_renamed_imports() {
  let small = SmallGuide::from_data(SmallGuideData {
    base: GuideDogData {
      base: ImportedDog::init("Ada"),
      badge: "guide".into(),
    },
  });
  let animal: Animal = small.upcast();
  let dog: ImportedDog = small.upcast();
  assert!(animal.ptr_eq(&small));
  assert_eq!(animal.describe(), "Ada says guide: Woof");
  assert_eq!(dog.weight(), 11);
  assert_eq!(small.score(3), 28);
  assert_eq!(dog.bark_volume(), 8);
  assert!(animal.is::<GuideDog>() && animal.is::<SmallGuide>());
  assert!(animal.downcast::<Puppy>().is_none());
  let recovered = animal.downcast::<SmallGuide>().expect("same most-derived object");
  assert!(small.ptr_eq(&recovered));
}

#[test]
fn generic_metadata_substitutes_nested_type_arguments() {
  let object = Collected::from_data(CollectedData {
    base: LabeledData {
      base: BoxedData { value: vec![2_u64, 3] },
      label: "typed".into(),
    },
  });
  let base: ImportedBox<Vec<u64>> = object.upcast();
  assert_eq!(base.get(), vec![2, 3]);
  base.set_value(vec![5]);
  assert_eq!(object.get(), vec![5]);
  assert!(base.downcast::<Collected<String>>().is_none());
  assert!(base.downcast::<Collected<u64>>().is_some());
}

#[test]
fn mutations_and_last_owner_survive_base_views_and_threads() {
  let puppy = Puppy::new("Bit");
  let animal: Animal = puppy.upcast();
  puppy.set_age(9);
  drop(puppy);
  std::thread::spawn(move || {
    assert_eq!(animal.age(), 9);
    animal.set_name("Rex".into());
    let puppy = animal.downcast::<Puppy>().expect("retained dynamic class");
    assert_eq!(puppy.describe(), "Rex says Woof (but squeakier)");
  })
  .join()
  .expect("thread completed");
}

#[derive(Clone)]
struct Counted(Arc<AtomicUsize>);

impl Drop for Counted {
  fn drop(&mut self) {
    self.0.fetch_add(1, Ordering::SeqCst);
  }
}

#[test]
fn typed_views_do_not_form_ownership_cycles() {
  let drops = Arc::new(AtomicUsize::new(0));
  let object = Tagged::new(Counted(drops.clone()));
  let base: ImportedBox<Counted> = object.upcast();
  let recovered = base.downcast::<Tagged<Counted>>().expect("matching generic class");
  drop(object);
  drop(recovered);
  assert_eq!(drops.load(Ordering::SeqCst), 0);
  drop(base);
  assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
#[cfg(not(miri))]
fn compile_time_contracts() {
  trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
