use std::ffi::c_void;

use super::context::{ERROR_VALUE, HelperContext};

pub(super) extern "C" fn object_get(pointer: *mut c_void, handle: u64, key: i64) -> i64 {
    let Some(context) = helper_context(pointer) else {
        return ERROR_VALUE;
    };
    context.invoke(|runtime| runtime.object_get(handle, key))
}

pub(super) extern "C" fn object_set(
    pointer: *mut c_void,
    handle: u64,
    key: i64,
    value: i64,
) -> i64 {
    let Some(context) = helper_context(pointer) else {
        return ERROR_VALUE;
    };
    context.invoke(|runtime| runtime.object_set(handle, key, value))
}

pub(super) extern "C" fn list_get(pointer: *mut c_void, handle: u64, index: i64) -> i64 {
    let Some(context) = helper_context(pointer) else {
        return ERROR_VALUE;
    };
    context.invoke(|runtime| runtime.list_get(handle, index))
}

pub(super) extern "C" fn list_set(
    pointer: *mut c_void,
    handle: u64,
    index: i64,
    value: i64,
) -> i64 {
    let Some(context) = helper_context(pointer) else {
        return ERROR_VALUE;
    };
    context.invoke(|runtime| runtime.list_set(handle, index, value))
}

pub(super) extern "C" fn list_append(pointer: *mut c_void, handle: u64, value: i64) -> i64 {
    let Some(context) = helper_context(pointer) else {
        return ERROR_VALUE;
    };
    context.invoke(|runtime| runtime.list_append(handle, value))
}

pub(super) extern "C" fn direct_call(
    pointer: *mut c_void,
    callee: u64,
    left: i64,
    right: i64,
) -> i64 {
    let Some(context) = helper_context(pointer) else {
        return ERROR_VALUE;
    };
    context.invoke(|runtime| runtime.direct_call(callee, left, right))
}

fn helper_context<'a>(pointer: *mut c_void) -> Option<&'a mut HelperContext<'a>> {
    if pointer.is_null() || !pointer.is_aligned() {
        return None;
    }
    // SAFETY: [Categories 1, 3, 5, 6, 8, and 9] NativeCode installs a unique,
    // aligned pointer to its live stack HelperContext for synchronous helper
    // calls, and generated code never stores that pointer after returning.
    Some(unsafe { &mut *pointer.cast::<HelperContext<'a>>() })
}
