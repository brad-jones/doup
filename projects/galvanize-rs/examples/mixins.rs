//! The mixin pattern with several traits.
//!
//! Rust has no multiple inheritance, but a class can implement as many traits as it likes
//! and each trait contributes default methods, which is what a mixin is. As in `shapes.rs`,
//! a trait can't see the implementor's fields, so each mixin requires one method that returns a
//! handle to the small class holding the state it works on, and everything else is a default
//! method built on that handle.
//!
//! - `Health` needs a `Vitals` and adds `damage`, `heal` and `is_alive`.
//! - `Movable` needs a `Position` and adds `move_by` and `distance_from_origin`.
//! - `Describe` is a mixin of mixins: it's implemented for anything that is both.

mod game {
  use galvanize_rs::class;

  #[class]
  pub struct Vitals {
    pub hp: i32,
    pub max_hp: i32,
  }

  impl Vitals {
    fn new(max_hp: i32) -> Self {
      Self::from(VitalsData { hp: max_hp, max_hp })
    }
  }

  #[class]
  #[derive(Default)]
  pub struct Position {
    pub x: f64,
    pub y: f64,
  }

  pub trait Health {
    /// Required: the state every default method below works on.
    fn vitals(&self) -> Vitals;

    fn is_alive(&self) -> bool {
      self.vitals().hp() > 0
    }

    fn damage(&self, amount: i32) {
      self.vitals().update_hp(|hp| *hp = (*hp - amount).max(0));
    }

    fn heal(&self, amount: i32) {
      let vitals = self.vitals();

      // Works, but not ideal: reading the cap first avoids a deadlock (calling `max_hp()`
      // inside `update_hp` would hang), yet the read and the update are two separate steps,
      // so `max_hp` could change in between.
      // let max_hp = vitals.max_hp();
      // vitals.update_hp(|hp| *hp = (*hp + amount).min(max_hp));

      // Better: the closure is handed every field, so there's no need to call accessors
      // (which would take the lock again), and the whole update is atomic.
      vitals.write_with(|v| v.hp = (v.hp + amount).min(v.max_hp));
    }
  }

  pub trait Movable {
    fn position(&self) -> Position;

    fn move_by(&self, dx: f64, dy: f64) {
      let position = self.position();
      position.update_x(|x| *x += dx);
      position.update_y(|y| *y += dy);
    }

    fn distance_from_origin(&self) -> f64 {
      let position = self.position();
      position.x().hypot(position.y())
    }
  }

  /// A mixin built from other mixins. The blanket impl below gives `status` to every type
  /// that is both `Health` and `Movable`, without any of them opting in.
  pub trait Describe: Health + Movable {
    fn status(&self) -> String {
      format!(
        "hp {}/{}, {:.1} from origin",
        self.vitals().hp(),
        self.vitals().max_hp(),
        self.distance_from_origin()
      )
    }
  }

  impl<T: Health + Movable> Describe for T {}

  /// Has health and moves: gets both mixins, and therefore `Describe`.
  #[class]
  pub struct Player {
    pub name: String,
    body: Vitals,
    location: Position,
  }

  impl Player {
    pub fn new(name: &str) -> Self {
      Self::from(PlayerData {
        name: name.into(),
        body: Vitals::new(100),
        location: Position::default(),
      })
    }
  }

  impl Health for Player {
    fn vitals(&self) -> Vitals {
      self.body()
    }
  }

  impl Movable for Player {
    fn position(&self) -> Position {
      self.location()
    }
  }

  /// Moves but has no health, so it only gets `Movable`.
  #[class]
  pub struct Boulder {
    location: Position,
  }

  impl Boulder {
    pub fn new() -> Self {
      Self::from(BoulderData {
        location: Position::default(),
      })
    }
  }

  impl Movable for Boulder {
    fn position(&self) -> Position {
      self.location()
    }
  }

  /// Has health but never moves, so it only gets `Health`.
  #[class]
  pub struct Turret {
    body: Vitals,
  }

  impl Turret {
    pub fn new() -> Self {
      Self::from(TurretData { body: Vitals::new(40) })
    }
  }

  impl Health for Turret {
    fn vitals(&self) -> Vitals {
      self.body()
    }
  }
}

use game::{Boulder, Describe, Health, Movable, Player, Turret};

/// Works for anything that mixes in both traits.
fn ambush(target: &(impl Health + Movable)) {
  target.move_by(3.0, 4.0);
  target.damage(30);
}

fn main() {
  let player = Player::new("Ada");
  let boulder = Boulder::new();
  let turret = Turret::new();

  ambush(&player);
  println!("{}: {}", player.name(), player.status());

  player.heal(1000); // capped at max_hp
  println!("{}: {}", player.name(), player.status());

  boulder.move_by(-6.0, 8.0);
  println!("boulder is {:.1} from origin", boulder.distance_from_origin());
  // boulder.status(); // doesn't compile: a boulder has no `Health`

  turret.damage(100);
  println!("turret alive: {}", turret.is_alive());
  // ambush(&turret); // doesn't compile either: a turret isn't `Movable`

  assert!(player.is_alive());
  assert!(!turret.is_alive());
  assert_eq!(boulder.distance_from_origin(), 10.0);
}
