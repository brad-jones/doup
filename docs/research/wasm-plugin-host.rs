async fn example() -> Result<()> {
  // Shared process wide
  let engine = Engine::default();

  // Could be shared per WIT world so two can diverge (eg: only some worlds get wasi:http).
  // Otherwise one linker process wide.
  let mut linker = Linker::new(&engine);
  add_to_linker_async(&mut linker)?;
  host_api::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;

  // Pre init a component
  // Do this once per wasm file
  // EcosystemPre came from the bindgen macro
  let component = EcosystemPre::new(linker.instantiate_pre(&Component::from_file(&engine, "examples/hello.wasm")?)?)?;

  // Create a new store for the component, this is what houses the WASM module's guest memory.
  // So if a component is given a new one of these it's as if it was being called for the very first time.
  let mut store = Store::new(
    &engine,
    HostState::new(|wasi| {
      // This is where we give permissions to the component
      wasi.inherit_stderr().inherit_stdout();
    }),
  );

  // Finally this returns the actual callable bindings for the component
  let bindings = component.instantiate_async(&mut store).await?;
  let result = bindings.call_hello_world(&mut store).await?;

  Ok(())
}
