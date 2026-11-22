//! Composition instead of inheritance: a `Car` isn't a kind of `Engine`, it has one.
//! Rust has no class inheritance, so behaviour is reused by holding another class
//! and delegating to it.

mod vehicles {
  use galvanize_rs::class;

  #[class]
  #[derive(Debug, Default)]
  pub struct Engine {
    pub running: bool,
    pub rpm: u32,
  }

  impl Engine {
    pub fn start(&self) {
      self.set_running(true);
      self.set_rpm(800);
    }

    pub fn stop(&self) {
      self.set_running(false);
      self.set_rpm(0);
    }
  }

  #[class]
  #[derive(Debug)]
  pub struct Car {
    pub model: String,
    pub engine: Engine,
  }

  impl Car {
    pub fn new(model: &str) -> Self {
      Self::from(CarData {
        model: model.into(),
        engine: Engine::default(),
      })
    }

    /// The car doesn't know how an engine starts, it just asks its engine to.
    pub fn start(&self) {
      self.engine().start();
    }

    pub fn stop(&self) {
      self.engine().stop();
    }
  }
}

use vehicles::Car;

fn main() {
  let car = Car::new("Roadster");

  // `engine()` returns a handle to the car's own engine, not a copy.
  let engine = car.engine();
  println!("{} running: {}", car.model(), engine.running());

  car.start();
  println!("running: {}, rpm: {}", engine.running(), engine.rpm());
  assert!(engine.running());

  car.stop();
  assert!(!engine.running());
  println!("{car:?}");
}
