use galvanize_rs::{ArcSwapStrategy, GPointer, class};

#[class]
#[derive(Debug, Default)]
pub struct Node {
  value: i32,
  next: Option<Node>,
  r#type: String,
}

#[test]
fn handles_share_one_object_and_compare_by_identity() {
  let a = Node::default();
  let b = a.clone();
  let c = Node::default();

  b.set_value(5);
  assert_eq!(a.value(), 5);
  assert_eq!(a, b);
  assert_ne!(a, c);
}

#[test]
fn getters_of_class_fields_return_shared_handles() {
  let tail = Node::default();
  let head = Node::default();
  head.set_next(Some(tail.clone()));

  head.next().unwrap().set_value(9);
  assert_eq!(tail.value(), 9);
  assert_eq!(head.next(), Some(tail));
}

#[test]
fn raw_identifier_fields_get_clean_setters() {
  let n = Node::default();
  n.set_type("b".into());
  assert_eq!(n.r#type(), "b");
}

#[test]
fn gpointer_display_eq_and_from() {
  let p: GPointer<i32> = 2.into();
  assert_eq!(p, 2);
  assert_eq!(p.to_string(), "2");
  assert_eq!(p, GPointer::new(2));
}

#[class]
#[derive(Default)]
struct Outer {
  inner: Node,
}

#[test]
fn class_typed_fields_default_to_a_fresh_object() {
  let o = Outer::default();
  assert_ne!(o.inner(), Node::default());
  o.inner().set_value(4);
  assert_eq!(o.inner().value(), 4);
}

#[class]
struct Custom {
  n: i32,
}

impl Custom {
  fn new(n: i32) -> Self {
    Self::from(CustomData { n })
  }
}

#[test]
fn users_can_write_their_own_constructor() {
  assert_eq!(Custom::new(7).n(), 7);
}

#[class(pointer_strategy = ArcSwapStrategy)]
#[derive(Default)]
struct Swapped {
  n: i32,
}

#[test]
fn strategy_option_selects_the_pointer_strategy() {
  let s = Swapped::default();
  let _: &GPointer<SwappedData, ArcSwapStrategy> = &s;
  s.clone().set_n(3);
  assert_eq!(s.n(), 3);
}

#[class]
#[derive(Default)]
struct Pair<A, B>
where
  B: Send,
{
  a: A,
  b: B,
}

#[test]
fn generic_classes_share_state() {
  let p: Pair<i32, String> = Pair::default();
  let q = p.clone();
  q.set_a(1);
  q.set_b("x".into());
  assert_eq!((p.a(), p.b()), (1, "x".to_string()));
  assert!(p == q);
}

#[class]
#[derive(Debug, Default)]
struct Holder<T> {
  item: T,
}

#[test]
fn generic_debug_names_type_arguments() {
  let h: Holder<Vec<String>> = Holder::default();
  assert_eq!(
    format!("{h:?}"),
    "Holder<Vec<String>>(GPointer<ArcRwLockStrategy>(HolderData { item: [] }))"
  );
}

#[class]
#[derive(Debug, Default)]
struct Bag {
  /// Items in the bag.
  items: Vec<i32>,
}

#[test]
fn with_and_update_avoid_cloning_and_mutate_in_place() {
  let bag = Bag::default();
  let alias = bag.clone();
  alias.update_items(|v| v.push(1));
  bag.update_items(|v| v.push(2));
  assert_eq!(bag.with_items(|v| v.len()), 2);
  assert_eq!(alias.items(), vec![1, 2]);
}

#[test]
fn duplicate_returns_an_independent_class() {
  let a = Bag::default();
  a.update_items(|v| v.push(1));
  let b: Bag = a.duplicate();
  b.update_items(|v| v.push(2));
  assert_ne!(a, b);
  assert_eq!(a.items(), vec![1]);
  assert_eq!(b.items(), vec![1, 2]);
}

#[test]
fn colliding_field_names_are_rejected() {
  let cases = trybuild::TestCases::new();
  cases.compile_fail("tests/ui/*.rs");
}
