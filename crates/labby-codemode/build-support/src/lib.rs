use anyhow::Result;
use sha2::{Digest, Sha256};
use wasmtime::{Engine, Linker, Store};
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wizer::Wizer;

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn preinitialize_javy_plugin(bytes: &[u8]) -> Result<Vec<u8>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let engine = Engine::default();
        let mut builder = WasiCtxBuilder::new();
        deterministic_wasi_ctx::add_determinism_to_wasi_ctx_builder(&mut builder);
        let wasi = builder.build_p1();
        let mut store = Store::new(&engine, wasi);
        Wizer::new()
            .init_func("initialize-runtime")
            .keep_init_func(true)
            .run(&mut store, bytes, async |store, module| {
                let engine = store.engine();
                let mut linker = Linker::new(engine);
                wasmtime_wasi::p1::add_to_linker_async(&mut linker, |cx| cx)?;
                linker.define_unknown_imports_as_traps(module)?;
                linker.instantiate_async(store, module).await
            })
            .await
            .map_err(Into::into)
    })
}
