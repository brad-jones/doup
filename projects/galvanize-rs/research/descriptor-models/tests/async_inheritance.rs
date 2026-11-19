#![forbid(unsafe_code)]

use galvanize_rs_descriptor_models::{
  AsyncAnimal, AsyncBoxed as ImportedBox, AsyncBoxedData, AsyncDog as ImportedDog, class, class_methods,
};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_overrides_cross_crates_and_remain_send() {
  let puppy = AsyncPuppy::from_data(AsyncPuppyData {
    base: ImportedDog::init("Bit"),
    suffix: "puppy".into(),
  });
  let animal: AsyncAnimal = puppy.upcast();
  drop(puppy);
  let task = tokio::spawn(async move {
    let prefix = String::from("hello ");
    assert_eq!(animal.describe(&prefix).await, Ok("[hello Bit:yip/dog/puppy]".into()));
    assert_eq!(animal.describe("fail").await, Err("requested failure"));
    assert!(animal.is::<AsyncPuppy>());
  });
  task.await.expect("the Send future completed without panicking");

  let beagle = AsyncBeagle::from_data(AsyncBeagleData {
    base: ImportedDog::init("Rex"),
  });
  let animal: AsyncAnimal = beagle.upcast();
  assert_eq!(animal.describe("").await, Ok("[Rex:woof/dog]".into()));
  let root = AsyncAnimal::from_data(AsyncAnimal::init("Root"));
  assert_eq!(root.speak("").await, Ok("Root:...".into()));
}

#[tokio::test]
async fn borrowed_arguments_and_callbacks_survive_await_and_super() {
  let puppy = AsyncPuppy::from_data(AsyncPuppyData {
    base: ImportedDog::init("Bit"),
    suffix: "!".into(),
  });
  let animal: AsyncAnimal = puppy.upcast();
  let prefix = String::from("hello ");
  let mut output = String::new();
  animal.append(Some(&prefix), &mut output, str::to_uppercase).await;
  assert_eq!(output, "HELLO yip!");
}

#[tokio::test]
async fn generic_async_overrides_preserve_types_and_lazy_execution() {
  let tagged = AsyncTagged::from_data(AsyncTaggedData {
    base: AsyncBoxedData {
      value: String::from("before"),
    },
  });
  let base: ImportedBox<String> = tagged.upcast();
  let future = base.get();
  tagged.set_value("after".into());
  assert_eq!(future.await, "after");
  assert!(base.downcast::<AsyncTagged<String>>().is_some());
  assert!(base.downcast::<AsyncTagged<u64>>().is_none());
}

#[test]
fn cancelling_a_suspended_override_releases_its_values_and_views() {
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
  assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
  assert_eq!(weak.strong_count(), 1);
  assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
  assert_eq!(weak.strong_count(), 2);
  drop(future);
  assert_eq!(weak.strong_count(), 1);
  drop(base);
  assert!(weak.upgrade().is_none());
}
