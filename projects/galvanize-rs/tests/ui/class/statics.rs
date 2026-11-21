use galvanize_rs::{class, class_methods};

#[class(abstract)]
pub struct Root {}

#[class_methods]
impl Root {
  #[class_abstract]
  pub const EXTENSIONS: &'static [&'static str];

  #[class_virtual]
  pub fn label() -> String {
    "root".into()
  }

  #[class_virtual]
  pub const DEPTH: u8 = 0;
}

// A concrete class must implement every abstract class-level member.
#[class(extends = Root)]
pub struct MissingConstant {}

#[class_methods]
impl MissingConstant {}

#[class(extends = Root)]
pub struct Unmarked {}

#[class_methods]
impl Unmarked {
  pub const EXTENSIONS: &'static [&'static str] = &[];

  pub fn label() -> String {
    "unmarked".into()
  }
}

#[class(extends = Root)]
pub struct WrongKind {}

#[class_methods]
impl WrongKind {
  pub const EXTENSIONS: &'static [&'static str] = &[];

  #[class_override]
  pub fn label(&self) -> String {
    "wrong".into()
  }

  #[class_override]
  pub fn DEPTH() -> u8 {
    1
  }
}

#[class(extends = Root)]
pub struct WrongVisibility {}

#[class_methods]
impl WrongVisibility {
  pub const EXTENSIONS: &'static [&'static str] = &[];

  #[class_override]
  fn label() -> String {
    "private".into()
  }

  #[class_override]
  pub const MISSING: u8 = 0;
}

#[class]
pub struct NotAbstract {}

#[class_methods]
impl NotAbstract {
  #[class_abstract]
  pub const NAME: &'static str;

  pub const VALUELESS: u8;

  #[class_abstract]
  pub const WITH_VALUE: u8 = 1;

  #[class_virtual]
  pub const fn constant() -> u8 {
    1
  }
}

fn main() {}
