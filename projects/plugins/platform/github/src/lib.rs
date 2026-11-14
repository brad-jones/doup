wit_bindgen::generate!({
    world: "platform",
    path: "../../../wit",
});

use doup::plugin::host_api;

struct Plugin;

impl Guest for Plugin {
  fn hello_world() -> String {
    let version = host_api::host_version();
    println!("[plugin GITHUB] Hello, {version}!");
    "hello world".to_string()
  }
}

export!(Plugin);
