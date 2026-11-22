mod example {
  use galvanize_rs::class;

  /// A singly linked list node that is shared by reference, like a class in C# or Java.
  #[class]
  #[derive(Debug, Default)]
  pub struct Node<T> {
    pub value: T,
    pub next: Option<Node<T>>,
  }

  impl<T: Clone> Node<T> {
    pub fn new(value: T) -> Self {
      Self::from(NodeData { value, next: None })
    }

    pub fn push_back(&self, value: T) {
      match self.next() {
        Some(next) => next.push_back(value),
        None => self.set_next(Some(Node::new(value))),
      }
    }
  }

  impl<T: Clone + std::fmt::Display> Node<T> {
    pub fn render(&self) -> String {
      match self.next() {
        Some(next) => format!("{} -> {}", self.value(), next.render()),
        None => self.value().to_string(),
      }
    }
  }
}

use example::Node;

fn main() {
  let head = Node::new(1);
  let alias = head.clone();
  alias.push_back(2);
  head.push_back(3);

  println!("{}", head.render());
  assert_eq!(head, alias);

  let names = Node::new("a".to_string());
  names.push_back("b".to_string());
  println!("{}", names.render());

  let empty: Node<i32> = Node::default();
  println!("{empty:?}");
}
