//! The host side of `HostApi`: the context C# reaches through `HostInit::ctx`, and the
//! `extern "C"` functions it calls.
//!
//! World access from C# (`World.Get<T>()`, registration) is only possible inside an
//! exclusive [`Scope`], which the host publishes for the duration of a behaviour phase,
//! a command or a tick boundary, on the thread that runs it.

use core::ffi::c_void;
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::thread::ThreadId;
use std::time::Instant;

use ara_diag::Diagnostic;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;

use crate::abi::{COMPONENT_HAS, COMPONENT_READ, COMPONENT_WRITE, HostApi, ReloadReport};
use crate::codes;
use crate::ecs::cmdbuf::{CmdBuf, Op};
use crate::host::registry::Registry;

/// What C# points at for a tag (zero-sized) component, whose C# struct is 1 byte:
/// bevy's pointer to zero-sized storage is dangling. The contents mean nothing.
static TAG_BYTES: AtomicU64 = AtomicU64::new(0);

/// Receives C# log lines: level (0 info, 1 warning, 2 error) and text.
pub type LogSink = Box<dyn Fn(u32, &str) + Send + Sync>;

/// What a command opened for writing, for undo and rejection: first-seen bytes per
/// `(entity, script component)` and per script resource.
#[derive(Default)]
pub(crate) struct Journal {
    pub seen: BTreeSet<(u64, u32)>,
    pub before: Vec<(u64, u32, Vec<u8>)>,
    pub resources: Vec<(u32, Vec<u8>)>,
}

/// Exclusive access published to C# callbacks.
pub(crate) struct Scope<'w> {
    pub world: UnsafeWorldCell<'w>,
    pub registry: *mut Registry,
    pub journal: Option<*mut Journal>,
    pub thread: ThreadId,
    /// `register_modules` is allowed (load and tick boundary only).
    pub register: bool,
}

/// State C# reaches through `HostInit::ctx`. Leaked by `ScriptHost::start`: the .NET
/// runtime outlives the host (it cannot be unloaded), so a late callback from a C#
/// thread still finds a live context.
pub(crate) struct HostCtx {
    pub start: Instant,
    pub diagnostics: Mutex<Vec<Diagnostic>>,
    pub reloads: Mutex<Vec<ReloadReport>>,
    pub log: LogSink,
    pub scope: AtomicPtr<Scope<'static>>,
    pub boost: crate::boost::Boost,
}

impl HostCtx {
    /// Stable host identity used by both directions of the ABI.
    pub fn handle(&self) -> *mut c_void {
        std::ptr::from_ref(self).cast_mut().cast()
    }

    pub fn new(log: LogSink) -> Self {
        HostCtx {
            start: Instant::now(),
            diagnostics: Mutex::new(Vec::new()),
            reloads: Mutex::new(Vec::new()),
            log,
            scope: AtomicPtr::new(std::ptr::null_mut()),
            boost: crate::boost::Boost::default(),
        }
    }

    /// Records a diagnostic (any thread).
    pub fn report(&self, d: Diagnostic) {
        if let Ok(mut v) = self.diagnostics.lock() {
            v.push(d);
        }
    }

    /// How many diagnostics are queued (a mark for [`HostCtx::take_since`]).
    pub fn diagnostic_count(&self) -> usize {
        self.diagnostics.lock().map_or(0, |d| d.len())
    }

    /// Removes and returns the diagnostics with `code` queued after mark `start`; the
    /// rest stay queued.
    pub fn take_since(&self, start: usize, code: ara_diag::Code) -> Vec<Diagnostic> {
        let Ok(mut all) = self.diagnostics.lock() else {
            return Vec::new();
        };
        let start = start.min(all.len());
        let (taken, kept): (Vec<_>, Vec<_>) = all.drain(start..).partition(|d| d.code == code);
        all.extend(kept);
        taken
    }

