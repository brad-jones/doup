// Class handles are invariant in their type parameters, so higher-ranked function pointers can't be
// narrowed to a specific lifetime.
fn main() {
  let boxed = galvanize_rs_fixtures_safe::Boxed::<fn(&str)>::new(|_| {});
  let _widened: galvanize_rs_fixtures_safe::Boxed<fn(&'static str)> = boxed;
  let tagged = galvanize_rs_fixtures_safe::Tagged::<fn(&str)>::new(|_| {});
  let _widened: galvanize_rs_fixtures_safe::Tagged<fn(&'static str)> = tagged;

  let boxed = galvanize_rs_fixtures_unsafe::Boxed::<fn(&str)>::new(|_| {});
  let _widened: galvanize_rs_fixtures_unsafe::Boxed<fn(&'static str)> = boxed;
  let tagged = galvanize_rs_fixtures_unsafe::Tagged::<fn(&str)>::new(|_| {});
  let _widened: galvanize_rs_fixtures_unsafe::Tagged<fn(&'static str)> = tagged;
}
