// The macro picks the generator from the strategy's name, and checks the name really is that strategy.
use galvanize_rs::SafeRuntimeStrategy as UnsafeRuntimeStrategy;
use galvanize_rs::{class, class_methods};

#[class(runtime_strategy = UnsafeRuntimeStrategy)]
struct Disguised {
  a: String,
}

#[class_methods]
impl Disguised {}

fn main() {}
