use wasmtime::component::bindgen;

bindgen!({
  path: "../wit",
  world: "platform",
  exports: { default: async },
  with: { "doup:plugin/host-api": super::host::host_api },
});
