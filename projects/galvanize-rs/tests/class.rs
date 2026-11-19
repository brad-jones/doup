use galvanize_rs::{
  ArcMutexPointer, ArcMutexStrategy, ArcSwapStrategy, Extends, List, MutablePointer, Str, Strategy, class,
};
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

  #[class(crate = galvanize, extends = Thing)]
  pub struct SubThing {
    pub n: i64,
  }

  #[test]
  fn crate_path_can_be_overridden() {
    let t = Thing::from_data(ThingData { id: 1 });
    assert_eq!(t.id(), 1);
    let s = SubThing::from_data(SubThingData { base: t, n: 2 });
    assert_eq!((s.id(), s.n()), (1, 2));
    assert_eq!(format!("{s:?}"), "SubThing { id: 1, n: 2 }");
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

// ---------- generics ----------

#[class]
pub struct Node<T> {
  pub value: T,
  pub next: Option<Self>,
}

impl<T> Node<T>
where
  T: Clone,
{
  fn new(value: T) -> Self {
    Self::from_data(NodeData { value, next: None })
  }

  fn len(&self) -> usize {
    1 + self.next().map_or(0, |n| n.len())
  }
}

#[test]
fn generic_classes_get_typed_accessors() {
  let head = Node::new(Str::from("a"));
  head.set_next(Node::new("b".into()));
  let doubled = head.update_value(|v| {
    *v = v.clone() + "a";
    v.len()
  });

  assert_eq!(doubled, 2);
  assert_eq!(head.value(), "aa");
  assert_eq!(head.len(), 2);
  assert_eq!(Node::from(&head), head);
  assert_eq!(
    format!("{head:?}"),
    r#"Node { value: "aa", next: Some(Node { value: "b", next: None }) }"#
  );

  let worker = head.clone();
  std::thread::spawn(move || worker.next().unwrap().set_value("c"))
    .join()
    .unwrap();
  assert_eq!(head.next().unwrap().value(), "c");
}

#[derive(Clone)]
struct NoDebug;

#[test]
fn generic_classes_only_need_debug_for_debug() {
  let n = Node::new(NoDebug);
  assert!(n.next().is_none());
}

/// Type parameters named like the generated methods' own don't clash with them.
#[class]
#[derive(Default)]
struct Pair<R, H = i64>
where
  R: Default,
{
  left: R,
  right: H,
}

#[test]
fn generic_classes_keep_defaults_and_where_clauses() {
  let p: Pair<Str> = Pair::from_data(PairData::default());
  p.update_right(|r| *r += 1);
  p.set_left("a");

  #[allow(clippy::mutable_key_type)]
  let set: HashSet<Pair<Str>> = [p.clone(), p.clone()].into_iter().collect();
  assert_eq!(set.len(), 1);
  assert_eq!((p.left(), p.right()), (Str::from("a"), 1));
}

/// `S` is only used by the strategies, so `StoreData` doesn't get it, and it needn't be `Clone`.
#[class(strategy = S)]
struct Store<T, S: Strategy = galvanize_rs::DefaultStrategy> {
  items: List<T>,
  #[class_prop(strategy = S)]
  count: usize,
  #[class_prop(strategy = ArcSwapStrategy)]
  label: Str,
}

#[test]
fn generic_strategies_apply_to_fields() {
  let before = COUNTED_POINTERS.load(Ordering::SeqCst);
  let store: Store<i64, Counting> = Store::from_data(StoreData {
    items: List::new(),
    count: 0,
    label: "nums".into(),
  });
  assert_eq!(COUNTED_POINTERS.load(Ordering::SeqCst) - before, 2);

  store.items().push(1);
  store.update_count(|c| *c += 1);
  assert_eq!(
    (store.items().len(), store.count(), store.label()),
    (1, 1, Str::from("nums"))
  );
  assert_eq!(format!("{store:?}"), r#"Store { items: [1], count: 1, label: "nums" }"#);
}

#[class]
struct Borrowed<'a> {
  name: &'a str,
}

#[class]
struct Buffer<const N: usize> {
  bytes: [u8; N],
}

#[test]
fn lifetime_and_const_generics_are_supported() {
  let owned = String::from("Ada");
  let b = Borrowed::from_data(BorrowedData { name: &owned });
  assert_eq!(b.name(), "Ada");

  let buf = Buffer::from_data(BufferData { bytes: [0; 4] });
  buf.update_bytes(|b| b[0] = 1);
  assert_eq!(buf.bytes(), [1, 0, 0, 0]);
}

#[class]
struct Chain {
  pub id: i64,
  pub parent: Option<Self>,
}

#[test]
fn self_in_field_types_refers_to_the_class() {
  let root = Chain::from_data(ChainData { id: 1, parent: None });
  let child = Chain::from_data(ChainData {
    id: 2,
    parent: Some(root.clone()),
  });
  assert_eq!(child.parent(), Some(root));
}

// ---------- inheritance ----------

#[class]
pub struct Animal {
  pub name: Str,
  legs: i64,
}

impl Animal {
  fn new(name: &str, legs: i64) -> Self {
    Self::from_data(AnimalData {
      name: name.into(),
      legs,
    })
  }

  fn describe(&self) -> Str {
    galvanize_rs::sf!("{} has {} legs", self.name(), self.legs())
  }

  fn kind(&self) -> &'static str {
    "animal"
  }
}

#[class(extends = Animal)]
pub struct Dog {
  pub tricks: List<Str>,
}

