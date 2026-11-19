use galvanize_rs_descriptor_models::{Puppy, class, class_methods};

#[class(extends = Puppy)]
pub struct Bad {}

#[class_methods]
impl Bad {}

fn main() {}
