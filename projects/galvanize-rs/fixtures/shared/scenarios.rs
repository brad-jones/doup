// Class behaviour that both runtime strategies must share, run against the fixture crate in scope as
// `fixtures`. Written for `SafeRuntimeStrategy`; the unsafe fixtures' build script rewrites it for
// `UnsafeRuntimeStrategy` and runs its tests under Miri only.

use fixtures::{
  Animal, Boxed as ImportedBox, BoxedData, ConfigFile, Dog as ImportedDog, Locate, Named, Polygon, Puppy, Rectangle,
  RepoConfigFile, Shape, Tagged,
};
use galvanize_rs::{
  ArcMutexPointer, ArcMutexStrategy, ArcSwapStrategy, DefaultPointerStrategy, Extends, GList, GPointer, GString,
  PointerStrategy, SafeRuntimeStrategy, class, class_methods, sf,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------- accessors and identity ----------

#[class(runtime_strategy = SafeRuntimeStrategy)]
pub struct Person {
  pub name: GString,
  pub age: i64,
  friends: GList<GString>,
}

#[class_methods]
impl Person {
  fn new(name: &str) -> Self {
    Self::from_data(PersonData {
      name: name.into(),
      age: 0,
      friends: GList::new(),
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
  p.update_name(|n| *n = sf!("{n} ({})", p.age()));
  assert_eq!(p.name(), "Ada (0)");
}

#[class(runtime_strategy = SafeRuntimeStrategy)]
struct Keyword {
  r#type: GString,
}

#[class_methods]
impl Keyword {}

#[test]
fn raw_identifier_fields_get_bare_accessor_names() {
  let k = Keyword::from_data(KeywordData { r#type: "a".into() });
  k.set_type("b");
  k.update_type(|t| *t = t.clone() + "c");
  assert_eq!(k.r#type(), "bc");
  assert_eq!(format!("{k:?}"), r#"Keyword { type: "bc" }"#);
}

/// Attributes other than docs are applied to the data struct.
#[class(runtime_strategy = SafeRuntimeStrategy)]
#[derive(Default)]
struct Settings {
  pub verbose: bool,
  pub level: i64,
}

#[class_methods]
impl Settings {}

#[test]
fn struct_attributes_apply_to_data() {
  let s = Settings::from_data(SettingsData {
    level: 3,
    ..Default::default()
  });
  assert!(!s.verbose());
  assert_eq!(s.level(), 3);
}

/// `base` is only reserved in subclasses, whose data uses it for the parent's.
#[class(runtime_strategy = SafeRuntimeStrategy)]
struct Statue {
  base: GString,
}

#[class_methods]
impl Statue {}

#[test]
fn base_is_a_valid_field_name_without_extends() {
  let s = Statue::from_data(StatueData { base: "marble".into() });
  assert_eq!(s.base(), "marble");
}

// ---------- the crate path ----------

use ::galvanize_rs as galvanize;

#[galvanize::class(crate = galvanize, open, runtime_strategy = SafeRuntimeStrategy)]
pub struct Thing {
  pub id: i64,
}

#[galvanize::class_methods]
impl Thing {}

#[galvanize::class(crate = galvanize, extends = Thing)]
pub struct SubThing {
  pub n: i64,
}

#[galvanize::class_methods]
impl SubThing {}

#[test]
fn crate_path_can_be_overridden() {
  let s = SubThing::from_data(SubThingData {
    base: ThingData { id: 1 },
    n: 2,
  });
  assert_eq!((s.id(), s.n()), (1, 2));
  assert_eq!(format!("{s:?}"), "SubThing { id: 1, n: 2 }");
}

// ---------- pointer strategies ----------

static COUNTED_POINTERS: AtomicUsize = AtomicUsize::new(0);

/// A pointer strategy whose pointers count how many times they have been created.
struct Counting;

#[derive(Clone)]
struct CountingPointer<T>(ArcMutexPointer<T>);

impl<T: Clone> GPointer<T> for CountingPointer<T> {
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

impl PointerStrategy for Counting {
  type Ptr<T: Clone> = CountingPointer<T>;
  type SyncPtr<T: Clone + Send + Sync> = CountingPointer<T>;
}

#[class(pointer_strategy = Counting, runtime_strategy = SafeRuntimeStrategy)]
struct AllCounted {
  a: i64,
  b: i64,
  #[class_prop(pointer_strategy = ArcSwapStrategy)]
  c: i64,
}

#[class_methods]
impl AllCounted {}

#[class(runtime_strategy = SafeRuntimeStrategy)]
struct OneCounted {
  a: i64,
  #[class_prop(pointer_strategy = Counting)]
  b: i64,
}

#[class_methods]
impl OneCounted {}

#[class(pointer_strategy = ArcMutexStrategy, runtime_strategy = SafeRuntimeStrategy)]
struct Mutexed {
  #[class_prop(pointer_strategy = ArcSwapStrategy)]
  pub list: GList<i64>,
  pub n: i64,
}

#[class_methods]
impl Mutexed {}

#[test]
fn pointer_strategies_apply_per_class_and_per_field() {
  let before = COUNTED_POINTERS.load(Ordering::SeqCst);
  let all = AllCounted::from_data(AllCountedData { a: 1, b: 2, c: 3 });
  let after_all = COUNTED_POINTERS.load(Ordering::SeqCst);
  let one = OneCounted::from_data(OneCountedData { a: 1, b: 2 });
  let after_one = COUNTED_POINTERS.load(Ordering::SeqCst);
  // Other tests may count pointers concurrently, so only lower bounds are certain.
  assert!(after_all - before >= 2);
  assert!(after_one - after_all >= 1);

  all.set_c(4);
  one.set_b(5);
  assert_eq!((all.a(), all.b(), all.c()), (1, 2, 4));
  assert_eq!((one.a(), one.b()), (1, 5));

  let m = Mutexed::from_data(MutexedData {
    list: GList::new(),
    n: 0,
  });
  m.list().push(1);
  m.update_n(|n| *n += 1);
  assert_eq!((m.list().len(), m.n()), (1, 1));
}

/// `S` is only used by the pointer strategies, so `StoreData` doesn't get it, and it needn't be `Clone`.
#[class(pointer_strategy = S, runtime_strategy = SafeRuntimeStrategy)]
struct Store<T, S: PointerStrategy = DefaultPointerStrategy> {
  items: GList<T>,
  #[class_prop(pointer_strategy = S)]
  count: usize,
  #[class_prop(pointer_strategy = ArcSwapStrategy)]
  label: GString,
}

#[class_methods]
impl<T, S> Store<T, S> {}

#[test]
fn generic_pointer_strategies_apply_to_fields() {
  let store: Store<i64, Counting> = Store::from_data(StoreData {
    items: GList::new(),
    count: 0,
    label: "nums".into(),
  });
  store.items().push(1);
  store.update_count(|c| *c += 1);
  assert_eq!(
    (store.items().len(), store.count(), store.label()),
    (1, 1, GString::from("nums"))
  );
  assert_eq!(format!("{store:?}"), r#"Store { items: [1], count: 1, label: "nums" }"#);
}

// ---------- generics ----------

#[class(runtime_strategy = SafeRuntimeStrategy)]
pub struct Node<T> {
  pub value: T,
  pub next: Option<Self>,
}

#[class_methods]
impl<T> Node<T> {
  fn new(value: T) -> Self {
    Self::from_data(NodeData { value, next: None })
  }

  fn len(&self) -> usize {
    1 + self.next().map_or(0, |n| n.len())
  }
}

#[test]
fn generic_classes_get_typed_accessors() {
  let head = Node::new(GString::from("a"));
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
#[class(runtime_strategy = SafeRuntimeStrategy)]
#[derive(Default)]
struct Pair<R, H = i64>
where
  R: Default,
{
  left: R,
  right: H,
}

#[class_methods]
impl<R, H> Pair<R, H> {}

#[test]
fn generic_classes_keep_defaults_and_where_clauses() {
  let p: Pair<GString> = Pair::from_data(PairData::default());
  p.update_right(|r| *r += 1);
  p.set_left("a");

  #[allow(clippy::mutable_key_type)]
  let set: HashSet<Pair<GString>> = [p.clone(), p.clone()].into_iter().collect();
  assert_eq!(set.len(), 1);
  assert_eq!((p.left(), p.right()), (GString::from("a"), 1));
}

#[class(runtime_strategy = SafeRuntimeStrategy)]
struct Chain {
  pub id: i64,
  pub parent: Option<Self>,
}

#[class_methods]
impl Chain {}

#[test]
fn self_in_field_types_refers_to_the_class() {
  let root = Chain::from_data(ChainData { id: 1, parent: None });
  let child = Chain::from_data(ChainData {
    id: 2,
    parent: Some(root.clone()),
  });
  assert_eq!(child.parent(), Some(root));
}

// ---------- inheritance, across crates ----------

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

fn small_guide() -> SmallGuide {
  SmallGuide::from_data(SmallGuideData {
    base: GuideDogData {
      base: ImportedDog::init("Ada"),
      badge: "guide".into(),
    },
  })
}

#[test]
fn descriptors_cross_crates_and_renamed_imports() {
  let small = small_guide();
  let animal: Animal = small.upcast();
  let dog: ImportedDog = small.upcast();
  assert!(animal.ptr_eq(&small));
  assert!(small.ptr_eq(&animal));
  assert_eq!(animal.describe(), "Ada says guide: Woof");
  assert_eq!(dog.weight(), 11);
  assert_eq!(small.score(3), 28);
  assert_eq!(dog.bark_volume(), 8);
  assert!(animal.is::<GuideDog>() && animal.is::<SmallGuide>());
  assert!(animal.downcast::<Puppy>().is_none());
  let recovered = animal.downcast::<SmallGuide>().expect("same most-derived object");
  assert!(small.ptr_eq(&recovered));
  assert_eq!(recovered, small);
}

#[test]
fn subclasses_inherit_accessors_and_methods() {
  let small = small_guide();
  small.set_name("Max");
  small.update_age(|age| *age += 1);
  small.set_badge("chief");

  fn age_of(animal: &Animal) -> u64 {
    animal.age()
  }

  assert_eq!(small.describe(), "Max says chief: Woof");
  assert_eq!(age_of(&small), 4);
  assert_eq!(small.tricks(), 0);
}

fn name_of<I>(animal: impl Extends<Animal, I>) -> String {
  animal.upcast().name()
}

#[test]
fn upcasting_views_the_same_object() {
  let small = small_guide();
  let guide: GuideDog = small.upcast();
  let animal: Animal = small.upcast();

  assert!(animal.ptr_eq(&guide));
  assert_eq!(GuideDog::from(&small), guide);
  assert_eq!(Animal::from(guide.clone()), animal);
  assert_eq!(small.upcast::<GuideDog, _>(), guide);
  assert_eq!(small.upcast::<SmallGuide, _>(), small);

  animal.set_name("Max".to_string());
  assert_eq!(small.name(), "Max");
  assert_eq!(name_of(small.clone()), "Max");
  assert_eq!(name_of(guide), "Max");
  assert_eq!(name_of(animal), "Max");
}

#[test]
fn debug_includes_ancestor_fields() {
  let small = small_guide();
  assert_eq!(
    format!("{small:?}"),
    r#"SmallGuide { name: "Ada", age: 3, tricks: 0, badge: "guide" }"#
  );
  let animal: Animal = small.upcast();
  assert_eq!(format!("{animal:?}"), r#"Animal { name: "Ada", age: 3 }"#);
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

fn value_of<T: Clone + Send + Sync + 'static, I>(boxed: impl Extends<ImportedBox<T>, I>) -> T {
  boxed.upcast().value()
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
  assert_eq!(value_of(object.clone()), vec![5]);
  assert!(base.downcast::<Collected<String>>().is_none());
  assert!(base.downcast::<Collected<u64>>().is_some());
  assert_eq!(format!("{object:?}"), r#"Collected { value: [5], label: "typed" }"#);
}

#[test]
fn mutations_and_last_owner_survive_base_views_and_threads() {
  let puppy = Puppy::new("Bit");
  let animal: Animal = puppy.upcast();
  puppy.set_age(9_u64);
  drop(puppy);
  std::thread::spawn(move || {
    assert_eq!(animal.age(), 9);
    animal.set_name("Rex");
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

// ---------- abstract classes and interfaces, across crates ----------

/// Final classes can live in any module, which makes their private overrides private to it.
mod shapes {
  use super::fixtures::{Polygon, PolygonData, Rectangle};
  use galvanize_rs::{class, class_methods};

  #[class(extends = Rectangle)]
  pub struct Square {}

  #[class_methods]
  impl Square {
    pub fn new(label: &str, side: f64) -> Self {
      Self::from_data(SquareData {
        base: Rectangle::init(label, side, side),
      })
    }

    /// Replaces `Rectangle`'s implementation of the abstract method, so it's marked.
    #[class_override]
    fn area(&self) -> f64 {
      self.super_area() + 0.5
    }

    pub fn scaled(&self, factor: f64) -> f64 {
      self.area() * factor
    }
  }

  #[class(extends = Polygon)]
  pub struct Triangle {
    pub width: f64,
    pub height: f64,
  }

  #[class_methods]
  impl Triangle {
    pub fn new(label: &str, width: f64, height: f64) -> Self {
      Self::from_data(TriangleData {
        base: PolygonData { label: label.into() },
        width,
        height,
      })
    }

    /// Implements an abstract method, where the marker is optional.
    #[class_override]
    fn area(&self) -> f64 {
      self.width() * self.height() / 2.0
    }

    #[class_override]
    fn corners(&self) -> u64 {
      3
    }

    #[class_override]
    fn title(&self) -> String {
      format!("triangle, not {}", self.super_title())
    }

    pub fn scaled(&self, factor: f64) -> f64 {
      self.area() * factor
    }
  }
}

use shapes::{Square, Triangle};

#[test]
fn abstract_methods_dispatch_to_implementations() {
  let polygons: Vec<Polygon> = vec![Square::new("sq", 2.0).upcast(), Triangle::new("tri", 3.0, 4.0).upcast()];
  let areas: Vec<f64> = polygons.iter().map(Shape::area).collect();
  assert_eq!(areas, [4.5, 6.0]);
  assert_eq!(polygons[0].summary(), "4 corners, area 4.5");
  assert_eq!(polygons[1].summary(), "3 corners, area 6");
  assert_eq!(polygons[0].scaled(2.0), 9.0);
  assert_eq!(polygons[1].describe(), "triangle, not polygon tri: 6");
}

#[test]
fn subclasses_reach_interface_methods_through_their_ancestors() {
  let square = Square::new("sq", 3.0);
  // `Square`'s own `area` is private to `shapes`, so these resolve to `Polygon`'s trait impls.
  assert_eq!(square.area(), 9.5);
  assert_eq!(square.title(), "polygon sq");
  assert_eq!(
    Named::title(&*Triangle::new("tri", 1.0, 1.0)),
    "triangle, not polygon tri"
  );

  fn total<S: Shape + ?Sized>(shapes: &[&S]) -> f64 {
    shapes.iter().map(|s| s.area()).sum()
  }
  let rectangle: Rectangle = square.upcast();
  let shapes: Vec<Box<dyn Shape>> = vec![
    Box::new(rectangle.upcast::<Polygon, _>()),
    Box::new(Polygon::from(&square)),
  ];
  assert_eq!(total(&shapes.iter().map(|s| &**s).collect::<Vec<_>>()), 19.0);
}

#[test]
fn abstract_classes_support_casts_and_debug() {
  let polygon: Polygon = Square::new("sq", 1.0).upcast();
  assert!(polygon.is::<Rectangle>());
  assert!(polygon.downcast::<Triangle>().is_none());
  let rectangle: Rectangle = polygon.downcast().expect("a Square is a Rectangle");
  assert!(rectangle.ptr_eq(&polygon.downcast::<Square>().expect("the dynamic class")));
  assert_eq!(rectangle.width(), 1.0);
  assert_eq!(
    format!("{rectangle:?}"),
    r#"Rectangle { label: "sq", width: 1.0, height: 1.0 }"#
  );
}

// ---------- interfaces declared with #[class_interface] ----------

/// `#[class_interface]` traits can be declared in any module.
mod interfaces {
  use galvanize_rs::class_interface;

  #[class_interface]
  pub trait ConfigSource: Send + Sync {
    fn locate(&self, dir: &str) -> Option<String>;

    fn format(&self) -> String;

    fn checksum(&self, pair: (u8, u8)) -> u16;

    fn quote<T: ToString>(&self, value: T) -> String
    where
      Self: Sized;

    fn describe(&self) -> String {
      format!("{} file", self.format())
    }
  }
}

use interfaces::ConfigSource;

/// Without `#[class_interface]`, a trait's methods are matched by visibility.
pub trait Reloadable {
  fn reload(&self) -> u64;
}

#[class(abstract, implements = ConfigSource, runtime_strategy = SafeRuntimeStrategy)]
pub struct SourceFile {
  pub reloads: u64,
}

#[class_methods]
impl SourceFile {
  /// Not part of `ConfigSource`, which doesn't declare it.
  #[class_abstract]
  fn file_names(&self) -> Vec<String>;

  /// An ordinary method implementing `ConfigSource::locate`.
  fn locate(&self, dir: &str) -> Option<String> {
    self.file_names().first().map(|name| format!("{dir}/{name}"))
  }

  /// A public virtual method implementing `ConfigSource::format`.
  #[class_virtual]
  pub fn format(&self) -> String {
    "yaml".into()
  }

  fn checksum(&self, (a, b): (u8, u8)) -> u16 {
    u16::from(a) * 256 + u16::from(b)
  }

  fn quote<T: ToString>(&self, value: T) -> String {
    format!("'{}'", value.to_string())
  }
}

#[class(extends = SourceFile, implements = Reloadable)]
pub struct JsonSourceFile {}

#[class_methods]
impl JsonSourceFile {
  fn file_names(&self) -> Vec<String> {
    vec!["doup.json".into()]
  }

  #[class_override]
  pub fn format(&self) -> String {
    format!("json, not {}", self.super_format())
  }

  /// Implements `Reloadable` by having no visibility, as it has no descriptor.
  #[class_virtual]
  fn reload(&self) -> u64 {
    self.update_reloads(|n| {
      *n += 1;
      *n
    })
  }
}

#[test]
fn class_interfaces_match_methods_by_name() {
  let file = JsonSourceFile::new();
  let source: Box<dyn ConfigSource> = Box::new(file.upcast::<SourceFile, _>());
  assert_eq!(source.locate("/etc").as_deref(), Some("/etc/doup.json"));
  assert_eq!(source.format(), "json, not yaml");
  assert_eq!(source.describe(), "json, not yaml file");
  assert_eq!(source.checksum((1, 2)), 258);
  assert_eq!(ConfigSource::quote(&*file, 7), "'7'");

  let reloadable: &dyn Reloadable = &file;
  assert_eq!(reloadable.reload(), 1);
  assert_eq!(file.reloads(), 1);
}

// ---------- default constructors ----------

#[class(runtime_strategy = SafeRuntimeStrategy)]
pub struct Blank {}

#[class_methods]
impl Blank {}

#[test]
fn classes_without_a_constructor_get_new_and_default() {
  let a = Blank::new();
  let b = Blank::default();
  assert!(!a.ptr_eq(&b));
  assert_eq!(format!("{a:?}"), "Blank");
}

/// `init` returns the data rather than the class, so it isn't a constructor.
#[class(runtime_strategy = SafeRuntimeStrategy)]
pub struct Defaulted {
  pub name: GString,
  pub count: i64,
  pub tags: GList<GString>,
  pub next: Option<Self>,
}

#[class_methods]
impl Defaulted {
  pub fn init(name: &str) -> DefaultedData {
    DefaultedData {
      name: name.into(),
      count: 1,
      tags: GList::new(),
      next: None,
    }
  }
}

#[test]
fn default_constructors_default_every_field() {
  let d = Defaulted::new();
  assert_eq!(
    format!("{d:?}"),
    r#"Defaulted { name: "", count: 0, tags: [], next: None }"#
  );
  d.set_next(Defaulted::from_data(Defaulted::init("b")));
  assert_eq!(d.next().map(|n| n.count()), Some(1));
}

#[test]
fn default_constructors_dont_clash_with_derived_data_defaults() {
  // `Settings` derives `Default` for its data, and `Pair` also has a generic where clause.
  let s = Settings::new();
  assert_eq!((s.verbose(), s.level()), (false, 0));
  let p: Pair<GString> = Pair::default();
  assert_eq!((p.left(), p.right()), (GString::new(), 0));
}

/// A field named `new` would clash with a default constructor, so the class doesn't get one.
#[class(runtime_strategy = SafeRuntimeStrategy)]
struct Fresh {
  new: bool,
}

#[class_methods]
impl Fresh {}

#[test]
fn fields_named_new_keep_their_getter() {
  assert!(Fresh::from_data(FreshData { new: true }).new());
}

#[class(extends = ConfigFile)]
pub struct BotConfigFile {}

#[class_methods]
impl BotConfigFile {
  fn locate(&self, dir: Option<&str>) -> Option<String> {
    dir.map(|dir| format!("{dir}/{}", self.file_name()))
  }
}

#[test]
fn default_constructors_default_inherited_fields_across_crates() {
  let bot = BotConfigFile::new();
  assert_eq!(format!("{bot:?}"), r#"BotConfigFile { file_name: "", loaded: false }"#);
  bot.set_file_name("bot.yaml");
  let repo = RepoConfigFile::default();
  repo.set_file_name("doup.yaml");

  let files: Vec<Box<dyn Locate>> = vec![
    Box::new(bot.upcast::<ConfigFile, _>()),
    Box::new(ConfigFile::from(&repo)),
  ];
  let found: Vec<Option<String>> = files.iter().map(|f| f.locate(Some("/src"))).collect();
  assert_eq!(
    found,
    [Some("/src/bot.yaml".into()), Some("/src/.repo/doup.yaml".into())]
  );
  assert!(!BotConfigFile::new().ptr_eq(&bot));
}

#[class(extends = Rectangle)]
pub struct Plain {}

#[class_methods]
impl Plain {
  pub fn scaled(&self, factor: f64) -> f64 {
    self.width() * self.height() * factor
  }
}

#[test]
fn default_constructors_default_every_ancestor() {
  let plain = Plain::new();
  plain.set_width(2.0);
  plain.set_height(3.0);
  assert_eq!(format!("{plain:?}"), r#"Plain { label: "", width: 2.0, height: 3.0 }"#);
  assert_eq!(plain.upcast::<Polygon, _>().summary(), "4 corners, area 6");
  assert_eq!(plain.describe(), "polygon : 6");
}

#[class(extends = ImportedBox<V>)]
pub struct Unboxed<V> {
  pub opened: bool,
}

#[class_methods]
impl<V> Unboxed<V> {}

#[test]
fn generic_subclasses_get_default_constructors() {
  let unboxed: Unboxed<u64> = Unboxed::new();
  unboxed.set_opened(true);
  assert_eq!(unboxed.get(), 0);
  assert_eq!(format!("{unboxed:?}"), "Unboxed { value: 0, opened: true }");
}