    /// Runs `f` with `scope` published to C# callbacks on this thread.
    pub fn with_scope<R>(&self, scope: &mut Scope<'_>, f: impl FnOnce() -> R) -> R {
        struct Reset<'a>(&'a AtomicPtr<Scope<'static>>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(std::ptr::null_mut(), Ordering::Release);
            }
        }
        self.scope.store(
            std::ptr::from_mut(scope).cast::<Scope<'static>>(),
            Ordering::Release,
        );
        let _reset = Reset(&self.scope);
        f()
    }

    /// The published scope, if this thread owns it.
    pub(super) fn current(&self) -> Option<&Scope<'static>> {
        let p = self.scope.load(Ordering::Acquire);
        // SAFETY: a non-null pointer is a live `Scope` published by `with_scope`, which
        // outlives the callback; only its owning thread (checked below) dereferences it.
        let scope = unsafe { p.as_ref() }?;
        (scope.thread == std::thread::current().id()).then_some(scope)
    }

    /// The `HostApi` table pointing at this module's functions.
    pub fn api() -> HostApi {
        HostApi {
            log: host_log,
            now_ns: host_now_ns,
            diagnostic: host_diagnostic,
            report_reload: host_report_reload,
            register_modules: host_register_modules,
            component_ptr: host_component_ptr,
            resource_ptr: host_resource_ptr,
            entity_alive: host_entity_alive,
            reserve_entity: host_reserve_entity,
            cmd_push: host_cmd_push,
            compile_state: host_compile_state,
            component_json: host_component_json,
            query_entities: host_query_entities,
            scene_entities: super::scene::scene_entities,
            input_axis: host_input_axis,
            input_player: host_input_player,
            navigate: host_navigate,
            character_stop: host_character_stop,
            animation_bool: host_animation_bool,
        }
    }
}

/// # Safety
/// `ctx` must be the `HostCtx` pointer from `HostInit`, which outlives the runtime.
pub(super) unsafe fn ctx<'a>(ctx: *mut c_void) -> &'a HostCtx {
    // SAFETY: guaranteed by the caller.
    unsafe { &*ctx.cast::<HostCtx>() }
}

/// # Safety
/// `ptr`/`len` must describe readable bytes (or `len == 0`).
pub(super) unsafe fn bytes<'a>(ptr: *const u8, len: u32) -> &'a [u8] {
    if len == 0 || ptr.is_null() {
        return &[];
    }
    // SAFETY: guaranteed by the caller.
    unsafe { std::slice::from_raw_parts(ptr, len as usize) }
}

unsafe extern "C" fn host_log(c: *mut c_void, level: u32, msg: *const u8, len: u32) {
    // SAFETY: C# passes back the ctx it was given and a valid UTF-8 span.
    let (c, msg) = unsafe { (ctx(c), bytes(msg, len)) };
    (c.log)(level, &String::from_utf8_lossy(msg));
}

