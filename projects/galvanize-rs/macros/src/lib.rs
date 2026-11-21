//! Procedural macros for `galvanize-rs`.
//!
//! Don't depend on this crate directly. Use the re-exports from `galvanize-rs`, since the generated code
//! refers to items in that crate.

mod class;

use proc_macro::TokenStream;

/// Turns a struct into a class: a shared handle type with accessors, inheritance and virtual methods.
///
/// The struct you write describes the class's fields. Every `#[class]` must be followed, in the same
/// module, by exactly one [`macro@class_methods`] block, which may be empty. For a struct named `Person`
/// the macros replace it with:
///
/// * the handle `Person` (same name and visibility as your struct). Cloning it is cheap and gives another
///   reference to the *same* object, like an object reference in Python, Go, C# or Java. It is
///   `Send + Sync`.
/// * `PersonData`, a plain struct with your fields, which you pass to the private `Person::from_data`
///   constructor. This keeps your own constructor the only way to create the class from outside its
///   module.
///
/// Each field is stored in its own `GPointer`, chosen by a `PointerStrategy` (its `SyncPtr`) that
/// defaults to `DefaultPointerStrategy`.
///
/// ```
/// use galvanize_rs::{GList, GString, class, class_methods, sf};
///
/// #[class]
/// pub struct Person {
///   /// The full name.
///   pub name: GString,
///   cities: GList<GString>,
/// }
///
/// #[class_methods]
/// impl Person {
///   pub fn new(first_name: impl Into<GString>, last_name: impl Into<GString>) -> Self {
///     Self::from_data(PersonData {
///       name: sf!("{} {}", first_name.into(), last_name.into()),
///       cities: GList::new(),
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
/// # fn main() {
/// let person = Person::new("Ada", "Lovelace");
/// let same = person.clone(); // another reference to the same object
/// same.set_name("Grace Hopper");
/// same.visit("Melbourne");
///
/// assert_eq!(person.name(), "Grace Hopper");
/// assert_eq!(person.visited(), 1);
/// assert_eq!(person, same); // classes compare by identity
/// # }
/// ```
///
/// # Generated methods
///
/// For every field `x: T` the handle gets:
///
/// * `x(&self) -> T` returns a clone of the value. For handle types (`GString`, `GList`, `GDict`, other classes)
///   that's the *same* shared object, so `person.cities().push(..)` modifies `person`. Other types are
///   copied, so keep field types cheap to clone and avoid e.g. `Vec`.
/// * `set_x(&self, value: impl Into<T>)` replaces the value.
/// * `update_x(&self, f: impl FnMut(&mut T) -> R) -> R` modifies the value in place, atomically with
///   respect to other accessors of `x`. `f` may be called more than once (see
///   `GPointer::write_with`).
///
/// The getter has the field's visibility, and so do the setter and updater unless the field is marked
/// `#[class_prop(readonly)]`. Each field has its own pointer, so `f` passed to `update_x` may safely call
/// accessors of *other* fields. Calling accessors of `x` itself from inside `update_x` deadlocks with the
/// locking pointer strategies.
///
/// The handle also gets:
///
/// * `from_data` (private), see above, unless the class is `abstract`.
/// * `new()` and `Default`, unless the class declares a constructor, see
///   [Default constructors](#default-constructors).
/// * `ptr_eq(&self, other)`, which is `true` when both handles refer to the same object, even when they
///   view it as different classes.
/// * `upcast`, `is` and `downcast`, see [Inheritance](#inheritance).
/// * `Clone`, `PartialEq`, `Eq` and `Hash` (all by identity), `Debug` (prints the field values) and
///   `From<&Person>` (clones the handle). As `Hash` and `Eq` ignore the field values, classes are sound
///   `HashMap`/`HashSet` keys; clippy's `mutable_key_type` lint doesn't know that and may need allowing.
///
/// # Options
///
/// On the struct, `#[class(...)]` takes comma separated options:
///
/// * `open`: allows other classes to extend this one. Without it a class is final.
/// * `abstract`: makes the class abstract, see [Abstract classes](#abstract-classes). Implies `open`.
/// * `extends = Parent`: makes this class a subclass of `Parent`, see [Inheritance](#inheritance).
/// * `implements = Trait` or `implements = (TraitA, TraitB)`: implements traits with the class's virtual
///   methods, see [Interfaces](#interfaces).
/// * `runtime_strategy = SafeRuntimeStrategy | UnsafeRuntimeStrategy`: how the hierarchy is implemented,
///   see [Runtime strategies](#runtime-strategies). Subclasses inherit their parent's.
/// * `pointer_strategy = SomePointerStrategy`: the `PointerStrategy` used for every field.
/// * `crate = some::path`: the path to `galvanize_rs`, if it has been renamed or re-exported.
///
/// On a field, `#[class_prop(...)]` takes comma separated options:
///
/// * `readonly`: makes `set_x` and `update_x` private. The field still can't be reassigned from outside
///   the module, but a handle-typed field (such as a `GList`) can still be modified through its getter.
/// * `pointer_strategy = SomePointerStrategy`: overrides the class's pointer strategy for this field.
///
/// `class_prop` is read and removed by `#[class]`, so it doesn't need importing.
///
/// ```
/// use galvanize_rs::{ArcMutexStrategy, ArcSwapStrategy, GString, class, class_methods};
///
/// #[class(pointer_strategy = ArcSwapStrategy)]
/// pub struct Counter {
///   #[class_prop(readonly)]
///   pub label: GString,
///   #[class_prop(pointer_strategy = ArcMutexStrategy)]
///   pub count: i64,
/// }
///
/// #[class_methods]
/// impl Counter {
///   pub fn new(label: &str) -> Self {
///     Self::from_data(CounterData { label: label.into(), count: 0 })
///   }
/// }
///
/// # fn main() {
/// let c = Counter::new("hits");
/// c.update_count(|n| *n += 1);
/// assert_eq!(c.count(), 1);
/// # }
/// ```
///
/// # Inheritance
///
/// A class declared `#[class(open)]` can be extended with `#[class(extends = Parent)]`. Only the
/// immediate parent is named. A subclass is final unless it's also `open`.
///
/// * The object is a single object: a `Dog` *is* an `Animal`, with one identity, not an `Animal` wrapped
///   in a `Dog`.
/// * `DogData` gets a `base: AnimalData` field, which the constructor usually fills using a function of
///   the parent, like calling `super().__init__()`.
/// * The handle dereferences to the parent, so the parent's accessors and methods can be called on it,
///   and `&Dog` coerces to `&Animal`. This chains through every ancestor.
/// * `upcast` returns a handle viewing the object as the class itself or any ancestor, e.g.
///   `let animal: Animal = puppy.upcast();`, and so do `From<Dog>` and `From<&Dog>` for each ancestor.
///   To accept any subclass of `Animal` by value, take an `impl Extends<Animal, I>`.
/// * `is::<T>()` tests whether the object is a `T` or a subclass of `T`, and `downcast::<T>()` returns a
///   handle viewing it as a `T` if it is one.
/// * `Debug` prints the fields of the ancestors first, then the class's own, of the class the handle views
///   the object as.
///
/// Mark methods in [`macro@class_methods`] with `#[class_virtual]` to make them overridable, and with
/// `#[class_override]` to override an inherited one, with the same signature and visibility. An override
/// `x` also gets a `super_x` method, which calls the parent's implementation. Calls are late-bound, so a
/// virtual call made through any view, or from inside a parent's method, runs the most-derived override.
///
/// As in Scala, a method named after an inherited virtual method can't shadow it: without
/// `#[class_override]` it's an error, unless it implements an abstract method (see
/// [Abstract classes](#abstract-classes)). So adding a virtual method to a class can't silently change
/// what a subclass's method of the same name does.
///
/// ```
/// use galvanize_rs::{GString, class, class_methods, sf};
///
/// #[class(open)]
/// pub struct Animal {
///   pub name: GString,
/// }
///
/// #[class_methods]
/// impl Animal {
///   pub fn init(name: &str) -> AnimalData {
///     AnimalData { name: name.into() }
///   }
///
///   #[class_virtual]
///   pub fn speak(&self) -> GString {
///     "...".into()
///   }
///
///   pub fn describe(&self) -> GString {
///     sf!("{} says {}", self.name(), self.speak())
///   }
/// }
///
/// #[class(extends = Animal)]
/// pub struct Dog {
///   pub good: bool,
/// }
///
/// #[class_methods]
/// impl Dog {
///   pub fn new(name: &str) -> Self {
///     Self::from_data(DogData { base: Animal::init(name), good: true })
///   }
///
///   #[class_override]
///   pub fn speak(&self) -> GString {
///     sf!("Woof (not {})", self.super_speak())
///   }
/// }
///
/// # fn main() {
/// let dog = Dog::new("Rex");
/// dog.set_name("Max"); // inherited from `Animal`
///
/// let animal: Animal = dog.upcast();
/// assert_eq!(animal.describe(), "Max says Woof (not ...)"); // late-bound
/// assert!(animal.ptr_eq(&dog));
/// assert!(animal.is::<Dog>());
/// assert!(animal.downcast::<Dog>().unwrap().good());
/// assert_eq!(format!("{dog:?}"), r#"Dog { name: "Max", good: true }"#);
/// # }
/// ```
///
/// Virtual methods may be `async`, returning `Send` futures, and overrides must match. Each call to an
/// async virtual method boxes its future.
///
/// # Abstract classes
///
/// A class declared `#[class(abstract)]` can't be instantiated, as it doesn't get `from_data` or `new()`,
/// but its `XData` still initializes subclasses. It may declare abstract methods, virtual methods without a
/// body, with `#[class_abstract]`. A class that isn't abstract must implement every abstract method it
/// inherits that no ancestor implements, which is checked at compile time. Implementing one doesn't need
/// `#[class_override]`, as there's no implementation to replace (and a misspelt one leaves the abstract
/// method unimplemented), nor does it get a `super_x`, as there's no implementation to call.
///
/// # Default constructors
///
/// Like in Java or C#, a class that doesn't declare a constructor gets a default one: `new()`, with the
/// class's visibility, which sets every field, including those of its ancestors, to `Default::default()`.
/// The class also implements `Default`, calling `new()`.
///
/// A constructor is a method named `new`, or a function without a receiver whose return type mentions
/// `Self` or the class, such as `fn load() -> Result<Self, Error>`. Functions returning the class's `XData`,
/// like the `init` functions above, aren't constructors. Abstract classes, and classes with a field named
/// `new`, don't get a default constructor. Declare constructors in the `#[class_methods]` block, as one in
/// another `impl` block isn't seen and would clash with `new()`, as would implementing `Default` yourself.
///
/// The class still compiles when a field, or an ancestor's, doesn't implement `Default`, but calling
/// `new()` or `default()` is then an error, like with `#[derive(Default)]` on a generic struct.
///
/// ```
/// use galvanize_rs::{class, class_methods};
///
/// pub trait Locate {
///   fn locate(&self, dir: &str) -> String;
/// }
///
/// #[class(abstract, implements = Locate)]
/// pub struct ConfigFile {
///   pub file_name: String,
/// }
///
/// #[class_methods]
/// impl ConfigFile {
///   #[class_abstract]
///   fn locate(&self, dir: &str) -> String;
/// }
///
/// #[class(extends = ConfigFile)]
/// pub struct BotConfigFile {}
///
/// #[class_methods]
/// impl BotConfigFile {
///   fn locate(&self, dir: &str) -> String {
///     format!("{dir}/{}", self.file_name())
///   }
/// }
///
/// # fn main() {
/// let bot = BotConfigFile::new();
/// assert_eq!(bot.file_name(), "");
/// bot.set_file_name("bot.yaml");
/// assert_eq!(bot.upcast::<ConfigFile, _>().locate("/etc"), "/etc/bot.yaml");
/// assert!(!BotConfigFile::default().ptr_eq(&bot));
/// # }
/// ```
///
/// # Interfaces
///
/// `implements = Trait` implements `Trait` for the class, so a trait can be an interface the class
/// fulfils. Declare the trait with [`macro@class_interface`] and the class's methods named after the
/// trait's methods implement it, whatever their kind or visibility, so ordinary methods can implement it
/// too, and private methods the trait doesn't declare stay out of it.
///
/// Otherwise, as a proc macro can't see the trait's methods, the class's virtual and abstract methods
/// declared *without a visibility*, like the methods of a trait impl, are the ones that implement it. With
/// several such traits, each of those names its trait: `#[class_virtual(TraitA)]`. Use `pub(self)` for a
/// private virtual method that isn't part of the trait. Either way, the trait's methods that the class
/// doesn't declare keep their defaults, and rustc checks the impl as usual.
///
/// Each method is still a method of the class too, callable in its module, and its doc comment documents
/// the trait impl, which otherwise shows the trait's docs.
///
/// Subclasses don't implement the trait themselves, as the macro can't see its signatures. Method calls
/// such as `square.area()` still reach it through the ancestor that does, and `&*square` or
/// `square.upcast()` gives that ancestor's handle for code expecting an `impl Trait` or `dyn Trait`.
/// Overrides of trait methods have no visibility either, so their wrapper on the subclass is private.
///
/// ```
/// use galvanize_rs::{class, class_methods};
///
/// pub trait Shape {
///   /// The area of the shape.
///   fn area(&self) -> f64;
///
///   fn describe(&self) -> String {
///     format!("area {}", self.area())
///   }
/// }
///
/// #[class(abstract, implements = Shape)]
/// pub struct Polygon {
///   pub label: String,
/// }
///
/// #[class_methods]
/// impl Polygon {
///   #[class_abstract]
///   fn area(&self) -> f64;
/// }
///
/// #[class(extends = Polygon)]
/// pub struct Square {
///   pub side: f64,
/// }
///
/// #[class_methods]
/// impl Square {
///   pub fn new(side: f64) -> Self {
///     Self::from_data(SquareData { base: PolygonData { label: "square".into() }, side })
///   }
///
///   /// The side, squared.
///   fn area(&self) -> f64 {
///     self.side() * self.side()
///   }
/// }
///
/// # fn main() {
/// let square = Square::new(2.0);
/// let shapes: Vec<Box<dyn Shape>> = vec![Box::new(square.upcast::<Polygon, _>())];
/// assert_eq!(shapes[0].describe(), "area 4");
/// assert_eq!(square.area(), 4.0);
/// # }
/// ```
///
/// # Class-level members
///
/// Associated functions (without `self`) and constants can be virtual, abstract or overrides too, with
/// the same markers. They're late-bound like Python's `cls` or PHP's `static::`: in their bodies, `Self` is
/// the class they're accessed through, so an inherited function sees its subclass's constants. Every
/// subclass gets them as its own associated items, so `Sub::locate()` and `Sub::NAMES` just work, and
/// `Self::super_x()` calls the parent's implementation in an override. Ordinary constants are allowed too,
/// and are neither inherited nor late-bound.
///
/// ```
/// use galvanize_rs::{class, class_methods};
///
/// #[class(abstract)]
/// pub struct ConfigFile {}
///
/// #[class_methods]
/// impl ConfigFile {
///   #[class_abstract]
///   pub const NAMES: &'static [&'static str];
///
///   #[class_virtual]
///   pub fn locate(dir: &str) -> String {
///     format!("{dir}/{}", Self::NAMES[0])
///   }
/// }
///
/// #[class(open, extends = ConfigFile)]
/// pub struct BotConfigFile {}
///
/// #[class_methods]
/// impl BotConfigFile {
///   pub const NAMES: &'static [&'static str] = &["bot.yaml"]; // implements the abstract constant
/// }
///
/// #[class(extends = BotConfigFile)]
/// pub struct TestConfigFile {}
///
/// #[class_methods]
/// impl TestConfigFile {
///   #[class_override]
///   pub const NAMES: &'static [&'static str] = &["test.yaml"];
///
///   #[class_override]
///   pub fn locate(dir: &str) -> String {
///     format!("{} (test)", Self::super_locate(dir))
///   }
/// }
///
/// # fn main() {
/// assert_eq!(BotConfigFile::locate("/etc"), "/etc/bot.yaml");
/// assert_eq!(TestConfigFile::locate("/tmp"), "/tmp/test.yaml (test)");
/// # }
/// ```
///
/// Rust only binds `Self` late in a trait's default methods, so these bodies are generic over the class:
/// through `Self`, they can only reach other class-level members, so name the class to call its ordinary
/// associated functions. A final class's overrides are the exception, as nothing can inherit them, so
/// `Self` is the class itself in them. A class only gets its class-level members once none is abstract.
///
/// A subclass repeats the signatures of the class-level members it inherits, so the types they name must
/// resolve where it's declared too (paths starting with `crate::` are adjusted to keep naming the parent's
/// crate). Class-level functions may be async and generic, and use `Self` and `impl Trait`, but can't be
/// `const`, `unsafe` or `extern`.
///
/// With [`macro@class_interface`], an interface may declare class-level members too. An open class whose
/// class-level members implement one implements it for every class that inherits them, so subclasses
/// satisfy `T: Trait`:
///
/// ```
/// use galvanize_rs::{class, class_interface, class_methods};
///
/// #[class_interface]
/// pub trait Named {
///   const NAME: &'static str;
///
///   fn greet() -> String;
/// }
///
/// #[class(abstract, implements = Named)]
/// pub struct Base {}
///
/// #[class_methods]
/// impl Base {
///   #[class_abstract]
///   pub const NAME: &'static str;
///
///   #[class_virtual]
///   pub fn greet() -> String {
///     format!("hello {}", Self::NAME)
///   }
/// }
///
/// #[class(extends = Base)]
/// pub struct World {}
///
/// #[class_methods]
/// impl World {
///   pub const NAME: &'static str = "world";
/// }
///
/// fn greet<T: Named>() -> String {
///   T::greet()
/// }
///
/// # fn main() {
/// assert_eq!(greet::<World>(), "hello world");
/// # }
/// ```
///
/// # Runtime strategies
///
/// Both strategies generate the same API from the same declarations, so a hierarchy can switch between
/// them by changing its root's `runtime_strategy` alone:
///
/// * `SafeRuntimeStrategy` (the default) generates no `unsafe` code. Handles grow by a word or two per
///   level of the hierarchy, and every call to an override clones and drops an `Arc`, which contends when
///   many threads call methods on the same object.
/// * `UnsafeRuntimeStrategy` uses a small `unsafe` kernel with C++-style layouts and vtables, which is
///   tested under Miri. Handles are four words at any depth, and virtual calls cost about a nanosecond
///   without writing to shared state.
///
/// # Generics
///
/// Classes may have type parameters, but not lifetime or const parameters. Type parameters used by
/// fields or the parent are bounded with `Clone + Send + Sync + 'static`, and those only used by a
/// pointer strategy with `'static`, so you write `Node<T>` rather than `Node<T: Clone + ..>`. `Debug` is
/// only implemented when the field type parameters are `Debug`. A subclass must give every one of its
/// parent's type parameters, e.g. `extends = Boxed<T>`.
///
/// Other `impl` blocks of a generic class must repeat the bounds the macro adds.
///
/// `Self` in a field type refers to the class (here `Node<T>`). A parameter that only appears in a
/// pointer strategy (such as `S` below) isn't in `NodeData`.
///
/// ```
/// use galvanize_rs::{DefaultPointerStrategy, PointerStrategy, class, class_methods};
///
/// #[class(pointer_strategy = S)]
/// pub struct Node<T, S: PointerStrategy = DefaultPointerStrategy> {
///   pub value: T,
///   pub next: Option<Self>,
/// }
///
/// #[class_methods]
/// impl<T, S> Node<T, S> {
///   pub fn new(value: T, next: Option<Self>) -> Self {
///     Self::from_data(NodeData { value, next })
///   }
/// }
///
/// # fn main() {
/// let list: Node<i64> = Node::new(1, Some(Node::new(2, None)));
/// list.next().unwrap().set_value(3);
/// assert_eq!(format!("{list:?}"), "Node { value: 1, next: Some(Node { value: 3, next: None }) }");
/// # }
/// ```
///
/// # Attributes
///
/// Doc comments on the struct document the handle. Any other attributes on the struct (such as
/// `#[derive(Default)]`) are applied to `PersonData`, as are the attributes on fields. Field doc comments
/// also document the getter.
///
/// # Limitations
///
/// * An `open` class must be declared at the root of its crate, as its descriptor (which subclasses read
///   at compile time) is exported from there. Final classes can be declared anywhere.
/// * Only structs with named fields are supported, and every field type must implement `Clone`, `Send`,
///   `Sync` and `Debug`, and be `'static`.
/// * Every type parameter must be used by a field type, pointer strategy or the parent; using it only
///   through `Self` doesn't count. Bounds such as `S: PointerStrategy` aren't inferred, write them on the
///   struct.
/// * Fields can't be named `base`, `from_data`, `ptr_eq`, `upcast`, `is` or `downcast`, or start with
///   `__`.
/// * There's no equivalent of `protected`; private items are only accessible in their own module, as
///   usual.
/// * `Debug` doesn't detect cycles; printing an object graph that refers back to itself recurses forever.
///   Objects that refer to each other strongly are never freed, as with any reference counting.
#[proc_macro_attribute]
pub fn class(attr: TokenStream, item: TokenStream) -> TokenStream {
  class::class(attr.into(), item.into())
    .unwrap_or_else(syn::Error::into_compile_error)
    .into()
}

