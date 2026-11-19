use galvanize_rs::{Extends, List, Str, class, sf};

// ---------- Shape ----------

/// Something drawn on a canvas.
#[class]
pub struct Shape {
  pub name: Str,
  pub x: f64,
  pub y: f64,
}

impl Shape {
  pub fn new(name: impl Into<Str>, x: f64, y: f64) -> Self {
    Self::from_data(ShapeData {
      name: name.into(),
      x,
      y,
    })
  }

  pub fn move_by(&self, dx: f64, dy: f64) {
    self.update_x(|x| *x += dx);
    self.update_y(|y| *y += dy);
  }

  pub fn describe(&self) -> Str {
    sf!("{} at ({}, {})", self.name(), self.x(), self.y())
  }
}

// ---------- Rect ----------

/// A rectangle is a shape with a size.
#[class(extends = Shape)]
pub struct Rect {
  pub width: f64,
  pub height: f64,
}

impl Rect {
  pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
    Self::from_data(RectData {
      // Build the base part with the base class's own constructor.
      base: Shape::new("rect", x, y),
      width,
      height,
    })
  }

  pub fn area(&self) -> f64 {
    self.width() * self.height()
  }

  // Shadows `Shape::describe` when called on a `Rect`. Calling it through a
  // `Shape` still runs `Shape::describe`, it doesn't override it.
  pub fn describe(&self) -> Str {
    sf!("{} sized {}x{}", self.base().describe(), self.width(), self.height())
  }
}

// ---------- Square ----------

/// Inheritance chains: a square is a rect, which is a shape.
#[class(extends = Rect)]
pub struct Square {}

impl Square {
  pub fn new(x: f64, y: f64, size: f64) -> Self {
    let rect = Rect::new(x, y, size, size);
    rect.set_name("square");
    Self::from_data(SquareData { base: rect })
  }
}

/// Takes any shape by reference: `&Rect` and `&Square` coerce to `&Shape`.
fn print_position(shape: &Shape) {
  println!("{} is at ({}, {})", shape.name(), shape.x(), shape.y());
}

/// Takes any shape by value, via `Extends`.
fn add_to_canvas<I>(canvas: &List<Shape>, shape: impl Extends<Shape, I>) {
  canvas.push(shape.upcast());
}

fn main() {
  let rect = Rect::new(0.0, 0.0, 2.0, 3.0);
  let square = Square::new(1.0, 1.0, 4.0);

  // Base class methods and accessors work on subclasses.
  square.move_by(1.0, 2.0);
  print_position(&rect);
  print_position(&square);
  println!("{}, area {}", square.describe(), square.area());

  let canvas = List::new();
  add_to_canvas(&canvas, rect.clone());
  add_to_canvas(&canvas, square.clone());

  // The canvas holds the same objects, so moving them there moves `rect` and `square` too.
  for shape in canvas.iter() {
    shape.move_by(10.0, 0.0);
  }
  println!("{:?}", rect);
  println!("{:?}", square);
}
