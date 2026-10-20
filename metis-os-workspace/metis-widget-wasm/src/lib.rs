//! Sandboxed Wasmtime host for Metis desktop widget packs (Extension API v2 / M7).
//!
//! See [`docs/decisions/widget-wasm.md`](../../docs/decisions/widget-wasm.md) and
//! [`docs/WIDGET_WASM_ABI.md`](../../docs/WIDGET_WASM_ABI.md).

#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use metis_config::{
    WIDGET_EXT_MAX_COPY, WIDGET_EXT_MAX_STRING, WIDGET_EXT_MAX_WASM_BYTES, is_safe_launch_exec,
    is_safe_launch_id, is_safe_node_id, is_safe_open_uri, widget_wasm_enabled,
};
use thiserror::Error;
use wasmtime::{Caller, Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

/// Fuel consumed per guest call (init / tick / on_event).
pub const WASM_FUEL_PER_CALL: u64 = 5_000_000;
/// Max linear memory (16 MiB).
pub const WASM_MEMORY_MAX_BYTES: usize = 16 * 1024 * 1024;
/// Max simultaneous instances (advisory; host enforces).
pub const WASM_MAX_INSTANCES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum TickReason {
    Timer = 0,
    Helper = 1,
    Event = 2,
}

/// Side effects the guest may request; applied by the shell after the call.
#[derive(Debug, Clone, Default)]
pub struct HostEffects {
    pub set_text: Vec<(String, String)>,
    pub set_visible: Vec<(String, bool)>,
    pub request_helper_refresh: bool,
    pub open_uri: Option<String>,
    pub launch_id: Option<String>,
    pub launch_exec: Option<String>,
    pub copy_text: Option<String>,
}

/// JSON snapshots supplied by the host for each call.
#[derive(Debug, Clone, Default)]
pub struct HostSnapshots {
    pub settings_json: String,
    pub host_json: String,
    pub helper_json: String,
}

struct GuestState {
    limits: StoreLimits,
    snapshots: HostSnapshots,
    effects: HostEffects,
}

#[derive(Debug, Error)]
pub enum WasmHostError {
    #[error("widget WASM disabled (METIS_WIDGET_WASM=0)")]
    Disabled,
    #[error("module too large ({0} bytes; max {WIDGET_EXT_MAX_WASM_BYTES})")]
    ModuleTooLarge(u64),
    #[error("wasmtime: {0}")]
    Engine(String),
    #[error("missing export: {0}")]
    MissingExport(&'static str),
}

/// One instantiated guest module bound to host snapshots/effects.
pub struct WidgetWasmInstance {
    store: Store<GuestState>,
    instance: wasmtime::Instance,
}

impl WidgetWasmInstance {
    /// Compile and instantiate `module_bytes` with the Metis host imports.
    pub fn new(module_bytes: &[u8], snapshots: HostSnapshots) -> Result<Self, WasmHostError> {
        if !widget_wasm_enabled() {
            return Err(WasmHostError::Disabled);
        }
        if module_bytes.len() as u64 > WIDGET_EXT_MAX_WASM_BYTES {
            return Err(WasmHostError::ModuleTooLarge(module_bytes.len() as u64));
        }

        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        config.max_wasm_stack(512 * 1024);
        let engine = Engine::new(&config).map_err(|e| WasmHostError::Engine(e.to_string()))?;

        let limits = StoreLimitsBuilder::new()
            .memory_size(WASM_MEMORY_MAX_BYTES)
            .memories(1)
            .tables(4)
            .instances(1)
            .build();

        let state = GuestState {
            limits,
            snapshots,
            effects: HostEffects::default(),
        };
        let mut store = Store::new(&engine, state);
        store.limiter(|s| &mut s.limits);
        store
            .set_fuel(WASM_FUEL_PER_CALL)
            .map_err(|e| WasmHostError::Engine(e.to_string()))?;
        store.set_epoch_deadline(1);

        let mut linker = Linker::new(&engine);
        link_host(&mut linker).map_err(|e| WasmHostError::Engine(e.to_string()))?;

        let module =
            Module::new(&engine, module_bytes).map_err(|e| WasmHostError::Engine(e.to_string()))?;
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| WasmHostError::Engine(e.to_string()))?;

        // Require memory + metis_init + metis_tick.
        let _ = instance
            .get_memory(&mut store, "memory")
            .ok_or(WasmHostError::MissingExport("memory"))?;
        let _ = instance
            .get_typed_func::<(), i32>(&mut store, "metis_init")
            .map_err(|_| WasmHostError::MissingExport("metis_init"))?;
        let _ = instance
            .get_typed_func::<(i32,), i32>(&mut store, "metis_tick")
            .map_err(|_| WasmHostError::MissingExport("metis_tick"))?;

        // Keep Engine alive via Store (Store owns an Engine clone/handle).
        let _ = engine;
        Ok(Self { store, instance })
    }

    pub fn update_snapshots(&mut self, snapshots: HostSnapshots) {
        self.store.data_mut().snapshots = snapshots;
    }

    pub fn take_effects(&mut self) -> HostEffects {
        std::mem::take(&mut self.store.data_mut().effects)
    }

    pub fn call_init(&mut self) -> Result<i32, WasmHostError> {
        self.with_budget(|store, instance| {
            let f = instance
                .get_typed_func::<(), i32>(&mut *store, "metis_init")
                .map_err(|_| WasmHostError::MissingExport("metis_init"))?;
            f.call(&mut *store, ())
                .map_err(|e| WasmHostError::Engine(e.to_string()))
        })
    }

    pub fn call_tick(&mut self, reason: TickReason) -> Result<i32, WasmHostError> {
        self.with_budget(|store, instance| {
            let f = instance
                .get_typed_func::<(i32,), i32>(&mut *store, "metis_tick")
                .map_err(|_| WasmHostError::MissingExport("metis_tick"))?;
            f.call(&mut *store, (reason as i32,))
                .map_err(|e| WasmHostError::Engine(e.to_string()))
        })
    }

    pub fn call_on_event(&mut self, event: &str) -> Result<i32, WasmHostError> {
        if !is_safe_node_id(event) {
            return Err(WasmHostError::Engine("event name rejected".into()));
        }
        self.with_budget(|store, instance| {
            let Some(memory) = instance.get_memory(&mut *store, "memory") else {
                return Err(WasmHostError::MissingExport("memory"));
            };
            // Write event into a scratch region at the end of the first page if present.
            let bytes = event.as_bytes();
            let ptr = 0xE000i32;
            let data = memory.data_mut(&mut *store);
            if ptr as usize + bytes.len() > data.len() {
                return Err(WasmHostError::Engine(
                    "guest memory too small for event".into(),
                ));
            }
            data[ptr as usize..ptr as usize + bytes.len()].copy_from_slice(bytes);
            let f = match instance.get_typed_func::<(i32, i32), i32>(&mut *store, "metis_on_event")
            {
                Ok(f) => f,
                Err(_) => return Ok(0), // optional export
            };
            f.call(&mut *store, (ptr, bytes.len() as i32))
                .map_err(|e| WasmHostError::Engine(e.to_string()))
        })
    }

    fn with_budget<R>(
        &mut self,
        f: impl FnOnce(&mut Store<GuestState>, &wasmtime::Instance) -> Result<R, WasmHostError>,
    ) -> Result<R, WasmHostError> {
        // Fuel is the primary limiter; epoch deadline stays armed for a future
        // background ticker (`Engine::increment_epoch`).
        self.store
            .set_fuel(WASM_FUEL_PER_CALL)
            .map_err(|e| WasmHostError::Engine(e.to_string()))?;
        f(&mut self.store, &self.instance)
    }
}

fn link_host(linker: &mut Linker<GuestState>) -> Result<(), wasmtime::Error> {
    linker.func_wrap(
        "metis",
        "get_settings_json",
        |mut caller: Caller<'_, GuestState>, buf: i32, cap: i32| -> i32 {
            write_json_to_guest(&mut caller, buf, cap, |s| s.snapshots.settings_json.clone())
        },
    )?;
    linker.func_wrap(
        "metis",
        "get_host_json",
        |mut caller: Caller<'_, GuestState>, buf: i32, cap: i32| -> i32 {
            write_json_to_guest(&mut caller, buf, cap, |s| s.snapshots.host_json.clone())
        },
    )?;
    linker.func_wrap(
        "metis",
        "get_helper_json",
        |mut caller: Caller<'_, GuestState>, buf: i32, cap: i32| -> i32 {
            write_json_to_guest(&mut caller, buf, cap, |s| s.snapshots.helper_json.clone())
        },
    )?;
    linker.func_wrap(
        "metis",
        "set_text",
        |mut caller: Caller<'_, GuestState>,
         id_ptr: i32,
         id_len: i32,
         text_ptr: i32,
         text_len: i32|
         -> i32 {
            let id = read_guest_str(&mut caller, id_ptr, id_len);
            let text = read_guest_str(&mut caller, text_ptr, text_len);
            let (Ok(id), Ok(text)) = (id, text) else {
                return -1;
            };
            if !is_safe_node_id(&id) || text.len() > WIDGET_EXT_MAX_STRING {
                return -1;
            }
            caller.data_mut().effects.set_text.push((id, text));
            0
        },
    )?;
    linker.func_wrap(
        "metis",
        "set_visible",
        |mut caller: Caller<'_, GuestState>, id_ptr: i32, id_len: i32, visible: i32| -> i32 {
            let Ok(id) = read_guest_str(&mut caller, id_ptr, id_len) else {
                return -1;
            };
            if !is_safe_node_id(&id) {
                return -1;
            }
            caller
                .data_mut()
                .effects
                .set_visible
                .push((id, visible != 0));
            0
        },
    )?;
    linker.func_wrap(
        "metis",
        "request_helper_refresh",
        |mut caller: Caller<'_, GuestState>| -> i32 {
            caller.data_mut().effects.request_helper_refresh = true;
            0
        },
    )?;
    linker.func_wrap(
        "metis",
        "open_uri",
        |mut caller: Caller<'_, GuestState>, ptr: i32, len: i32| -> i32 {
            let Ok(uri) = read_guest_str(&mut caller, ptr, len) else {
                return -1;
            };
            if !is_safe_open_uri(&uri) {
                return -1;
            }
            caller.data_mut().effects.open_uri = Some(uri);
            0
        },
    )?;
    linker.func_wrap(
        "metis",
        "launch",
        |mut caller: Caller<'_, GuestState>,
         id_ptr: i32,
         id_len: i32,
         exec_ptr: i32,
         exec_len: i32|
         -> i32 {
            let id = read_guest_str(&mut caller, id_ptr, id_len).unwrap_or_default();
            let exec = read_guest_str(&mut caller, exec_ptr, exec_len).unwrap_or_default();
            if !id.is_empty() {
                if !is_safe_launch_id(&id) {
                    return -1;
                }
                caller.data_mut().effects.launch_id = Some(id);
                return 0;
            }
            if !exec.is_empty() {
                if !is_safe_launch_exec(&exec) {
                    return -1;
                }
                caller.data_mut().effects.launch_exec = Some(exec);
                return 0;
            }
            -1
        },
    )?;
    linker.func_wrap(
        "metis",
        "copy_text",
        |mut caller: Caller<'_, GuestState>, ptr: i32, len: i32| -> i32 {
            let Ok(text) = read_guest_str(&mut caller, ptr, len) else {
                return -1;
            };
            if text.len() > WIDGET_EXT_MAX_COPY {
                return -1;
            }
            caller.data_mut().effects.copy_text = Some(text);
            0
        },
    )?;
    Ok(())
}