/// Supplies the methods of the [`macro@class`] declared just before it, in the same module.
///
/// Every `#[class]` needs exactly one `#[class_methods]` block, even an empty one (`impl Name {}`). It must
/// be an inherent impl of the class with the struct's type parameters and no bounds; the struct's bounds
/// apply to it. Other, ordinary `impl` blocks can be added as usual.
///
/// Within it, mark methods, associated functions and constants with:
///
/// * `#[class_virtual]` to declare a virtual method, which subclasses may override.
/// * `#[class_abstract]` to declare an abstract method, a virtual method without a body
///   (`fn x(&self) -> R;`), in an abstract class.
/// * `#[class_virtual(Trait)]` or `#[class_abstract(Trait)]` to name the trait a method implements, when
///   the class `implements` several.
///
/// * `#[class_override]` to override a virtual method declared by any ancestor. Its signature, including
///   whether it's `async`, and its visibility must match. It gets a `super_x` method calling the parent's
///   implementation.
///
/// The same markers on associated functions without `self` and on constants (`const X: T;` when abstract)
/// make them late-bound class-level members, see [Class-level members](macro@class#class-level-members).
/// Their overrides call the parent's implementation with `Self::super_x`.
///
/// A method implementing an abstract method that no ancestor implements is an override without the
/// marker, and gets no `super_x`. Any other method named after an inherited virtual method is an error.
///
/// Virtual methods and overrides must take `&self`, and not be generic or use `Self` or `impl Trait` in
/// their signature. Their parameters must be plain identifiers.
///
/// See [`macro@class`] for an example.
#[proc_macro_attribute]
pub fn class_methods(attr: TokenStream, item: TokenStream) -> TokenStream {
  class::class_methods(attr.into(), item.into())
    .unwrap_or_else(syn::Error::into_compile_error)
    .into()
}

