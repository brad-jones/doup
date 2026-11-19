//! Procedural macros for `galvanize-rs`.
//!
//! Don't depend on this crate directly. Use the re-exports from `galvanize-rs`,
//! since the generated code refers to items in that crate.

mod class;

use proc_macro::TokenStream;

/// Turns a struct into a class-like, shared handle type.
///
/// The struct you write describes the class's fields. For a struct named
/// `Person`, the macro replaces it with:
///
/// * the handle `Person` (same name and visibility as your struct). It's a cheap
///   `Arc` clone that refers to the *same* object, like an object reference in
///   Python, Go, C# or Java. It is `Send + Sync` when the field types are.
/// * `PersonData`, a plain struct with your fields, which you pass to the private
///   `Person::from_data` constructor. This keeps your own constructor the only
///   way to create the class from outside its module.
///
/// Each field is stored in its own `MutablePointer`, chosen by a `Strategy`
/// that defaults to `DefaultStrategy`.
///
/// ```
/// use galvanize_rs::{List, Str, class, sf};
///
/// #[class]
/// pub struct Person {
///   /// The full name.
///   pub name: Str,
///   cities: List<Str>,
/// }
///
/// impl Person {
///   pub fn new(first_name: impl Into<Str>, last_name: impl Into<Str>) -> Self {
///     Self::from_data(PersonData {
///       name: sf!("{} {}", first_name.into(), last_name.into()),
///       cities: List::new(),
///     })
///   }
///
///   pub fn visit(&self, city: &str) {
///     self.cities().push(city);
///   }
///
///   pub fn visited(&self) -> usize {
///     self.cities().len()
///   }
/// }
///
/// let person = Person::new("Ada", "Lovelace");
/// let same = person.clone(); // another reference to the same object
/// same.set_name("Grace Hopper");
/// same.visit("Melbourne");
///
/// assert_eq!(person.name(), "Grace Hopper");
/// assert_eq!(person.visited(), 1);
/// assert_eq!(person, same); // classes compare by identity
/// ```
///
/// # Generated methods
///
/// For every field `x: T` the handle gets:
///
/// * `x(&self) -> T` returns a clone of the value. For handle types (`Str`,
///   `List`, `Dict`, other classes) that's the *same* shared object, so
///   `person.cities().push(..)` modifies `person`. Other types are copied, so
///   keep field types cheap to clone and avoid e.g. `Vec`.
/// * `set_x(&self, value: impl Into<T>)` replaces the value.
/// * `update_x(&self, f: impl FnMut(&mut T) -> R) -> R` modifies the value
///   in place, atomically with respect to other accessors of `x`. `f` may be
///   called more than once (see `MutablePointer::write_with`).
///
/// The getter has the field's visibility, and so do the setter and updater
/// unless the field is marked `#[class(readonly)]`.
///
/// The handle also gets `from_data` (private), `ptr_eq`, and implementations
/// of `Clone`, `PartialEq`, `Eq` and `Hash` (all by identity), `Debug` (prints
/// the field values) and `From<&Person>` (clones the handle). As `Hash` and
/// `Eq` ignore the field values, classes are sound `HashMap`/`HashSet` keys;
/// clippy's `mutable_key_type` lint doesn't know that and may need allowing.
///
/// Each field has its own pointer, so `f` passed to `update_x` may safely call
/// accessors of *other* fields. Calling accessors of `x` itself from inside
/// `update_x` deadlocks with the locking strategies.
///
/// # Options
///
/// On the struct, `#[class(...)]` takes comma separated options:
///
/// * `strategy = SomeStrategy`: the `Strategy` used for every field.
/// * `crate = some::path`: the path to `galvanize_rs`, if it has been renamed
///   or re-exported.
///
/// On a field, `#[class(...)]` takes:
///
/// * `readonly`: makes `set_x` and `update_x` private. The field still can't
///   be reassigned from outside the module, but a handle-typed field (such as a
///   `List`) can still be modified through its getter.
/// * `strategy = SomeStrategy`: overrides the class's strategy for this field.
///
/// ```
/// use galvanize_rs::{ArcMutexStrategy, ArcSwapStrategy, Str, class};
///
/// #[class(strategy = ArcSwapStrategy)]
/// pub struct Counter {
///   #[class(readonly)]
///   pub label: Str,
///   #[class(strategy = ArcMutexStrategy)]
///   pub count: i64,
/// }
///
/// impl Counter {
///   pub fn new(label: &str) -> Self {
///     Self::from_data(CounterData { label: label.into(), count: 0 })
///   }
/// }
///
/// let c = Counter::new("hits");
/// c.update_count(|n| *n += 1);
/// assert_eq!(c.count(), 1);
/// ```
///
/// # Attributes
///
/// Doc comments on the struct document the handle. Any other attributes on the
/// struct (such as `#[derive(Default)]`) are applied to `PersonData`, as are the
/// attributes on fields. Field doc comments also document the getter.
///
/// # Limitations
///
/// * Only structs with named fields are supported, and they can't be generic.
/// * Fields can't be named `from_data` or `ptr_eq`.
/// * Every field type must implement `Clone` and `Debug`.
/// * `Debug` doesn't detect cycles; printing an object graph that refers back
///   to itself recurses forever.
///
#[proc_macro_attribute]
pub fn class(attr: TokenStream, item: TokenStream) -> TokenStream {
  class::expand(attr.into(), item.into())
    .unwrap_or_else(syn::Error::into_compile_error)
    .into()
}
