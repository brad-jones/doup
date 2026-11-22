use galvanize_rs::{GList, any, class, list};

#[any]
trait Speak {
  fn name(&self) -> Name;
  fn hello(&self) -> String {
    format!("hello from {}", self.name().value())
  }
}

#[class]
struct Name {
  value: String,
}

#[class]
struct Dog {
  name: Name,
}

impl Speak for Dog {
  fn name(&self) -> Name {
    self.name()
  }
}

#[test]
fn any_wrapper_stores_different_classes_by_reference_and_value() {
  let dog = Dog::from(DogData {
    name: Name::from(NameData { value: "dog".into() }),
  });
  let by_ref: GList<AnySpeak> = list![&dog];
  by_ref.push(AnySpeak::new(dog.clone()));

  assert_eq!(by_ref.len(), 2);
  assert!(by_ref.iter().all(|s| s.hello() == "hello from dog"));
}
