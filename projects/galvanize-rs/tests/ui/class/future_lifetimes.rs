// An async virtual method's future borrows its receiver and arguments until it completes.
fn main() {
  let safe = galvanize_rs_fixtures_safe::AsyncAnimal::new("root");
  let future;
  {
    let prefix = String::from("temporary");
    future = safe.speak(&prefix);
  }
  drop(future);
  let future = safe.speak("");
  drop(safe);
  drop(future);

  let unsafe_ = galvanize_rs_fixtures_unsafe::AsyncAnimal::new("root");
  let future;
  {
    let prefix = String::from("temporary");
    future = unsafe_.speak(&prefix);
  }
  drop(future);
  let future = unsafe_.speak("");
  drop(unsafe_);
  drop(future);
}
