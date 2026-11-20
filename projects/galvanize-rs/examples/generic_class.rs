use galvanize_rs::{ArcSwapStrategy, DefaultPointerStrategy, Dict, List, PointerStrategy, class, class_methods};

// ---------- Tree ----------

/// A tree of values.
///
/// The macro bounds `T` with `Clone + Send + Sync + 'static`, and `Self` refers to `Tree<T>`.
#[class]
pub struct Tree<T> {
  pub value: T,
  children: List<Self>,
}

// The struct's bounds apply to the methods block, so it doesn't repeat them.
#[class_methods]
impl<T> Tree<T> {
  pub fn new(value: T) -> Self {
    Self::from_data(TreeData {
      value,
      children: List::new(),
    })
  }

  /// Adds a child and returns it, so it can be given children of its own.
  pub fn add(&self, value: T) -> Self {
    let child = Self::new(value);
    self.children().push(&child);
    child
  }

  pub fn size(&self) -> usize {
    1 + self.children().iter().map(|c| c.size()).sum::<usize>()
  }

  /// Visits every value, depth first.
  pub fn for_each(&self, f: &mut impl FnMut(&T)) {
    f(&self.value());
    for child in self.children().iter() {
      child.for_each(f);
    }
  }
}

// ---------- Cache ----------

/// Remembers computed values by key.
///
/// `S` picks the pointer strategy of every field. It's only used by the
/// strategies, so it doesn't need to be `Clone` and `CacheData` doesn't have it.
#[class(pointer_strategy = S)]
pub struct Cache<K: Ord, V, S: PointerStrategy = DefaultPointerStrategy> {
  entries: Dict<K, V>,
  #[class_prop(readonly)]
  pub hits: usize,
  #[class_prop(readonly)]
  pub misses: usize,
}

#[class_methods]
impl<K, V, S> Cache<K, V, S> {
  pub fn new() -> Self {
    Self::from_data(CacheData {
      entries: Dict::new(),
      hits: 0,
      misses: 0,
    })
  }

  pub fn get_or_compute(&self, key: K, compute: impl FnOnce() -> V) -> V {
    if let Some(value) = self.entries().get(&key) {
      self.update_hits(|n| *n += 1);
      return value;
    }
    self.update_misses(|n| *n += 1);
    let value = compute();
    self.entries().insert(key, value.clone());
    value
  }
}

// Other impls must repeat the bounds the macro adds.
impl<K, V, S> Default for Cache<K, V, S>
where
  K: Ord + Clone + Send + Sync + 'static,
  V: Clone + Send + Sync + 'static,
  S: PointerStrategy + 'static,
{
  fn default() -> Self {
    Self::new()
  }
}

fn main() {
  let root = Tree::new("galvanize-rs");
  let src = root.add("src");
  src.add("lib.rs");
  src.add("list/mod.rs");
  root.add("README.md");

  // `src` is the same object as the child stored in `root`, so this shows up in `root` too.
  src.add("dict/mod.rs");

  let mut paths = Vec::new();
  root.for_each(&mut |p| paths.push(*p));
  println!("{} nodes: {}", root.size(), paths.join(", "));

  // Uses the default pointer strategy.
  let lengths: Cache<&str, usize> = Cache::new();
  for word in ["class", "generic", "class"] {
    lengths.get_or_compute(word, || word.len());
  }
  println!("lengths: {} hits, {} misses", lengths.hits(), lengths.misses());

  // Every field of this cache is stored in an `ArcSwapPointer` instead.
  let squares: Cache<u64, u64, ArcSwapStrategy> = Cache::new();
  let worker = squares.clone();
  std::thread::spawn(move || {
    for n in [2, 3, 2] {
      worker.get_or_compute(n, || n * n);
    }
  })
  .join()
  .unwrap();
  println!("{squares:?}");
}