/// Lets classes implement a trait by matching their methods to the trait's by name.
///
/// A proc macro can't see a trait's methods, so `implements = Trait` normally treats a class's virtual
/// and abstract methods declared without a visibility as the trait's (see [Interfaces](macro@class#interfaces)).
/// With `#[class_interface]` on the trait, the methods named after the trait's methods implement it
/// instead, whether they're ordinary, virtual or abstract, and whatever their visibility. The class's
/// other methods are just methods of the class. Constants and class-level members are matched by name too,
/// see [Class-level members](macro@class#class-level-members).
///
/// ```
/// use galvanize_rs::{class, class_interface, class_methods};
///
/// #[class_interface]
/// pub trait Locate {
///   fn locate(&self, dir: &str) -> Option<String>;
/// }
///
/// #[class(abstract, implements = Locate)]
/// pub struct ConfigFile {}
///
/// #[class_methods]
/// impl ConfigFile {
///   /// Not part of `Locate`, as it doesn't declare it.
///   #[class_abstract]
///   fn file_name(&self) -> String;
///
///   /// An ordinary method implementing `Locate::locate`.
///   fn locate(&self, dir: &str) -> Option<String> {
///     Some(format!("{dir}/{}", self.file_name()))
///   }
/// }
///
/// #[class(extends = ConfigFile)]
/// pub struct BotConfigFile {}
///
/// #[class_methods]
/// impl BotConfigFile {
///   fn file_name(&self) -> String {
///     "bot.yaml".into()
///   }
/// }
///
/// # fn main() {
/// let file: Box<dyn Locate> = Box::new(BotConfigFile::new().upcast::<ConfigFile, _>());
/// assert_eq!(file.locate("/etc").as_deref(), Some("/etc/bot.yaml"));
/// # }
/// ```
///
/// The trait itself is unchanged. Its method names are recorded in a hidden macro, which shares the
/// trait's name and so is imported along with it. It's visible within the trait's crate only, so classes
/// in other crates, like those implementing traits without `#[class_interface]`, match the trait's
/// methods by visibility. So do classes implementing a trait named like one of std's derive macros, such
/// as `Ord`, as they'd find the derive macro instead.
#[proc_macro_attribute]
pub fn class_interface(attr: TokenStream, item: TokenStream) -> TokenStream {
  class::class_interface(attr.into(), item.into())
    .unwrap_or_else(syn::Error::into_compile_error)
    .into()
}

#[doc(hidden)]
#[proc_macro]
pub fn __implement(input: TokenStream) -> TokenStream {
  class::implement(input.into())
    .unwrap_or_else(syn::Error::into_compile_error)
    .into()
}

#[doc(hidden)]
#[proc_macro]
pub fn __no_interface(input: TokenStream) -> TokenStream {
  class::no_interface(input.into())
    .unwrap_or_else(syn::Error::into_compile_error)
    .into()
}

#[doc(hidden)]
#[proc_macro]
pub fn __complete(input: TokenStream) -> TokenStream {
  class::complete(input.into())
    .unwrap_or_else(syn::Error::into_compile_error)
    .into()
}

#[doc(hidden)]
#[proc_macro]
pub fn __inherit(input: TokenStream) -> TokenStream {
  class::inherit(input.into())
    .unwrap_or_else(syn::Error::into_compile_error)
    .into()
}
