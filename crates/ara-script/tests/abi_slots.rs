//! Foreign code can return success with a partial table; null must never become a Rust fn.
use ara_script::abi::{BehaviourCall, CommandCall, SCRIPT_API, ScriptApi, SystemCall, Tick};
use core::ffi::c_void;
use std::mem::{MaybeUninit, offset_of, size_of};

macro_rules! stub {
    ($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty) => {
        #[allow(unused_variables)]
        unsafe extern "C" fn $name($($arg: $ty),*) -> $ret {
            panic!("validation must never call foreign slots")
        }
    };
}
stub!(load(host: *mut c_void, json: *const u8, len: u32) -> i32);
stub!(boundary(host: *mut c_void, tick: *const Tick) -> u32);
stub!(pending(host: *mut c_void) -> u32);
stub!(system(host: *mut c_void, call: *const SystemCall) -> i32);
stub!(behaviours(host: *mut c_void, call: *const BehaviourCall) -> i32);
stub!(command(host: *mut c_void, call: *mut CommandCall) -> i32);
stub!(heat(host: *mut c_void, module: u32, hot: u32) -> i32);
stub!(diag(host: *mut c_void, key: u32) -> u64);
stub!(shutdown(host: *mut c_void) -> i32);

#[test]
fn every_null_dispatch_slot_is_rejected_before_constructing_the_table() {
    let valid = ScriptApi {
        load_modules: load,
        tick_boundary: boundary,
        has_pending: pending,
        run_system: system,
        run_behaviours: behaviours,
        run_command: command,
        set_heat: heat,
        request_compile: load,
        diag,
        shutdown,
    };
    let offsets = [
        offset_of!(ScriptApi, load_modules),
        offset_of!(ScriptApi, tick_boundary),
        offset_of!(ScriptApi, has_pending),
        offset_of!(ScriptApi, run_system),
        offset_of!(ScriptApi, run_behaviours),
        offset_of!(ScriptApi, run_command),
        offset_of!(ScriptApi, set_heat),
        offset_of!(ScriptApi, request_compile),
        offset_of!(ScriptApi, diag),
        offset_of!(ScriptApi, shutdown),
    ];
    assert_eq!(offsets.len(), SCRIPT_API.fns.len());
    for (offset, function) in offsets.into_iter().zip(SCRIPT_API.fns) {
        let mut raw = MaybeUninit::new(valid);
        // SAFETY: the output buffer is fully initialized. Zeroing one pointer-sized
        // field models a foreign initializer that leaves that slot null. Never form
        // a ScriptApi value from these bytes until read_checked accepts them.
        unsafe {
            raw.as_mut_ptr()
                .cast::<u8>()
                .add(offset)
                .write_bytes(0, size_of::<usize>());
            assert_eq!(
                ScriptApi::read_checked(raw.as_ptr()).expect_err("null slot"),
                format!("ScriptApi.{}", function.name),
            );
        }
    }
    // SAFETY: valid is a fully initialized table of correctly typed function pointers.
    assert!(unsafe { ScriptApi::read_checked(&valid) }.is_ok());
}
