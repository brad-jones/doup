//! Implementing a trait on a class, with a `GList` of trait objects.
//!
//! A trait has no access to the fields of the type implementing it: there is no `this`
//! and no late binding of data. A default method can only call the other methods of the
//! trait. So when the trait wants shared behaviour that reads or writes data, it must
//! declare a method that hands back a *handle* to a class holding that data, and each
//! implementor fulfils the trait by returning its own handle.
//!
//! Here `Shape::position` is that method. `describe` and `move_by` are written once in the
//! trait, using only `position()`, and every shape gets them for free.

mod shapes {
  use galvanize_rs::{any, class};
  use std::f64::consts::PI;

  /// Data shared by every shape. It is a class so that handles to it are cheap and share state.
  #[class]
  pub struct Placement {
    pub name: String,
    pub x: f64,
    pub y: f64,
  }

  impl Placement {
    fn new(name: &str) -> Self {
      Self::from(PlacementData {
        name: name.into(),
        x: 0.0,
        y: 0.0,
      })
    }
  }

  /// `#[any]` also generates `AnyShape`, a cloneable `Arc<dyn Shape>` wrapper that lets shapes
  /// of different classes share a `GList`. (`GPointer<dyn Shape>` isn't possible: a class
  /// handle wraps `GPointer<CircleData>`, and `CircleData` isn't a `Shape`, the handle is.)
  #[any]
  pub trait Shape {
    /// Required: a handle to this shape's `Placement`. This is the only way the trait's
    /// default methods can reach any data, because a trait can't see the implementor's fields.
    fn position(&self) -> Placement;

    /// Required: only the implementor knows its own dimensions.
    fn area(&self) -> f64;

    fn describe(&self) -> String {
      let place = self.position();
      format!(
        "{} at ({:.1}, {:.1}), area {:.2}",
        place.name(),
        place.x(),
        place.y(),
        self.area()
      )
    }

    fn move_by(&self, dx: f64, dy: f64) {
      // The handle shares state with the shape, so this moves the shape itself.
      let place = self.position();
      place.update_x(|x| *x += dx);
      place.update_y(|y| *y += dy);
    }
  }

  #[class]
  pub struct Circle {
    placement: Placement,
    pub radius: f64,
  }

  impl Circle {
    pub fn new(radius: f64) -> Self {
      Self::from(CircleData {
        placement: Placement::new("circle"),
        radius,
      })
    }
  }

  impl Shape for Circle {
    // The class's generated getter `placement()` is an ordinary method, not part of the
    // trait. The trait needs its own method, which we fulfil by returning that handle.
    fn position(&self) -> Placement {
      self.placement()
    }

    fn area(&self) -> f64 {
      PI * self.radius().powi(2)
    }
  }

  #[class]
  pub struct Rectangle {
    placement: Placement,
    pub width: f64,
    pub height: f64,
  }

  impl Rectangle {
    pub fn new(width: f64, height: f64) -> Self {
      Self::from(RectangleData {
        placement: Placement::new("rectangle"),
        width,
        height,
      })
    }
  }

  impl Shape for Rectangle {
    fn position(&self) -> Placement {
      self.placement()
    }

    fn area(&self) -> f64 {
      self.width() * self.height()
    }
  }
}

use galvanize_rs::{GList, list};
use shapes::{AnyShape, Circle, Rectangle, Shape};

fn total_area(shapes: &GList<AnyShape>) -> f64 {
  shapes.iter().map(|shape| shape.area()).sum()
}

fn main() {
  let circle = Circle::new(1.0);
  let rect = Rectangle::new(2.0, 3.0);

  // The list holds clones of the handles, so these are the same objects as `circle` and `rect`.
  let shapes: GList<AnyShape> = list![&circle, &rect];

  for shape in shapes.iter() {
    println!("{}", shape.describe());
  }
  println!("total area: {:.2}\n", total_area(&shapes));

  // Mutating through our own handles is visible through the list...
  circle.set_radius(2.0);
  // ...and so is mutating through the trait's default method.
  shapes.first().unwrap().move_by(5.0, -1.0);

  for shape in shapes.iter() {
    println!("{}", shape.describe());
  }
  println!("total area: {:.2}", total_area(&shapes));

  assert_eq!(circle.position().x(), 5.0);
}