impl Dog {
  fn new(name: &str) -> Self {
    Self::from_data(DogData {
      base: Animal::new(name, 4),
      tricks: List::new(),
    })
  }

  fn kind(&self) -> &'static str {
    "dog"
  }
}

#[class(extends = Dog, strategy = ArcSwapStrategy)]
pub struct Puppy {
  pub age_weeks: i64,
}

impl Puppy {
  fn new(name: &str) -> Self {
    Self::from_data(PuppyData {
      base: Dog::new(name),
      age_weeks: 8,
    })
  }
}

#[class(extends = Animal)]
struct Snake {}

fn legs_of(animal: &Animal) -> i64 {
  animal.legs()
}

fn name_of<I>(animal: impl Extends<Animal, I>) -> Str {
  animal.upcast().name()
}

#[test]
fn subclasses_inherit_accessors_and_methods() {
  let p = Puppy::new("Rex");
  p.set_name("Max");
  p.tricks().push("sit");
  p.update_age_weeks(|w| *w += 1);

  assert_eq!(p.describe(), "Max has 4 legs");
  assert_eq!(p.tricks().len(), 1);
  assert_eq!(p.age_weeks(), 9);
  assert_eq!(legs_of(&p), 4);
}

#[test]
fn subclass_methods_shadow_base_methods_statically() {
  let d = Dog::new("Rex");
  let as_animal: &Animal = &d;
  assert_eq!(d.kind(), "dog");
  assert_eq!(d.base().kind(), "animal");
  assert_eq!(as_animal.kind(), "animal");
}

#[test]
fn upcasting_returns_the_shared_base_object() {
  let p = Puppy::new("Rex");
  let dog: Dog = p.base();
  let animal: Animal = p.upcast();

  assert!(animal.ptr_eq(&dog.base()));
  assert_eq!(Dog::from(&p), dog);
  assert_eq!(Animal::from(dog.clone()), animal);
  assert_eq!(p.upcast::<Dog, _>(), dog);
  assert_eq!(p.upcast::<Puppy, _>(), p);

  animal.set_name("Max");
  assert_eq!(p.name(), "Max");
  assert_eq!(name_of(p.clone()), "Max");
  assert_eq!(name_of(dog), "Max");
  assert_eq!(name_of(animal), "Max");
}

#[test]
fn subclasses_sharing_a_base_keep_their_own_identity() {
  let shared = Animal::new("Rex", 4);
  let a = Dog::from_data(DogData {
    base: shared.clone(),
    tricks: List::new(),
  });
  let b = Dog::from_data(DogData {
    base: shared,
    tricks: List::new(),
  });
  a.set_name("Max");

  assert_ne!(a, b);
  assert_eq!(b.name(), "Max");
  assert!(a.base().ptr_eq(&b.base()));
}

#[test]
fn debug_includes_base_fields() {
  let p = Puppy::new("Rex");
  p.tricks().push("sit");
  assert_eq!(
    format!("{p:?}"),
    r#"Puppy { name: "Rex", legs: 4, tricks: ["sit"], age_weeks: 8 }"#
  );

  let s = Snake::from_data(SnakeData {
    base: Animal::new("Kaa", 0),
  });
  assert_eq!(format!("{s:?}"), r#"Snake { name: "Kaa", legs: 0 }"#);
}

#[test]
fn subclass_handles_are_send_and_sync() {
  let p = Puppy::new("Rex");
  let worker = p.clone();
  std::thread::spawn(move || worker.upcast::<Animal, _>().set_name("Max"))
    .join()
    .unwrap();
  assert_eq!(p.name(), "Max");
}

#[class]
pub struct Boxed<T> {
  pub value: T,
}

#[class(extends = Boxed<T>)]
pub struct Labelled<T> {
  pub label: Str,
}

#[class(extends = Labelled<i64>)]
pub struct Counted {
  pub count: usize,
}

fn value_of<T: Clone, I>(boxed: impl Extends<Boxed<T>, I>) -> T {
  boxed.upcast().value()
}

#[test]
fn generic_classes_can_be_extended() {
  let labelled = Labelled::from_data(LabelledData {
    base: Boxed::from_data(BoxedData { value: Str::from("a") }),
    label: "x".into(),
  });
  labelled.set_value("b");
  assert_eq!(value_of(labelled.clone()), "b");
  assert_eq!(format!("{labelled:?}"), r#"Labelled { value: "b", label: "x" }"#);

  let counted = Counted::from_data(CountedData {
    base: Labelled::from_data(LabelledData {
      base: Boxed::from_data(BoxedData { value: 1 }),
      label: "n".into(),
    }),
    count: 2,
  });
  counted.update_value(|v| *v += 1);
  assert_eq!(value_of(counted.clone()), 2);
  assert_eq!(counted.upcast::<Labelled<i64>, _>().label(), "n");
  assert_eq!(format!("{counted:?}"), r#"Counted { value: 2, label: "n", count: 2 }"#);
}

#[test]
fn generic_subclasses_only_need_debug_for_debug() {
  let labelled = Labelled::from_data(LabelledData {
    base: Boxed::from_data(BoxedData { value: NoDebug }),
    label: "x".into(),
  });
  assert_eq!(labelled.label(), "x");
}

/// `base` is only reserved on subclasses.
#[class]
struct Statue {
  base: Str,
}

#[test]
fn base_is_a_valid_field_name_without_extends() {
  let s = Statue::from_data(StatueData { base: "marble".into() });
  assert_eq!(s.base(), "marble");
}
