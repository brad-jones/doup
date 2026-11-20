// Async virtual methods: suspension, borrowed inputs, `super`, laziness, thread migration and cancellation,
// run against the fixture crate in scope as `fixtures`. See `scenarios.rs` for how it's shared.

use fixtures::{
  AsyncAnimal, AsyncBoxed as ImportedBox, AsyncBoxedData, AsyncDog as ImportedDog, AsyncSource, AsyncSourceData, Lookup,
};
use galvanize_rs::{class, class_methods};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

#[class(extends = ImportedDog)]
pub struct AsyncPuppy {
  pub suffix: String,
}

#[class_methods]
impl AsyncPuppy {
  #[class_override]
  pub fn sound(&self) -> String {
    "yip".into()
  }

  #[class_override]
  pub async fn speak(&self, prefix: &str) -> Result<String, &'static str> {
    Ok(format!("{}/{}", self.super_speak(prefix).await?, self.suffix()))
  }

  #[class_override]
  pub async fn append(&self, prefix: Option<&str>, output: &mut String, convert: fn(&str) -> String) {
    self.super_append(prefix, output, convert).await;
    output.push_str(&self.suffix());
  }
}

#[class(extends = ImportedDog)]
pub struct AsyncBeagle {}

#[class_methods]
impl AsyncBeagle {}

#[class(open, extends = ImportedBox<U>)]
pub struct AsyncTagged<U> {}

#[class_methods]
impl<U> AsyncTagged<U> {
  #[class_override]
  pub async fn get(&self) -> U {
    self.super_get().await
  }
}

#[class(extends = AsyncTagged<V>)]
pub struct PendingBox<V> {}

#[class_methods]
impl<V> PendingBox<V> {
  #[class_override]
  pub async fn get(&self) -> V {
    let value = self.super_get().await;
    std::future::pending::<()>().await;
    value
  }
}

// These fixtures only yield finitely or intentionally stay pending; no timer or I/O executor is needed.
fn run<F: Future>(future: F) -> F::Output {
  let mut future = Box::pin(future);
  let mut cx = Context::from_waker(Waker::noop());
  for _ in 0..32 {
    if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
      return value;
    }
  }
  panic!("fixture did not complete within its expected suspension bound");
}

fn puppy() -> AsyncPuppy {
  AsyncPuppy::from_data(AsyncPuppyData {
    base: ImportedDog::init("Bit"),
    suffix: "puppy".into(),
  })
}

#[test]
fn async_overrides_cross_crates_and_preserve_late_binding() {
  let puppy = puppy();
  let animal: AsyncAnimal = puppy.upcast();
  drop(puppy);
  let prefix = String::from("hello ");
  assert_eq!(run(animal.describe(&prefix)), Ok("[hello Bit:yip/dog/puppy]".into()));
  assert_eq!(run(animal.describe("fail")), Err("requested failure"));
  assert!(animal.is::<AsyncPuppy>());
  let beagle = AsyncBeagle::from_data(AsyncBeagleData {
    base: ImportedDog::init("Rex"),
  });
  let animal: AsyncAnimal = beagle.upcast();
  assert_eq!(run(animal.describe("")), Ok("[Rex:woof/dog]".into()));
  let root = AsyncAnimal::new("Root");
  assert_eq!(run(root.speak("")), Ok("Root:...".into()));
}

#[test]
fn borrowed_mutable_optional_arguments_and_callback_binders_survive_super() {
  let puppy = puppy();
  puppy.set_suffix("!");
  let animal: AsyncAnimal = puppy.upcast();
  let prefix = String::from("hello ");
  let mut output = String::new();
  run(animal.append(Some(&prefix), &mut output, str::to_uppercase));
  assert_eq!(output, "HELLO yip!");
  output.clear();
  run(animal.append(None, &mut output, str::to_uppercase));
  assert_eq!(output, "yip!");
}

#[test]
fn generic_results_are_lazy_and_a_suspended_borrowed_future_can_change_threads() {
  let object = AsyncTagged::from_data(AsyncTaggedData {
    base: AsyncBoxedData {
      value: String::from("before"),
    },
  });
  let base: ImportedBox<String> = object.upcast();
  let mut future = Box::pin(base.get());
  object.set_value("after");
  assert!(
    future
      .as_mut()
      .poll(&mut Context::from_waker(Waker::noop()))
      .is_pending()
  );
  std::thread::scope(|scope| {
    assert_eq!(scope.spawn(move || run(future)).join().expect("future thread"), "after");
  });
  assert!(base.downcast::<AsyncTagged<String>>().is_some());
  assert!(base.downcast::<AsyncTagged<u64>>().is_none());
}

#[test]
fn cancellation_releases_values_and_borrowed_views_at_each_suspension() {
  for polls in 0..=2 {
    let value = Arc::new(());
    let weak = Arc::downgrade(&value);
    let object = PendingBox::from_data(PendingBoxData {
      base: AsyncTaggedData {
        base: AsyncBoxedData { value },
      },
    });
    let base: ImportedBox<Arc<()>> = object.upcast();
    drop(object);
    let mut future = Box::pin(base.get());
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..polls {
      assert!(future.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(weak.strong_count(), if polls == 2 { 2 } else { 1 });
    drop(future);
    assert_eq!(weak.strong_count(), 1);
    drop(base);
    assert!(weak.upgrade().is_none());
  }
}

#[test]
fn suspended_super_borrows_inputs_until_completion_or_cancellation() {
  let animal: AsyncAnimal = puppy().upcast();
  let prefix = String::from("borrowed ");
  let mut output = String::new();
  let mut future = Box::pin(animal.append(Some(&prefix), &mut output, str::to_uppercase));
  assert!(
    future
      .as_mut()
      .poll(&mut Context::from_waker(Waker::noop()))
      .is_pending()
  );
  drop(future);
  output.push_str("released");
  assert_eq!(output, "released");
  assert_eq!(run(animal.describe(&prefix)), Ok("[borrowed Bit:yip/dog/puppy]".into()));
}

// Tokio's runtime is too slow under Miri; the paths it exercises are covered by the tests above.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(miri, ignore)]
async fn owning_async_tasks_spawn_on_tokio() {
  let animal: AsyncAnimal = puppy().upcast();
  tokio::spawn(async move {
    let prefix = String::from("hello ");
    assert_eq!(animal.describe(&prefix).await, Ok("[hello Bit:yip/dog/puppy]".into()));
  })
  .await
  .expect("Send task completed");
}

#[class(extends = AsyncSource)]
pub struct MemorySource {
  pub prefix: String,
}

#[class_methods]
impl MemorySource {
  async fn lookup(&self, key: &str) -> Result<String, &'static str> {
    fixtures::suspend_once().await;
    if key.is_empty() {
      return Err("empty key");
    }
    Ok(format!("{}{}:{key}", self.prefix(), self.name()))
  }
}

#[test]
fn abstract_async_methods_dispatch_through_interfaces() {
  let source = MemorySource::from_data(MemorySourceData {
    base: AsyncSourceData { name: "mem".into() },
    prefix: "@".into(),
  });
  let base: AsyncSource = source.upcast();
  assert_eq!(run(Lookup::lookup(&base, "k")), Ok("@mem:k".to_string()));
  assert_eq!(run(source.lookup("j")), Ok("@mem:j".to_string()));
  assert_eq!(
    run(base.lookup_all(&["a", "b"])),
    Ok(vec!["@mem:a".to_string(), "@mem:b".to_string()])
  );
  assert_eq!(run(base.lookup_all(&["a", ""])), Err("empty key"));
}
