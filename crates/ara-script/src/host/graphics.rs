//! Graphics preferences are host presentation state, never reflected gameplay data.
use ara_graphics_settings::{Request, Service};
use core::ffi::c_void;
use serde_json::json;

pub(super) unsafe extern "C" fn host_graphics_revision(context: *mut c_void) -> u64 {
    // SAFETY: generated callback receives its retained HostCtx.
    let ctx = unsafe { super::ctx::ctx(context) };
    let Some(scope) = ctx.current() else {
        return 0;
    };
    // SAFETY: current checked the owning exclusive scope and thread.
    unsafe { scope.world.get_resource::<Service>() }.map_or(0, |service| service.state().revision)
}

pub(super) unsafe extern "C" fn host_graphics(
    context: *mut c_void,
    request: *const u8,
    len: u32,
    out: *mut u8,
    cap: u32,
) -> i32 {
    const CAPACITY: u32 = 512 * 1024;
    // Check before dispatch: managed callers must never retry an already executed mutation.
    if cap < CAPACITY || out.is_null() {
        return -(CAPACITY as i32);
    }
    // SAFETY: the generated host ABI passes a live context and readable request bytes.
    let (ctx, input) = unsafe {
        (
            super::ctx::ctx(context),
            super::ctx::bytes(request, len.min(65537)),
        )
    };
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = (|| {
            let scope = ctx
                .current()
                .ok_or("graphics settings require an exclusive script callback")?;
            if len > 65536 {
                return Err("graphics request exceeds 64 KiB".into());
            }
            let text = std::str::from_utf8(input).map_err(|e| e.to_string())?;
            let request: Request =
                serde_json::from_value(ara_json::parse(text).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            // SAFETY: current verifies the owning thread of the exclusive host scope.
            let mut service = unsafe { scope.world.get_resource_mut::<Service>() }
                .ok_or("this host has no graphics settings service")?;
            service.request(request).map_err(|d| {
                ctx.report(d.clone());
                d.to_string()
            })
        })();
        match result {
            Ok(state) => json!({"state":state}),
            Err(error) => json!({"error":error}),
        }
    }))
    .unwrap_or_else(|_| json!({"error":"graphics settings host fault"}));
    let Ok(bytes) = serde_json::to_vec(&response) else {
        return 0;
    };
    if bytes.len() > cap as usize {
        return -(bytes.len().min(i32::MAX as usize) as i32);
    }
    // SAFETY: the caller supplied cap writable bytes; length was checked above.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
    }
    bytes.len() as i32
}
