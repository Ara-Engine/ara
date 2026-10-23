//! Aligned SDK allocations use Rust's allocator, making native callback allocations measurable.

use std::{
    alloc::{Layout, alloc, dealloc},
    ffi::c_void,
    ptr,
};
#[repr(C)]
struct Header {
    base: *mut u8,
    layout: Layout,
}

pub(crate) unsafe extern "C" fn allocate(size: usize, alignment: usize) -> *mut c_void {
    let alignment = alignment.max(align_of::<Header>());
    if !alignment.is_power_of_two() {
        return ptr::null_mut();
    }
    let Some(total) = size
        .checked_add(alignment)
        .and_then(|n| n.checked_add(size_of::<Header>()))
    else {
        return ptr::null_mut();
    };
    let Ok(layout) = Layout::from_size_align(total, alignment) else {
        return ptr::null_mut();
    };
    // SAFETY: checked, nonzero layout; a null allocation is returned to Steam as allocation failure.
    let base = unsafe { alloc(layout) };
    if base.is_null() {
        return ptr::null_mut();
    }
    let address = (base as usize + size_of::<Header>() + alignment - 1) & !(alignment - 1);
    let data = base.wrapping_add(address - base as usize);
    // SAFETY: allocation includes header and alignment padding; aligned Header is wholly within it.
    unsafe {
        data.sub(size_of::<Header>())
            .cast::<Header>()
            .write(Header { base, layout });
    }
    data.cast()
}
pub(crate) unsafe extern "C" fn free(data: *mut c_void) {
    if data.is_null() {
        return;
    }
    // SAFETY: Steam returns exactly a live pointer from allocate, once; its stored header precedes it.
    let header = unsafe {
        data.cast::<u8>()
            .sub(size_of::<Header>())
            .cast::<Header>()
            .read()
    };
    // SAFETY: original allocation base and exact original layout are recovered from the header.
    unsafe {
        dealloc(header.base, header.layout);
    }
}