unsafe extern "C" fn host_now_ns(c: *mut c_void) -> u64 {
    // SAFETY: C# passes back the ctx it was given.
    let c = unsafe { ctx(c) };
    u64::try_from(c.start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

unsafe extern "C" fn host_diagnostic(c: *mut c_void, json: *const u8, len: u32) {
    // SAFETY: C# passes back the ctx it was given and a valid UTF-8 span.
    let (c, json) = unsafe { (ctx(c), bytes(json, len)) };
    let d = serde_json::from_slice::<Diagnostic>(json).unwrap_or_else(|_| {
        Diagnostic::error(
            codes::RUNTIME_START,
            String::from_utf8_lossy(json).into_owned(),
        )
    });
    c.report(d);
}

unsafe extern "C" fn host_report_reload(c: *mut c_void, report: *const ReloadReport) {
    // SAFETY: C# passes back the ctx it was given and a pointer to a live report.
    let (c, r) = unsafe { (ctx(c), *report) };
    if let Ok(mut v) = c.reloads.lock() {
        v.push(r);
    }
}

unsafe extern "C" fn host_register_modules(
    c: *mut c_void,
    json: *const u8,
    len: u32,
    out: *mut u8,
    cap: u32,
) -> i32 {
    // SAFETY: C# passes back the ctx it was given and valid spans.
    let (c, json) = unsafe { (ctx(c), bytes(json, len)) };
    let Some(scope) = c.current().filter(|s| s.register) else {
        c.report(Diagnostic::error(
            codes::LOAD_FAILED,
            "register_modules called outside load or a tick boundary",
        ));
        return write_response(br#"{"ok":false}"#, out, cap);
    };
    // SAFETY: the scope's world cell has exclusive access (made from `&mut World`) and
    // its registry pointer is live for the scope; nothing else uses either meanwhile.
    let (world, registry) = unsafe { (scope.world.world_mut(), &mut *scope.registry) };
    let response = match registry.register_modules(world, json) {
        Ok(v) => serde_json::to_vec(&v).unwrap_or_default(),
        Err(problems) => {
            for p in problems {
                c.report(p);
            }
            br#"{"ok":false}"#.to_vec()
        }
    };
    write_response(&response, out, cap)
}

fn write_response(bytes: &[u8], out: *mut u8, cap: u32) -> i32 {
    let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
    if bytes.len() > cap as usize {
        return -len;
    }
    // SAFETY: C# passed a writable buffer of `cap` bytes, and `bytes.len() <= cap`.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    len
}

unsafe extern "C" fn host_component_ptr(
    c: *mut c_void,
    entity: u64,
    component: u32,
    write: u32,
) -> *mut u8 {
    // SAFETY: C# passes back the ctx it was given.
    let c = unsafe { ctx(c) };
    let Some(scope) = c.current() else {
        return std::ptr::null_mut();
    };
    // SAFETY: the registry is live for the scope.
    let registry = unsafe { &*scope.registry };
    let (Some(e), Some(comp)) = (
        Entity::try_from_bits(entity),
        registry.components.get(component as usize),
    ) else {
        return std::ptr::null_mut();
    };
    let Ok(cell) = scope.world.get_entity(e) else {
        return std::ptr::null_mut();
    };
    if comp.json {
        return std::ptr::null_mut();
    }
    if comp.layout.size == 0 {
        return if cell.contains_id(comp.id) {
            TAG_BYTES.as_ptr().cast()
        } else {
            std::ptr::null_mut()
        };
    }
    if write == 0 {
        // SAFETY: exclusive phase; the pointer is used only until the callback returns.
        return unsafe { cell.get_by_id(comp.id) }
            .map_or(std::ptr::null_mut(), |p| p.as_ptr().cast_mut());
    }
    // SAFETY: exclusive phase: no Rust reference to this component exists while C# runs.
    let Ok(m) = (unsafe { cell.get_mut_by_id(comp.id) }) else {
        return std::ptr::null_mut();
    };
    let ptr = m.into_inner().as_ptr();
    if let Some(journal) = scope.journal {
        // SAFETY: the journal is live for the scope and only touched on this thread.
        let journal = unsafe { &mut *journal };
        if journal.seen.insert((entity, component)) {
            // SAFETY: `ptr` points at `layout.size` initialized bytes of the component.
            let before = unsafe { std::slice::from_raw_parts(ptr, comp.layout.size) }.to_vec();
            journal.before.push((entity, component, before));
        }
    }
    ptr
}

unsafe extern "C" fn host_resource_ptr(c: *mut c_void, resource: u32, write: u32) -> *mut u8 {
    // SAFETY: C# passes back the ctx it was given.
    let c = unsafe { ctx(c) };
    let Some(scope) = c.current() else {
        return std::ptr::null_mut();
    };
    // SAFETY: the registry is live for the scope.
    let registry = unsafe { &*scope.registry };
    let Some((r, id)) = registry
        .resources
        .get(resource as usize)
        .and_then(|r| Some((r, r.id?)))
    else {
        return std::ptr::null_mut();
    };
    if write == 0 {
        // SAFETY: exclusive phase; the pointer is used only until the callback returns.
        return unsafe { scope.world.get_resource_by_id(id) }
            .map_or(std::ptr::null_mut(), |p| p.as_ptr().cast_mut());
    }
    // SAFETY: exclusive phase: no Rust reference to this resource exists while C# runs.
    let Some(m) = (unsafe { scope.world.get_resource_mut_by_id(id) }) else {
        return std::ptr::null_mut();
    };
    let ptr = m.into_inner().as_ptr();
    if let Some(journal) = scope.journal {
        // SAFETY: the journal is live for the scope and only touched on this thread.
        let journal = unsafe { &mut *journal };
        if !journal.resources.iter().any(|(i, _)| *i == resource) {
            // SAFETY: `ptr` points at `layout.size` initialized bytes of the resource.
            let before = unsafe { std::slice::from_raw_parts(ptr, r.layout.size) }.to_vec();
            journal.resources.push((resource, before));
        }
    }
    ptr
}

unsafe extern "C" fn host_entity_alive(c: *mut c_void, entity: u64) -> u32 {
    // SAFETY: C# passes back the ctx it was given.
    let c = unsafe { ctx(c) };
    let alive = c.current().is_some_and(|s| {
        Entity::try_from_bits(entity).is_some_and(|e| s.world.get_entity(e).is_ok())
    });
    u32::from(alive)
}

unsafe extern "C" fn host_reserve_entity(_c: *mut c_void, buf: *mut c_void) -> u64 {
    // SAFETY: C# passes a buffer pointer the host gave it for the current call.
    match unsafe { buf.cast::<CmdBuf>().as_ref() } {
        Some(b) => b.reserve(),
        None => crate::value::NULL_ENTITY,
    }
}

unsafe extern "C" fn host_cmd_push(
    _c: *mut c_void,
    buf: *mut c_void,
    op: u32,
    type_index: u32,
    entity: u64,
    data: *const u8,
    len: u32,
) {
    // SAFETY: C# passes a buffer pointer the host gave it for the current call, used by
    // one thread at a time, and a readable data span.
    let (buf, data) = unsafe { (buf.cast::<CmdBuf>().as_mut(), bytes(data, len)) };
    if let Some(buf) = buf {
        buf.ops.push(Op {
            op,
            type_index,
            entity,
            data: data.to_vec(),
        });
    }
}

unsafe extern "C" fn host_compile_state(c: *mut c_void, active: u32) {
    // SAFETY: C# passes back the ctx it was given.
    unsafe { ctx(c) }.boost.set(active != 0);
}

unsafe extern "C" fn host_component_json(
    c: *mut c_void,
    entity: u64,
    component: u32,
    op: u32,
    data: *const u8,
    len: u32,
    out: *mut u8,
    cap: u32,
) -> i32 {
    // SAFETY: the runtime returns its live context and borrowed spans from HostApi.
    let c = unsafe { ctx(c) };
    let Some(scope) = c.current() else {
        return 0;
    };
    let Some(entity_id) = Entity::try_from_bits(entity) else {
        return 0;
    };
    // SAFETY: this callback runs only on the exclusive scope's owning thread.
    let (world, registry) = unsafe { (scope.world.world_mut(), &*scope.registry) };
    let Some(comp) = registry.components.get(component as usize) else {
        return 0;
    };
    if op == COMPONENT_HAS {
        return i32::from(
            world
                .get_entity(entity_id)
                .is_ok_and(|e| e.contains_id(comp.id)),
        );
    }
    let Some(before) = world
        .resource::<crate::ecs::cmdbuf::ScriptCatalog>()
        .read(world, entity_id, component)
    else {
        return 0;
    };
    if op == COMPONENT_READ {
        let value = if comp.json {
            before
        } else {
            serde_json::to_vec(&crate::value::decode(
                &registry.schema,
                &comp.layout,
                &before,
            ))
            .unwrap_or_default()
        };
        return write_response(&value, out, cap);
    }
    if op != COMPONENT_WRITE {
        return 0;
    }
    // SAFETY: data/len is a readable UTF-8 JSON span for this call.
    let Ok(value) = serde_json::from_slice(unsafe { bytes(data, len) }) else {
        return 0;
    };
    let encoded = if comp.json {
        serde_json::to_vec(&value).ok()
    } else {
        crate::value::encode(&registry.schema, &comp.layout, &value, &comp.name).ok()
    };
    let Some(encoded) = encoded else {
        return 0;
    };
    if let Some(journal) = scope.journal {
        // SAFETY: the journal is exclusively borrowed for this scope.
        let journal = unsafe { &mut *journal };
        if journal.seen.insert((entity, component)) {
            journal.before.push((entity, component, before));
        }
    }
    world.resource_scope(
        |world, catalog: bevy_ecs::prelude::Mut<crate::ecs::cmdbuf::ScriptCatalog>| {
            i32::from(catalog.insert(world, entity_id, component, &encoded))
        },
    )
}

unsafe extern "C" fn host_query_entities(
    c: *mut c_void,
    component: u32,
    out: *mut u64,
    cap: u32,
) -> u32 {
    // SAFETY: the caller returns its live host context.
    let Some(scope) = (unsafe { ctx(c) }).current() else {
        return 0;
    };
    // SAFETY: registry metadata remains live and unchanged throughout this callback.
    let Some(component) = (unsafe { &*scope.registry })
        .components
        .get(component as usize)
    else {
        return 0;
    };
    let mut entities: Vec<_> = scope
        .world
        .archetypes()
        .iter()
        .filter(|a| a.contains(component.id))
        .flat_map(|a| a.entities().iter().map(|e| e.id().to_bits()))
        .collect();
    entities.sort_unstable();
    if entities.len() <= cap as usize && !out.is_null() {
        // SAFETY: C# supplies cap writable u64s, disjoint from engine storage.
        unsafe {
            std::ptr::copy_nonoverlapping(entities.as_ptr(), out, entities.len());
        }
    }
    entities.len() as u32
}

unsafe extern "C" fn host_input_axis(
    c: *mut c_void,
    player: u64,
    name: *const u8,
    len: u32,
    axis: u32,
) -> f32 {
    // SAFETY: the caller supplies its live context and a readable UTF-8 span.
    let (context, name) = unsafe { (ctx(c), bytes(name, len)) };
    let Some(scope) = context.current() else {
        return 0.0;
    };
    let Ok(name) = std::str::from_utf8(name) else {
        return 0.0;
    };
    // SAFETY: ScriptInputs is immutable for the entire exclusive C# phase and is
    // not exposed as a writable C# component/resource.
    let inputs = unsafe { scope.world.get_resource::<super::ScriptInputs>() };
    inputs
        .and_then(|i| i.actions.get(&(player, name.to_owned())))
        .and_then(|v| v.get(axis as usize))
        .copied()
        .unwrap_or(0.0)
}

unsafe extern "C" fn host_input_player(c: *mut c_void, entity: u64) -> u64 {
    // SAFETY: the caller returns its live context; the input snapshot is immutable during callbacks.
    let Some(scope) = (unsafe { ctx(c) }).current() else {
        return 0;
    };
    // SAFETY: the snapshot is not accessible for mutation from C#.
    unsafe { scope.world.get_resource::<super::ScriptInputs>() }
        .and_then(|i| i.players.get(&entity))
        .copied()
        .unwrap_or(0)
}

unsafe extern "C" fn host_navigate(
    c: *mut c_void,
    entity: u64,
    x: f32,
    y: f32,
    z: f32,
    enabled: u32,
) {
    if ![x, y, z].iter().all(|v| v.is_finite()) {
        return;
    }
    // SAFETY: the caller returns its live context, on the exclusive callback thread.
    let Some(scope) = (unsafe { ctx(c) }).current() else {
        return;
    };
    // SAFETY: only this exclusive callback can access this unbound resource.
    if let Some(mut requests) = unsafe { scope.world.get_resource_mut::<super::ScriptNavigation>() }
    {
        requests
            .0
            .push((entity, (enabled != 0).then_some([x, y, z])));
    }
}

unsafe extern "C" fn host_character_stop(c: *mut c_void, entity: u64) {
    // SAFETY: the caller returns its live host context on the exclusive callback thread.
    let Some(scope) = (unsafe { ctx(c) }).current() else {
        return;
    };
    // SAFETY: this unbound resource is accessible only by the exclusive callback.
    if let Some(mut controls) = unsafe { scope.world.get_resource_mut::<super::ScriptControls>() } {
        controls.stopped.push(entity);
    }
}

unsafe extern "C" fn host_animation_bool(
    c: *mut c_void,
    entity: u64,
    name: *const u8,
    len: u32,
    value: u32,
) {
    if len > 128 {
        return;
    }
    // SAFETY: the caller supplies its live context and a readable UTF-8 span.
    let (context, name) = unsafe { (ctx(c), bytes(name, len)) };
    let Some(scope) = context.current() else {
        return;
    };
    let Ok(name) = std::str::from_utf8(name) else {
        return;
    };
    // SAFETY: this unbound resource is accessible only by the exclusive callback.
    if let Some(mut controls) = unsafe { scope.world.get_resource_mut::<super::ScriptControls>() } {
        controls
            .animation
            .push((entity, name.to_owned(), value != 0));
    }
}
