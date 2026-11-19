use galvanize_rs::{ArcMutexPointer, ArcMutexStrategy, ArcSwapStrategy, List, MutablePointer, Str, Strategy, class};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

#[class]
pub struct Person {
  pub name: Str,
  pub age: i64,
  friends: List<Str>,
}

impl Person {
  fn new(name: &str) -> Self {
    Self::from_data(PersonData {
      name: name.into(),
      age: 0,
      friends: List::new(),
    })
  }

  fn befriend(&self, name: &str) {
    self.friends().push(name);
  }

  fn friend_count(&self) -> usize {
    self.friends().len()
  }
}

#[test]
fn accessors_read_and_write_fields() {
  let p = Person::new("Ada");
  p.set_name("Grace");
  p.set_age(36);
  let updated = p.update_age(|a| {
    *a += 1;
    *a
  });
  p.befriend("Alan");

  assert_eq!(updated, 37);
  assert_eq!(p.name(), "Grace");
  assert_eq!(p.age(), 37);
  assert_eq!(p.friend_count(), 1);
}

#[test]
fn clones_share_the_same_object() {
  let p = Person::new("Ada");
  let alias = p.clone();
  alias.set_age(5);
  alias.befriend("Alan");

  assert_eq!(p.age(), 5);
  assert_eq!(p.friend_count(), 1);
  assert_eq!(Person::from(&p), p);
}

#[test]
fn equality_and_hash_use_identity() {
  let a = Person::new("Ada");
  let b = Person::new("Ada");
  assert_ne!(a, b);
  assert!(a.ptr_eq(&a.clone()));

  // Hash and Eq ignore the (interior mutable) field values, so this is sound.
  #[allow(clippy::mutable_key_type)]
  let set: HashSet<Person> = [a.clone(), a.clone(), b].into_iter().collect();
  assert_eq!(set.len(), 2);
  assert!(set.contains(&a));
}

#[test]
fn debug_prints_field_values() {
  let p = Person::new("Ada");
  p.befriend("Alan");
  assert_eq!(format!("{p:?}"), r#"Person { name: "Ada", age: 0, friends: ["Alan"] }"#);
}

#[test]
fn handles_are_send_and_sync() {
  let p = Person::new("Ada");
  let worker = p.clone();
  std::thread::spawn(move || worker.update_age(|a| *a += 1))
    .join()
    .unwrap();
  assert_eq!(p.age(), 1);
}

#[test]
fn update_may_access_other_fields() {
  let p = Person::new("Ada");
  p.update_name(|n| *n = galvanize_rs::sf!("{n} ({})", p.age()));
  assert_eq!(p.name(), "Ada (0)");
}

#[class]
struct Keyword {
  r#type: Str,
}

#[test]
fn raw_identifier_fields_get_bare_accessor_names() {
  let k = Keyword::from_data(KeywordData { r#type: "a".into() });
  k.set_type("b");
  k.update_type(|t| *t = t.clone() + "c");
  assert_eq!(k.r#type(), "bc");
  assert_eq!(format!("{k:?}"), r#"Keyword { type: "bc" }"#);
}

/// Attributes other than docs are applied to the data struct.
#[class]
#[derive(Default)]
struct Settings {
  pub verbose: bool,
  pub level: i64,
}

#[test]
fn struct_attributes_apply_to_data() {
  let s = Settings::from_data(SettingsData {
    level: 3,
    ..Default::default()
  });
  assert!(!s.verbose());
  assert_eq!(s.level(), 3);
}

mod renamed {
  use ::galvanize_rs as galvanize;
  use galvanize::class;

  #[class(crate = galvanize)]
  pub struct Thing {
    pub id: i64,
  }

  #[test]
  fn crate_path_can_be_overridden() {
    let t = Thing::from_data(ThingData { id: 1 });
    assert_eq!(t.id(), 1);
  }
}

// ---------- strategies ----------

static COUNTED_POINTERS: AtomicUsize = AtomicUsize::new(0);

/// A strategy whose pointers count how many times they have been created.
struct Counting;

#[derive(Clone)]
struct CountingPointer<T>(ArcMutexPointer<T>);

impl<T: Clone> MutablePointer<T> for CountingPointer<T> {
  fn new(value: T) -> Self {
    COUNTED_POINTERS.fetch_add(1, Ordering::SeqCst);
    Self(ArcMutexPointer::new(value))
  }

  fn read_with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
    self.0.read_with(f)
  }

  fn write_with<R>(&self, f: impl FnMut(&mut T) -> R) -> R {
    self.0.write_with(f)
  }
}

impl Strategy for Counting {
  type Ptr<T: Clone> = CountingPointer<T>;
}

#[class(strategy = Counting)]
struct AllCounted {
  a: i64,
  b: i64,
  #[class_prop(strategy = ArcSwapStrategy)]
  c: i64,
}

#[class]
struct OneCounted {
  a: i64,
  #[class_prop(strategy = Counting)]
  b: i64,
}

#[class(strategy = ArcMutexStrategy)]
struct Mutexed {
  #[class_prop(strategy = ArcSwapStrategy)]
  pub list: List<i64>,
  pub n: i64,
}

#[test]
fn strategies_apply_per_class_and_per_field() {
  let before = COUNTED_POINTERS.load(Ordering::SeqCst);
  let all = AllCounted::from_data(AllCountedData { a: 1, b: 2, c: 3 });
  assert_eq!(COUNTED_POINTERS.load(Ordering::SeqCst) - before, 2);

  let one = OneCounted::from_data(OneCountedData { a: 1, b: 2 });
  assert_eq!(COUNTED_POINTERS.load(Ordering::SeqCst) - before, 3);

  all.set_c(4);
  one.set_b(5);
  assert_eq!((all.a(), all.b(), all.c()), (1, 2, 4));
  assert_eq!((one.a(), one.b()), (1, 5));

  let m = Mutexed::from_data(MutexedData {
    list: List::new(),
    n: 0,
  });
  m.list().push(1);
  m.update_n(|n| *n += 1);
  assert_eq!((m.list().len(), m.n()), (1, 1));
}
