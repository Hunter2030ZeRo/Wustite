use super::super::handles::{HandleError, RuntimeId, StableHandleTable};
use super::super::heap::{GcConfig, GcError, GcHeap, GcObject};
use super::super::roots::RootInventory;
use super::super::value_word::{ScalarValue, ValueWord};

#[test]
fn value_word_roundtrips_immediate_boundaries_boxed_scalars() {
    let heap = GcHeap::new(GcConfig::default());
    for value in [
        -(1_i64 << 46) - 1,
        -(1_i64 << 46),
        -(1_i64 << 46) + 1,
        (1_i64 << 46) - 2,
        (1_i64 << 46) - 1,
        1_i64 << 46,
        i64::MIN,
        i64::MAX,
    ] {
        let word = ValueWord::encode_scalar(ScalarValue::Integer(value), &heap)
            .expect("the scalar allocation should fit");
        assert_eq!(word.decode_scalar(&heap), Ok(ScalarValue::Integer(value)));
        assert_eq!(
            word.is_boxed(),
            !(-(1_i64 << 46)..=(1_i64 << 46) - 1).contains(&value)
        );
    }
    for bits in [
        0.0_f64.to_bits(),
        (-0.0_f64).to_bits(),
        1.0_f64.to_bits(),
        f64::MIN_POSITIVE.to_bits(),
        1_u64,
        f64::INFINITY.to_bits(),
        f64::NEG_INFINITY.to_bits(),
        0x7ff0_0000_0000_0001,
        0x7ff8_0000_0000_0001,
        0x7ffc_0000_0000_0001,
        0xfffd_0000_0000_0001,
    ] {
        let word = ValueWord::encode_scalar(ScalarValue::FloatBits(bits), &heap)
            .expect("the scalar allocation should fit");
        assert_eq!(word.decode_scalar(&heap), Ok(ScalarValue::FloatBits(bits)));
        assert_eq!(word.is_boxed(), f64::from_bits(bits).is_nan());
        if !word.is_boxed() {
            assert_eq!(word.bits(), bits);
        }
    }
    assert_eq!(std::mem::size_of::<ValueWord>(), 8);
}

#[test]
fn handle_table_rejects_stale_foreign_retired() {
    let mut first = StableHandleTable::new(RuntimeId::new(11), 2);
    let second = StableHandleTable::<u8>::new(RuntimeId::new(12), 2);
    let handle = first.allocate(7).expect("the table should have capacity");
    assert_eq!(second.resolve(handle), Err(HandleError::WrongRuntime));
    first
        .release(handle)
        .expect("the live handle should release");
    assert_eq!(first.resolve(handle), Err(HandleError::Stale));
    let reused = first
        .allocate(8)
        .expect("the released slot should be reusable");
    assert_eq!(reused.slot(), handle.slot());
    assert_eq!(reused.generation(), handle.generation() + 1);
    first
        .release(reused)
        .expect("the reused slot should release");
    let last = first
        .allocate_with_generation(9, u16::MAX)
        .expect("the slot should permit generation exhaustion testing");
    first
        .release(last)
        .expect("the exhausted generation should retire");
    assert_eq!(first.retired_slots(), 1);
}

#[test]
fn copied_host_handle_pinned_until_explicit_release() {
    let heap = GcHeap::new(GcConfig::default());
    let handle = heap
        .allocate(GcObject::new())
        .expect("allocation should fit");
    let copied = handle;
    heap.pin_host(handle)
        .expect("host pin should accept a live handle");
    heap.minor_collect(&RootInventory::new())
        .expect("pin should survive collection");
    assert!(heap.resolve(copied).is_ok());
    heap.unpin_host(handle)
        .expect("host pin should release explicitly");
    heap.minor_collect(&RootInventory::new())
        .expect("unpin collection should finish");
    assert_eq!(
        heap.resolve(copied),
        Err(GcError::InvalidHandle(HandleError::Stale))
    );
}

#[test]
fn runtime_teardown_invalidates_handles_in_next_runtime() {
    let handle = {
        let heap = GcHeap::new(GcConfig::default());
        heap.allocate(GcObject::new())
            .expect("allocation should fit")
    };
    let replacement = GcHeap::new(GcConfig::default());
    assert_eq!(
        replacement.resolve(handle),
        Err(GcError::InvalidHandle(HandleError::WrongRuntime))
    );
}
