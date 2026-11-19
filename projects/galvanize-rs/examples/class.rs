use galvanize_rs::{ArcSwapStrategy, List, Str, class, sf};

// ---------- Person ----------

/// A person and the cities they have visited.
#[class]
pub struct Person {
  // A public field gets a public getter, setter and updater.
  pub name: Str,

  // A private field gets them too, they're just private.
  cities: List<Str>,
}

impl Person {
  pub fn new(first_name: impl Into<Str>, last_name: impl Into<Str>) -> Self {
    Self::from_data(PersonData {
      name: sf!("{} {}", first_name.into(), last_name.into()),
      cities: List::new(),
    })
  }

  pub fn visit(&self, city: impl Into<Str>) {
    self.cities().push(city);
  }

  pub fn visited(&self) -> usize {
    self.cities().len()
  }
}

// ---------- Team ----------

/// A team and the person who leads it.
///
/// Stores every field in an `ArcSwapPointer` instead of the default.
#[class(strategy = ArcSwapStrategy)]
pub struct Team {
  pub name: Str,

  // Anyone can read the leader, but only `Team` can replace them.
  #[class_prop(readonly)]
  pub leader: Person,
}

impl Team {
  pub fn new(name: impl Into<Str>, leader: impl Into<Person>) -> Self {
    Self::from_data(TeamData {
      name: name.into(),
      leader: leader.into(),
    })
  }

  pub fn describe(&self) -> Str {
    let leader = self.leader();
    sf!(
      "{} (led by {}, who has visited {} cities)",
      self.name(),
      leader.name(),
      leader.visited()
    )
  }
}

fn main() {
  let brad = Person::new("Brad", "Jones");
  let team = Team::new("Platform", &brad);

  // `brad` and `team.leader()` are the same object, so changes show up through both.
  brad.visit("Melbourne");
  team.leader().visit("Sydney");
  team.set_name("Platform Engineering");

  println!("{}", team.describe());
  println!("{team:?}");
}
