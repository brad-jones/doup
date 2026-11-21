// The class hierarchies shared by galvanize-rs's tests and benchmarks. Written for
// `SafeRuntimeStrategy`; the unsafe fixtures' build script rewrites it for `UnsafeRuntimeStrategy`.

use galvanize_rs::{SafeRuntimeStrategy, class, class_interface, class_methods};

/// Root of the benchmark hierarchy.
#[class(open, runtime_strategy = SafeRuntimeStrategy)]
pub struct Animal {
  pub name: String,
  pub age: u64,
}

#[class_methods]
impl Animal {
  pub fn new(name: &str) -> Self {
    Self::from_data(Self::init(name))
  }

  pub fn init(name: &str) -> AnimalData {
    AnimalData {
      name: name.into(),
      age: 3,
    }
  }

  #[class_virtual]
  pub fn speak(&self) -> String {
    "...".into()
  }

  #[class_virtual]
  pub fn legs(&self) -> u64 {
    4
  }

  #[class_virtual]
  pub fn weight(&self) -> u64 {
    10
  }

  pub fn describe(&self) -> String {
    format!("{} says {}", self.name(), self.speak())
  }
}

/// An open subclass; only its immediate parent is named.
#[class(open, extends = Animal)]
pub struct Dog {
  pub tricks: usize,
}

#[class_methods]
impl Dog {
  pub fn new(name: &str) -> Self {
    Self::from_data(Self::init(name))
  }

  pub fn init(name: &str) -> DogData {
    DogData {
      base: Animal::init(name),
      tricks: 0,
    }
  }

  #[class_override]
  pub fn speak(&self) -> String {
    "Woof".into()
  }

  #[class_override]
  pub fn weight(&self) -> u64 {
    30
  }

  #[class_virtual]
  pub fn bark_volume(&self) -> u64 {
    8
  }
}

/// A final subclass overriding methods declared at two different levels.
#[class(extends = Dog)]
pub struct Puppy {}

#[class_methods]
impl Puppy {
  pub fn new(name: &str) -> Self {
    Self::from_data(PuppyData { base: Dog::init(name) })
  }

  #[class_override]
  pub fn speak(&self) -> String {
    format!("{} (but squeakier)", self.super_speak())
  }

  #[class_override]
  pub fn weight(&self) -> u64 {
    self.super_weight() / 4
  }

  #[class_override]
  pub fn bark_volume(&self) -> u64 {
    self.super_bark_volume() / 2
  }
}

/// A sibling that inherits its parent's implementations unchanged.
#[class(extends = Dog)]
pub struct Beagle {}

#[class_methods]
impl Beagle {
  pub fn new(name: &str) -> Self {
    Self::from_data(BeagleData { base: Dog::init(name) })
  }
}

/// Generic root; the macro bounds `T` with `Clone + Send + Sync + 'static`.
#[class(open, runtime_strategy = SafeRuntimeStrategy)]
pub struct Boxed<T> {
  pub value: T,
}

#[class_methods]
impl<T> Boxed<T> {
  pub fn new(value: T) -> Self {
    Self::from_data(BoxedData { value })
  }

  #[class_virtual]
  pub fn get(&self) -> T {
    self.value()
  }
}

/// Generic final subclass with a lexical super call.
#[class(extends = Boxed<T>)]
pub struct Tagged<T> {
  pub tag: u64,
}

#[class_methods]
impl<T> Tagged<T> {
  pub fn new(value: T) -> Self {
    Self::from_data(TaggedData {
      base: BoxedData { value },
      tag: 0,
    })
  }

  #[class_override]
  pub fn get(&self) -> T {
    self.super_get()
  }
}

/// An interface, implemented by a class through `implements`.
pub trait Shape: Send + Sync {
  /// The shape's area.
  fn area(&self) -> f64;

  fn corners(&self) -> u64;

  /// A default method, calling the class's virtual methods.
  fn summary(&self) -> String {
    format!("{} corners, area {}", self.corners(), self.area())
  }
}

/// A second interface, so methods name the trait they implement.
pub trait Named: Send + Sync {
  fn title(&self) -> String;
}

/// An abstract root implementing two interfaces. `abstract` implies `open`.
#[class(abstract, implements = (Shape, Named), runtime_strategy = SafeRuntimeStrategy)]
pub struct Polygon {
  pub label: String,
}

#[class_methods]
impl Polygon {
  pub fn init(label: &str) -> PolygonData {
    PolygonData { label: label.into() }
  }

  /// Methods implementing a trait don't take a visibility, as in a trait impl.
  #[class_abstract(Shape)]
  fn area(&self) -> f64;

