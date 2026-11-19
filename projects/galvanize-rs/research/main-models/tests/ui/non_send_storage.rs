use galvanize_rs_main_models::{class, class_methods};
use std::rc::Rc;

#[class(open)]
pub struct Bad {
  pub value: Rc<()>,
}
#[class_methods]
impl Bad {}
fn main() {}
