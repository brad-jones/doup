use galvanize_rs::{class, class_methods};

pub trait First {
  fn first(&self) -> u64;
}

pub trait Second {
  fn second(&self) -> u64;
}

#[class(implements = (First, Second))]
pub struct Ambiguous {}

#[class_methods]
impl Ambiguous {
  #[class_virtual]
  fn first(&self) -> u64 {
    1
  }
}

#[class(implements = First)]
pub struct NamesOtherTrait {}

#[class_methods]
impl NamesOtherTrait {
  #[class_virtual(Second)]
  fn first(&self) -> u64 {
    1
  }
}

#[class(implements = First)]
pub struct TraitMethodWithVisibility {}

#[class_methods]
impl TraitMethodWithVisibility {
  #[class_virtual(First)]
  pub fn first(&self) -> u64 {
    1
  }
}

#[class(implements = First)]
pub struct OverrideNamesTrait {}

#[class_methods]
impl OverrideNamesTrait {
  #[class_override(First)]
  fn first(&self) -> u64 {
    1
  }
}

#[class(implements = (First, First))]
pub struct Duplicate {}

#[class_methods]
impl Duplicate {}

#[class(implements = ())]
pub struct Empty {}

#[class_methods]
impl Empty {}

// The trait impl is checked against the trait as usual.
#[class(implements = First)]
pub struct NotInTrait {}

#[class_methods]
impl NotInTrait {
  #[class_virtual]
  fn first(&self) -> u64 {
    1
  }

  #[class_virtual]
  fn third(&self) -> u64 {
    3
  }
}

#[class(implements = Second)]
pub struct MissingTraitMethod {}

#[class_methods]
impl MissingTraitMethod {}

fn main() {}
