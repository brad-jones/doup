use galvanize_rs::{ArcSwapStrategy, GList, GString, class, class_methods, sf};

// ---------- Person ----------

/// A person and the cities they have visited.
#[class]
pub struct Person {
  /// A public field gets a public getter, setter and updater.
  pub name: GString,

  /// A private field gets them too, they're just private.
  cities: GList<GString>,
}

// Every class needs one `#[class_methods]` block, which holds its methods (and can be empty).
#[class_methods]
impl Person {
  pub fn new(first_name: impl Into<GString>, last_name: impl Into<GString>) -> Self {
    Self::from_data(PersonData {
      name: sf!("{} {}", first_name.into(), last_name.into()),
      cities: GList::new(),
    })
  }

  pub fn visit(&self, city: impl Into<GString>) {
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
#[class(pointer_strategy = ArcSwapStrategy)]
pub struct Team {
  pub name: GString,

  /// Anyone can read the leader, but only `Team` can replace them.
  #[class_prop(readonly)]
  pub leader: Person,
}

#[class_methods]
impl Team {
  pub fn new(name: impl Into<GString>, leader: impl Into<Person>) -> Self {
    Self::from_data(TeamData {
      name: name.into(),
      leader: leader.into(),
    })
  }

  pub fn describe(&self) -> GString {
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
