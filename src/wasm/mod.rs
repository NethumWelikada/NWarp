use crate::config::Config;
use std::fs;
use std::io;
use wasmi::{Engine, Linker, Memory, Module, Store};

/// A single configured WASM route: a path prefix mapped to a
/// pre-compiled WASM module. Compilation happens once at startup
/// (`Module::new` does the parsing/validation work); each request gets
/// a fresh `Store`/instance for isolation - see `invoke` below.
pub struct WasmRoute {
    pub prefix: String,
    engine: Engine,
    module: Module,
}

/// The full set of configured WASM routes, built once at startup from
/// `wasm_route` lines in nwarp.conf. A module that fails to load or
/// compile is logged and skipped rather than crashing the server.
pub struct WasmTable {
    routes: Vec<WasmRoute>,
}

impl WasmTable {
    pub fn from_config(cfg: &Config) -> WasmTable {
        let mut routes = Vec::new();
        for (prefix, path) in &cfg.wasm_routes {
            match load_module(path) {
                Ok((engine, module)) => routes.push(WasmRoute {
                    prefix: prefix.clone(),
                    engine,
                    module,
                }),
                Err(e) => {
                    eprintln!(
                        "[nwarp] warning: failed to load WASM module '{}' for route '{}': {}",
                        path, prefix, e
                    );
                }
            }
        }
        WasmTable { routes }
    }

    pub fn match_route(&self, path: &str) -> Option<&WasmRoute> {
        self.routes
            .iter()
            .filter(|r| path.starts_with(r.prefix.as_str()))
            .max_by_key(|r| r.prefix.len())
    }

    /// Reserved for future use (e.g. a status/health endpoint listing
    /// active WASM routes) - not currently called, kept for parity
    /// with ProxyTable's has_routes.
    #[allow(dead_code)]
    pub fn has_routes(&self) -> bool {
        !self.routes.is_empty()
    }
}

fn load_module(path: &str) -> io::Result<(Engine, Module)> {
    let bytes = fs::read(path)
        .map_err(|e| io::Error::new(e.kind(), format!("could not read '{}': {}", path, e)))?;
    let engine = Engine::default();
    let module = Module::new(&engine, &bytes[..])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("invalid WASM module: {}", e)))?;
    Ok((engine, module))
}

/// Invokes a WASM route's `handle` export for a single request.
///
/// ## NWarp WASM handler ABI (Phase 6)
///
/// A compatible module must export:
/// - `memory` - linear memory the host reads/writes into
/// - `alloc(size: i32) -> i32` - bump-allocate `size` bytes, return a pointer
/// - `handle(method_ptr: i32, method_len: i32, path_ptr: i32, path_len: i32) -> i64`
///   - packs `(response_ptr << 32) | response_len` into the return value
///
/// The response bytes at `response_ptr` must be laid out as: the first
/// 2 bytes are the HTTP status code (u16, little-endian), followed by
/// the response body. This is intentionally minimal for Phase 6 - see
/// docs/ARCHITECTURE.md for documented limitations (fixed content
/// type, no host-provided logging/fetch imports, no request body,
/// fresh instance per request rather than a pooled/reused one).
///
/// A fresh `Store` (and therefore a fresh linear memory and fresh
/// global state) is created per request. This is the safe default for
/// a first working version: one request's module state can never leak
/// into another's. It costs a small amount of per-request instantiation
/// overhead; pooling/reusing instances across requests is a natural
/// follow-up optimization, not implemented here.
pub fn invoke(route: &WasmRoute, method: &str, path: &str) -> Result<(u16, Vec<u8>), String> {
    let mut store = Store::new(&route.engine, ());
    let linker = Linker::new(&route.engine);

    let instance = linker
        .instantiate(&mut store, &route.module)
        .map_err(|e| format!("instantiation failed: {}", e))?
        .start(&mut store)
        .map_err(|e| format!("module start failed: {}", e))?;

    let memory: Memory = instance
        .get_memory(&store, "memory")
        .ok_or_else(|| "module does not export 'memory'".to_string())?;

    let alloc = instance
        .get_typed_func::<i32, i32>(&store, "alloc")
        .map_err(|e| format!("module does not export a compatible 'alloc': {}", e))?;

    let handle = instance
        .get_typed_func::<(i32, i32, i32, i32), i64>(&store, "handle")
        .map_err(|e| format!("module does not export a compatible 'handle': {}", e))?;

    let method_ptr = alloc
        .call(&mut store, method.len() as i32)
        .map_err(|e| format!("alloc call failed: {}", e))?;
    memory
        .write(&mut store, method_ptr as usize, method.as_bytes())
        .map_err(|e| format!("writing method into guest memory failed: {}", e))?;

    let path_ptr = alloc
        .call(&mut store, path.len() as i32)
        .map_err(|e| format!("alloc call failed: {}", e))?;
    memory
        .write(&mut store, path_ptr as usize, path.as_bytes())
        .map_err(|e| format!("writing path into guest memory failed: {}", e))?;

    let packed = handle
        .call(&mut store, (method_ptr, method.len() as i32, path_ptr, path.len() as i32))
        .map_err(|e| format!("handle call failed: {}", e))?;

    let response_ptr = (packed >> 32) as u32 as usize;
    let response_len = (packed & 0xFFFF_FFFF) as u32 as usize;

    if response_len < 2 {
        return Err("handler returned a response shorter than the 2-byte status header".to_string());
    }

    let mut buf = vec![0u8; response_len];
    memory
        .read(&store, response_ptr, &mut buf)
        .map_err(|e| format!("reading response from guest memory failed: {}", e))?;

    let status = u16::from_le_bytes([buf[0], buf[1]]);
    let body = buf[2..].to_vec();

    Ok((status, body))
}