  #[class_virtual(Shape)]
  fn corners(&self) -> u64 {
    0
  }

  #[class_virtual(Named)]
  fn title(&self) -> String {
    format!("polygon {}", self.label())
  }

  /// An abstract method that isn't part of an interface.
  #[class_abstract]
  pub fn scaled(&self, factor: f64) -> f64;

  /// An ordinary method calling abstract ones.
  pub fn describe(&self) -> String {
    format!("{}: {}", self.title(), self.scaled(1.0))
  }
}

/// An abstract subclass implementing some of its parent's abstract methods.
#[class(abstract, extends = Polygon)]
pub struct Rectangle {
  pub width: f64,
  pub height: f64,
}

#[class_methods]
impl Rectangle {
  pub fn init(label: &str, width: f64, height: f64) -> RectangleData {
    RectangleData {
      base: Polygon::init(label),
      width,
      height,
    }
  }

  /// Implements `Polygon`'s abstract method, without `#[class_override]`.
  fn area(&self) -> f64 {
    self.width() * self.height()
  }

  #[class_override]
  fn corners(&self) -> u64 {
    self.super_corners() + 4
  }
}

/// An interface implemented by an abstract class, like doup's config files.
pub trait Locate: Send + Sync {
  /// Where the file is, if `dir` is given.
  fn locate(&self, dir: Option<&str>) -> Option<String>;
}

/// An abstract root whose subclasses declare no constructor, so they get `new()`, which defaults the
/// fields they inherit from it too.
#[class(abstract, implements = Locate, runtime_strategy = SafeRuntimeStrategy)]
pub struct ConfigFile {
  pub file_name: String,
  pub loaded: bool,
}

#[class_methods]
impl ConfigFile {
  #[class_abstract]
  fn locate(&self, dir: Option<&str>) -> Option<String>;
}

/// A concrete subclass without a constructor, which other crates create with its generated `new()`.
#[class(extends = ConfigFile)]
pub struct RepoConfigFile {}

#[class_methods]
impl RepoConfigFile {
  fn locate(&self, dir: Option<&str>) -> Option<String> {
    dir.map(|dir| format!("{dir}/.repo/{}", self.file_name()))
  }
}

/// Returns `Pending` once before completing, so async tests exercise real suspension.
pub async fn suspend_once() {
  let mut suspended = false;
  std::future::poll_fn(|cx| {
    if suspended {
      std::task::Poll::Ready(())
    } else {
      suspended = true;
      cx.waker().wake_by_ref();
      std::task::Poll::Pending
    }
  })
  .await;
}

/// Async root used by the cross-crate tests, separate from the synchronous benchmark fixtures.
#[class(open, runtime_strategy = SafeRuntimeStrategy)]
pub struct AsyncAnimal {
  pub name: String,
}

#[class_methods]
impl AsyncAnimal {
  pub fn new(name: &str) -> Self {
    Self::from_data(Self::init(name))
  }

  pub fn init(name: &str) -> AsyncAnimalData {
    AsyncAnimalData { name: name.into() }
  }

  #[class_virtual]
  pub fn sound(&self) -> String {
    "...".into()
  }

  #[class_virtual]
  pub async fn speak(&self, prefix: &str) -> Result<String, &'static str> {
    suspend_once().await;
    if prefix == "fail" {
      return Err("requested failure");
    }
    Ok(format!("{prefix}{}:{}", self.name(), self.sound()))
  }

  #[class_virtual]
  pub async fn append(&self, prefix: Option<&str>, output: &mut String, convert: fn(&str) -> String) {
    suspend_once().await;
    if let Some(prefix) = prefix {
      output.push_str(&convert(prefix));
    }
    output.push_str(&self.sound());
  }

  pub async fn describe(&self, prefix: &str) -> Result<String, &'static str> {
    Ok(format!("[{}]", self.speak(prefix).await?))
  }
}

/// Async override with a real suspension and an awaited lexical super call.
#[class(open, extends = AsyncAnimal)]
pub struct AsyncDog {
  pub adjective: String,
}

#[class_methods]
impl AsyncDog {
  pub fn init(name: &str) -> AsyncDogData {
    AsyncDogData {
      base: AsyncAnimal::init(name),
      adjective: "dog".into(),
    }
  }

  #[class_override]
  pub fn sound(&self) -> String {
    "woof".into()
  }

  #[class_override]
  pub async fn speak(&self, prefix: &str) -> Result<String, &'static str> {
    let parent = self.super_speak(prefix).await?;
    suspend_once().await;
    Ok(format!("{parent}/{}", self.adjective()))
  }
}

