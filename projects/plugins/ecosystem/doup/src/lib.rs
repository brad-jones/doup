wit_bindgen::generate!({
    world: "ecosystem",
    path: "../../../wit",
});

use doup::plugin::host_api;

struct Plugin;

impl Guest for Plugin {
  fn hello_world() -> String {
    let version = host_api::host_version();
    println!("[plugin DOUP] Hello, {version}!");
    "hello world".to_string()
  }
}

export!(Plugin);
