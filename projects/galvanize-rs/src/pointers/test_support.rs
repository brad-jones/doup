/// Generates the behavioural tests every [`GPointer`](super::GPointer)
/// implementation must satisfy.
macro_rules! pointer_contract_tests {
  ($pointer:ident) => {
    #[test]
    fn read_with_returns_closure_result() {
      let pointer = $pointer::new(vec![1, 2, 3]);
      assert_eq!(pointer.read_with(|v| v.len()), 3);
    }

    #[test]
    fn write_with_mutates_and_returns_closure_result() {
      let pointer = $pointer::new(vec![1]);
      let len = pointer.write_with(|v| {
        v.push(2);
        v.len()
      });
      assert_eq!(len, 2);
      assert_eq!(pointer.read_with(|v| v.clone()), vec![1, 2]);
    }

    #[test]
    fn clone_shares_the_same_value() {
      let a = $pointer::new(vec![1]);
      let b = a.clone();
      b.write_with(|v| v.push(2));
      a.write_with(|v| v.push(3));
      assert_eq!(a.read_with(|v| v.clone()), vec![1, 2, 3]);
      assert_eq!(b.read_with(|v| v.clone()), vec![1, 2, 3]);
    }

    #[test]
    fn deep_clone_is_independent() {
      let a = $pointer::new(vec![1, 2]);
      let b = a.deep_clone();
      assert_eq!(b.read_with(|v| v.clone()), vec![1, 2]);

      b.write_with(|v| v.clear());
      a.write_with(|v| v.push(3));
      assert_eq!(a.read_with(|v| v.clone()), vec![1, 2, 3]);
      assert!(b.read_with(|v| v.is_empty()));
    }

    #[test]
    fn concurrent_writes_are_not_lost() {
      const THREADS: usize = 8;
      const INCREMENTS: usize = 500;

      let pointer = $pointer::new(0usize);
      let handles: Vec<_> = (0..THREADS)
        .map(|_| {
          let pointer = pointer.clone();
          std::thread::spawn(move || {
            for _ in 0..INCREMENTS {
              pointer.write_with(|v| *v += 1);
            }
          })
        })
        .collect();
      for handle in handles {
        handle.join().expect("writer thread panicked");
      }

      assert_eq!(pointer.read_with(|v| *v), THREADS * INCREMENTS);
    }
  };
}
