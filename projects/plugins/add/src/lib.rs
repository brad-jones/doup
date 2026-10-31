wit_bindgen::generate!({
    world: "plugin",
    path: "../../wit",
});

use calculator::plugin::host_api;

struct Plugin;

impl Guest for Plugin {
  fn operation(a: i64, b: i64) -> Result<i64, ()> {
    let version = host_api::host_version();
    println!("[plugin ADD] Hello, {version}!");
    a.checked_add(b).ok_or(())
  }
}

export!(Plugin);
