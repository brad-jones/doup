//! A small issue tracker. Users, tasks and projects are classes: handing a `User` to a task
//! doesn't copy it, so renaming the user is visible from every task assigned to them.

mod tracker {
  use galvanize_rs::{ArcSwapStrategy, GList, class};

  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub enum Status {
    #[default]
    Todo,
    InProgress,
    Done,
  }

  #[class]
  #[derive(Debug)]
  pub struct User {
    pub name: String,
    pub email: String,
  }

  impl User {
    pub fn new(name: &str, email: &str) -> Self {
      Self::from(UserData {
        name: name.into(),
        email: email.into(),
      })
    }
  }

  #[class]
  #[derive(Debug, Default)]
  pub struct Task {
    pub title: String,
    pub status: Status,
    pub assignee: Option<User>,
  }

  impl Task {
    pub fn new(title: &str) -> Self {
      Self::from(TaskData {
        title: title.into(),
        ..Default::default()
      })
    }

    /// Shares `user` with this task rather than copying them.
    pub fn assign(&self, user: &User) {
      self.set_assignee(Some(user.clone()));
    }

    pub fn advance(&self) {
      self.update_status(|status| {
        *status = match *status {
          Status::Todo => Status::InProgress,
          Status::InProgress | Status::Done => Status::Done,
        }
      });
    }

    pub fn describe(&self) -> String {
      let who = self.assignee().map_or("nobody".to_string(), |user| user.name());
      format!("{} [{:?}] -> {who}", self.title(), self.status())
    }
  }

  /// Read-mostly and shared widely, so it uses the lock-free `ArcSwapStrategy`.
  #[class(pointer_strategy = ArcSwapStrategy)]
  #[derive(Debug, Default)]
  pub struct Project {
    pub name: String,
    pub tasks: GList<Task>,
  }

  impl Project {
    pub fn new(name: &str) -> Self {
      Self::from(ProjectData {
        name: name.into(),
        ..Default::default()
      })
    }

    pub fn add_task(&self, title: &str) -> Task {
      let task = Task::new(title);
      self.tasks().push(task.clone());
      task
    }

    pub fn tasks_for(&self, user: &User) -> Vec<Task> {
      self
        .tasks()
        .iter()
        .filter(|task| task.assignee().is_some_and(|assignee| assignee == *user))
        .collect()
    }

    pub fn report(&self) {
      println!("{}", self.name());
      for task in self.tasks().iter() {
        println!("  {}", task.describe());
      }
    }
  }
}

use tracker::{Project, Status, User};

fn main() {
  let alice = User::new("Alice", "alice@example.com");
  let bob = User::new("Bob", "bob@example.com");

  let project = Project::new("Website relaunch");
  let design = project.add_task("Design landing page");
  let build = project.add_task("Build landing page");
  let launch = project.add_task("Launch");

  design.assign(&alice);
  build.assign(&alice);
  launch.assign(&bob);

  design.advance();
  design.advance();
  build.advance();
  project.report();

  // Every task assigned to Alice sees the rename, because they all share the one `User`.
  alice.set_name("Alice Smith".into());
  println!("\nAfter renaming Alice:");
  project.report();
  assert_eq!(project.tasks_for(&alice).len(), 2);

  // `design` and the project's list are the same task, not copies.
  assert!(project.tasks().first() == Some(design.clone()));
  assert_eq!(project.tasks().first().unwrap().status(), Status::Done);

  // `duplicate` copies the task's own fields, but the assignee is still shared.
  let followup = build.duplicate();
  followup.set_title("Build the pricing page".into());
  followup.assignee().unwrap().set_email("alice.smith@example.com".into());
  assert_eq!(build.title(), "Build landing page");
  assert_eq!(alice.email(), "alice.smith@example.com");

  println!("\n{followup:?}");
}