/// Generic async root for typed return values and cancellation tests.
#[class(open, runtime_strategy = SafeRuntimeStrategy)]
pub struct AsyncBoxed<T> {
  pub value: T,
}

#[class_methods]
impl<T> AsyncBoxed<T> {
  #[class_virtual]
  pub async fn get(&self) -> T {
    suspend_once().await;
    self.value()
  }
}

/// An interface with an async method.
pub trait Lookup: Send + Sync {
  fn lookup(&self, key: &str) -> impl std::future::Future<Output = Result<String, &'static str>> + Send;
}

/// An abstract root implementing an interface with an async abstract method.
#[class(abstract, implements = Lookup, runtime_strategy = SafeRuntimeStrategy)]
pub struct AsyncSource {
  pub name: String,
}

#[class_methods]
impl AsyncSource {
  pub fn init(name: &str) -> AsyncSourceData {
    AsyncSourceData { name: name.into() }
  }

  #[class_abstract]
  async fn lookup(&self, key: &str) -> Result<String, &'static str>;

  pub async fn lookup_all(&self, keys: &[&str]) -> Result<Vec<String>, &'static str> {
    let mut values = Vec::new();
    for key in keys {
      values.push(self.lookup(key).await?);
    }
    Ok(values)
  }
}

/// The stem of a file name. Named through `crate::` by `Format`, which its subclasses in other crates repeat.
pub type Stem = str;

/// An interface with class-level members, implemented by every class that implements `Format`'s.
#[class_interface]
pub trait FileFormat {
  const EXTENSIONS: &'static [&'static str];

  fn file_name(stem: &Stem) -> String;

  fn preview(&self) -> String;
}

/// An abstract root with class-level members, which are late-bound: in their bodies `Self` is the class
/// they're called on.
#[class(abstract, implements = FileFormat, runtime_strategy = SafeRuntimeStrategy)]
pub struct Format {
  pub text: String,
}

#[class_methods]
impl Format {
  /// The format's file extensions.
  #[class_abstract]
  pub const EXTENSIONS: &'static [&'static str];

  /// A virtual constant, computed from an abstract one.
  #[class_virtual]
  pub const PRIMARY: &'static str = Self::EXTENSIONS[0];

  /// The file name for `stem`, with the extension of the class it's called on.
  #[class_virtual]
  pub fn file_name(stem: &crate::Stem) -> String {
    format!("{stem}.{}", Self::PRIMARY)
  }

  #[class_virtual]
  pub fn label() -> String {
    "format".into()
  }

  #[class_virtual]
  pub async fn fetch(stem: &str) -> String {
    suspend_once().await;
    format!("fetched {}", Self::file_name(stem))
  }

  /// An ordinary method, implementing `FileFormat::preview` for every subclass.
  fn preview(&self) -> String {
    format!("{}: {}", self.label_of(), self.text())
  }

  #[class_virtual]
  pub fn label_of(&self) -> String {
    "format".into()
  }
}

/// An open subclass implementing the abstract constant, overriding a function and declaring another.
#[class(open, extends = Format)]
pub struct Yaml {}

#[class_methods]
impl Yaml {
  /// Implements `Format`'s abstract constant, without `#[class_override]`.
  pub const EXTENSIONS: &'static [&'static str] = &["yaml", "yml"];

  /// Shared with subclasses, which run it with `Self` as themselves.
  #[class_override]
  pub fn label() -> String {
    format!("yaml {}", Self::super_label())
  }

  #[class_virtual]
  pub fn indent() -> usize {
    2
  }
}

/// A final subclass, whose overrides use `Self` as the concrete class, so can call its ordinary members.
#[class(extends = Format)]
pub struct Json {}

#[class_methods]
impl Json {
  /// An ordinary constant, neither late-bound nor inherited.
  pub const MIME: &'static str = "application/json";

  pub const EXTENSIONS: &'static [&'static str] = &["json"];

  #[class_override]
  pub const PRIMARY: &'static str = "JSON";

  #[class_override]
  pub fn file_name(stem: &crate::Stem) -> String {
    Self::shout(&Self::super_file_name(stem))
  }

  fn shout(text: &str) -> String {
    text.to_uppercase()
  }
}

/// A generic open class with class-level members using its type parameter.
#[class(open, runtime_strategy = SafeRuntimeStrategy)]
pub struct Registry<T> {
  pub items: Vec<T>,
}

#[class_methods]
impl<T> Registry<T> {
  #[class_virtual]
  pub fn capacity() -> usize {
    4
  }

  #[class_virtual]
  pub fn wrap(item: T) -> Vec<T> {
    let mut items = Vec::with_capacity(Self::capacity());
    items.push(item);
    items
  }
}
