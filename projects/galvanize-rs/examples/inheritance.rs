//! Class inheritance: virtual methods, overrides, `super` calls, upcasts and downcasts.
//!
//! Run with `cargo run -p galvanize-rs --example inheritance`.

use galvanize_rs::{Extends, List, Str, UnsafeRuntimeStrategy, class, class_methods, sf};

// ---------- Shape ----------

/// Something drawn on a canvas.
///
/// `open` lets other classes extend it. The runtime strategy is chosen by the root of a hierarchy and
/// inherited by its subclasses. Leave it out for the default, `SafeRuntimeStrategy`; both generate the
/// same API, so this example behaves the same either way.
#[class(open, runtime_strategy = UnsafeRuntimeStrategy)]
pub struct Shape {
  pub name: Str,
  pub x: f64,
  pub y: f64,
}

#[class_methods]
impl Shape {
  /// The initial values of a `Shape`, for subclasses to build on, like calling `super().__init__()`.
  pub fn init(name: &str, x: f64, y: f64) -> ShapeData {
    ShapeData {
      name: name.into(),
      x,
      y,
    }
  }

  pub fn new(name: &str, x: f64, y: f64) -> Self {
    Self::from_data(Self::init(name, x, y))
  }

  pub fn move_by(&self, dx: f64, dy: f64) {
    self.update_x(|x| *x += dx);
    self.update_y(|y| *y += dy);
  }

  /// Virtual: subclasses may override it.
  #[class_virtual]
  pub fn area(&self) -> f64 {
    0.0
  }

  #[class_virtual]
  pub fn describe(&self) -> Str {
    sf!("{} at ({}, {})", self.name(), self.x(), self.y())
  }

  /// Virtual methods can be async too. Their futures are `Send`, so they work with any executor.
  #[class_virtual]
  pub async fn render(&self, canvas: &str) -> Str {
    sf!("drew {} on {canvas}", self.name())
  }

  /// Not virtual, but the calls it makes are, so it does the right thing for every subclass.
  pub fn summary(&self) -> Str {
    sf!("{}, with an area of {}", self.describe(), self.area())
  }
}

// ---------- Rect ----------

/// A rectangle is a shape with a size. It's `open` too, so it can be extended in turn.
#[class(open, extends = Shape)]
pub struct Rect {
  pub width: f64,
  pub height: f64,
}

#[class_methods]
impl Rect {
  pub fn init(name: &str, x: f64, y: f64, width: f64, height: f64) -> RectData {
    RectData {
      base: Shape::init(name, x, y),
      width,
      height,
    }
  }

  pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
    Self::from_data(Self::init("rect", x, y, width, height))
  }

  #[class_override]
  pub fn area(&self) -> f64 {
    self.width() * self.height()
  }

  /// Overrides get a `super_` method to call the parent's implementation.
  #[class_override]
  pub fn describe(&self) -> Str {
    sf!("{} sized {}x{}", self.super_describe(), self.width(), self.height())
  }

  #[class_override]
  pub async fn render(&self, canvas: &str) -> Str {
    sf!("{} with a border", self.super_render(canvas).await)
  }
}

// ---------- Square ----------

/// Inheritance chains: a square is a rect, which is a shape. Without `open`, it's final.
#[class(extends = Rect)]
pub struct Square {}

#[class_methods]
impl Square {
  pub fn new(x: f64, y: f64, size: f64) -> Self {
    Self::from_data(SquareData {
      base: Rect::init("square", x, y, size, size),
    })
  }

  /// Overrides a method declared two levels up. `Rect`'s `area` is inherited.
  #[class_override]
  pub fn describe(&self) -> Str {
    sf!("{} (a square)", self.super_describe())
  }
}

/// Takes any shape by reference: `&Rect` and `&Square` coerce to `&Shape`.
fn print_summary(shape: &Shape) {
  println!("{}", shape.summary());
}

/// Takes any shape by value, via `Extends`.
fn add_to_canvas<I>(canvas: &List<Shape>, shape: impl Extends<Shape, I>) {
  canvas.push(shape.upcast());
}

fn main() {
  let dot = Shape::new("dot", 0.0, 0.0);
  let rect = Rect::new(0.0, 0.0, 2.0, 3.0);
  let square = Square::new(1.0, 1.0, 4.0);

  // Accessors and methods of ancestors work on subclasses.
  square.move_by(1.0, 2.0);
  print_summary(&rect);
  print_summary(&square);

  // A canvas of `Shape`s holds the same objects, and every call is late-bound to the object's own class.
  let canvas = List::new();
  add_to_canvas(&canvas, dot);
  add_to_canvas(&canvas, rect.clone());
  add_to_canvas(&canvas, square.clone());
  for shape in canvas.iter() {
    shape.move_by(10.0, 0.0);
    println!("{}: {}", shape.name(), block_on(shape.render("the canvas")));
  }
  println!("{rect:?}");

  // Ask what a shape really is, and get a handle to it as that class.
  for shape in canvas.iter() {
    match shape.downcast::<Rect>() {
      Some(rect) => println!("{} is a rect {} wide", rect.name(), rect.width()),
      None => println!("{} isn't a rect", shape.name()),
    }
  }
  let as_shape: Shape = square.upcast();
  assert!(as_shape.is::<Square>() && as_shape.ptr_eq(&square));
}

/// Runs a future to completion on this thread. The futures above never wait, so this is enough here; real
/// code would use an executor such as Tokio.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
  let mut future = std::pin::pin!(future);
  let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
  loop {
    if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut cx) {
      return output;
    }
  }
}