fn write_json_to_guest(
    caller: &mut Caller<'_, GuestState>,
    buf: i32,
    cap: i32,
    get: impl FnOnce(&GuestState) -> String,
) -> i32 {
    if buf < 0 || cap <= 0 {
        return -1;
    }
    let json = get(caller.data());
    let bytes = json.as_bytes();
    if bytes.len() > cap as usize {
        return -1;
    }
    let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
        return -1;
    };
    let data = memory.data_mut(&mut *caller);
    let start = buf as usize;
    let end = start + bytes.len();
    if end > data.len() {
        return -1;
    }
    data[start..end].copy_from_slice(bytes);
    bytes.len() as i32
}

fn read_guest_str(caller: &mut Caller<'_, GuestState>, ptr: i32, len: i32) -> Result<String, ()> {
    if ptr < 0 || len < 0 {
        return Err(());
    }
    if len as usize > WIDGET_EXT_MAX_STRING {
        return Err(());
    }
    let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
        return Err(());
    };
    let data = memory.data(&*caller);
    let start = ptr as usize;
    let end = start.saturating_add(len as usize);
    if end > data.len() {
        return Err(());
    }
    std::str::from_utf8(&data[start..end])
        .map(|s| s.to_string())
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_module() -> Vec<u8> {
        // Minimal guest: init/tick ok; on tick set_text("status", "ok"); on_event open_uri.
        let wat = r#"
(module
  (import "metis" "get_host_json" (func $get_host_json (param i32 i32) (result i32)))
  (import "metis" "set_text" (func $set_text (param i32 i32 i32 i32) (result i32)))
  (import "metis" "open_uri" (func $open_uri (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "status")
  (data (i32.const 16) "ok")
  (data (i32.const 32) "https://example.com")
  (func (export "metis_init") (result i32) (i32.const 0))
  (func (export "metis_tick") (param i32) (result i32)
    (call $set_text (i32.const 0) (i32.const 6) (i32.const 16) (i32.const 2))
  )
  (func (export "metis_on_event") (param i32 i32) (result i32)
    (call $open_uri (i32.const 32) (i32.const 19))
  )
)
"#;
        wat::parse_str(wat).expect("wat")
    }

    #[test]
    fn instantiates_and_set_text_on_tick() {
        let mut inst = WidgetWasmInstance::new(
            &sample_module(),
            HostSnapshots {
                host_json: r#"{"time":"12:00"}"#.into(),
                settings_json: "{}".into(),
                helper_json: "{}".into(),
            },
        )
        .expect("instance");
        assert_eq!(inst.call_init().expect("init"), 0);
        assert_eq!(inst.call_tick(TickReason::Timer).expect("tick"), 0);
        let effects = inst.take_effects();
        assert_eq!(effects.set_text, vec![("status".into(), "ok".into())]);
    }

    #[test]
    fn on_event_open_uri_hardened() {
        let mut inst =
            WidgetWasmInstance::new(&sample_module(), HostSnapshots::default()).expect("instance");
        assert_eq!(inst.call_on_event("open").expect("event"), 0);
        let effects = inst.take_effects();
        assert_eq!(effects.open_uri.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn rejects_oversized_module() {
        let big = vec![0u8; (WIDGET_EXT_MAX_WASM_BYTES as usize) + 8];
        match WidgetWasmInstance::new(&big, HostSnapshots::default()) {
            Err(WasmHostError::ModuleTooLarge(_)) => {}
            Ok(_) => panic!("expected ModuleTooLarge"),
            Err(other) => panic!("unexpected {other}"),
        }
    }
}
