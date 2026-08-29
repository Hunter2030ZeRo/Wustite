use super::abi::NativeFrame;

pub(super) type NativeEntry = unsafe extern "C" fn(*mut NativeFrame) -> u32;

pub(super) fn call(entry: NativeEntry, frame: &mut NativeFrame) -> u32 {
    // SAFETY: [Categories 3, 5, 6, 8, 10, and 14] `entry` was finalized with
    // the exact NativeEntry ABI and its owning module remains in NativeCode.
    // `frame` is aligned, initialized, live for the call, and generated code
    // bounds every input/output access using compile-time verified arities.
    unsafe { entry(frame) }
}

pub(super) fn from_code_ptr(pointer: *const u8) -> NativeEntry {
    // SAFETY: [Categories 3, 5, 6, 8, and 14] Cranelift returns a non-null,
    // aligned finalized function pointer whose signature is declared as
    // NativeEntry; NativeCode retains the JITModule for the entry lifetime.
    unsafe { std::mem::transmute::<*const u8, NativeEntry>(pointer) }
}
