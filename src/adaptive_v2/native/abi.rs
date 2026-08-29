use std::ffi::c_void;

use super::NativeValue;
use crate::adaptive_v2::wxir_v2::SnapshotId;
use crate::adaptive_v2::wxir_v2::ir::ValueType;

pub(super) const NATIVE_ABI_MAGIC: u64 = 0x5755_5354_5632_4e41;
pub(super) const NATIVE_ABI_VERSION: u32 = 3;
pub(super) const DIRECT_STORAGE_MAGIC: u64 = 0x5755_5354_4453_4936;
pub(super) const DIRECT_STORAGE_ABI: u32 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NativeDirectStorage {
    pub(super) magic: u64,
    pub(super) abi: u32,
    pub(super) strategy: u32,
    pub(super) alias: u64,
    pub(super) owner: u64,
    pub(super) layout_epoch: u64,
    pub(super) version: u64,
    pub(super) length: u64,
    pub(super) capacity: u64,
    pub(super) values: *mut i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NativeDirectStorageReceipt {
    pub(super) storage_identity: u64,
    pub(super) strategy: u32,
    pub(super) reserved: u32,
    pub(super) alias: u64,
    pub(super) owner: u64,
    pub(super) layout_epoch: u64,
    pub(super) version: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NativeSlot {
    pub(super) tag: u32,
    pub(super) reserved: u32,
    pub(super) payload: u64,
}

impl NativeSlot {
    pub(super) const fn from_value(value: NativeValue) -> Self {
        match value {
            NativeValue::Integer(value) => Self {
                tag: 1,
                reserved: 0,
                payload: value as u64,
            },
            NativeValue::FloatBits(value) => Self {
                tag: 4,
                reserved: 0,
                payload: value,
            },
            NativeValue::Boolean(value) => Self {
                tag: 2,
                reserved: 0,
                payload: value as u64,
            },
            NativeValue::Handle(value) => Self {
                tag: 3,
                reserved: 0,
                payload: value,
            },
        }
    }

    pub(super) fn to_value(self) -> Result<NativeValue, super::NativeError> {
        match self.tag {
            1 => Ok(NativeValue::Integer(self.payload as i64)),
            2 if self.payload <= 1 => Ok(NativeValue::Boolean(self.payload != 0)),
            3 => Ok(NativeValue::Handle(self.payload)),
            4 => Ok(NativeValue::FloatBits(self.payload)),
            _ => Err(super::NativeError::MalformedValue),
        }
    }

    pub(super) const fn zero(ty: ValueType) -> Result<Self, super::NativeError> {
        match ty {
            ValueType::I64 => Ok(Self::from_value(NativeValue::Integer(0))),
            ValueType::Bool => Ok(Self::from_value(NativeValue::Boolean(false))),
            ValueType::Handle => Ok(Self::from_value(NativeValue::Handle(0))),
            ValueType::F64 => Ok(Self::from_value(NativeValue::FloatBits(0))),
            ValueType::BorrowedView => Err(super::NativeError::MalformedValue),
        }
    }
}

#[repr(C)]
pub(super) struct NativeFrame {
    pub(super) magic: u64,
    pub(super) abi: u32,
    pub(super) input_count: u32,
    pub(super) output_capacity: u32,
    pub(super) exit_kind: u32,
    pub(super) snapshot_id: [u8; 32],
    pub(super) inputs: *const NativeSlot,
    pub(super) outputs: *mut NativeSlot,
    pub(super) helper_context: *mut c_void,
    pub(super) machine_entries: u64,
    pub(super) generic_dispatch_calls: u64,
    pub(super) helper_calls: u64,
    pub(super) deopts: u64,
    pub(super) exit_id: u32,
    pub(super) guard_id: u32,
    pub(super) safepoint_id: u32,
    pub(super) deopt_id: u32,
    pub(super) direct_storage: *const NativeDirectStorage,
    pub(super) direct_storage_receipts: *const NativeDirectStorageReceipt,
    pub(super) direct_storage_count: u32,
    pub(super) direct_storage_reserved: u32,
    pub(super) direct_storage_index: *const u8,
}

impl NativeFrame {
    pub(super) fn new(
        snapshot_id: SnapshotId,
        inputs: &[NativeSlot],
        outputs: &mut [NativeSlot],
    ) -> Result<Self, super::NativeError> {
        Ok(Self {
            magic: NATIVE_ABI_MAGIC,
            abi: NATIVE_ABI_VERSION,
            input_count: u32::try_from(inputs.len())
                .map_err(|_| super::NativeError::CountOverflow)?,
            output_capacity: u32::try_from(outputs.len())
                .map_err(|_| super::NativeError::CountOverflow)?,
            exit_kind: 0,
            snapshot_id: snapshot_id.as_bytes(),
            inputs: inputs.as_ptr(),
            outputs: outputs.as_mut_ptr(),
            helper_context: std::ptr::null_mut(),
            machine_entries: 0,
            generic_dispatch_calls: 0,
            helper_calls: 0,
            deopts: 0,
            exit_id: 0,
            guard_id: 0,
            safepoint_id: 0,
            deopt_id: 0,
            direct_storage: std::ptr::null(),
            direct_storage_receipts: std::ptr::null(),
            direct_storage_count: 0,
            direct_storage_reserved: 0,
            direct_storage_index: std::ptr::null(),
        })
    }
}

pub(super) const INPUTS_OFFSET: i32 = 56;
pub(super) const OUTPUTS_OFFSET: i32 = 64;
pub(super) const MACHINE_ENTRIES_OFFSET: i32 = 80;
pub(super) const HELPER_CONTEXT_OFFSET: i32 = 72;
pub(super) const HELPER_CALLS_OFFSET: i32 = 96;
pub(super) const DEOPTS_OFFSET: i32 = 104;
pub(super) const EXIT_KIND_OFFSET: i32 = 20;
pub(super) const EXIT_ID_OFFSET: i32 = 112;
pub(super) const GUARD_ID_OFFSET: i32 = 116;
pub(super) const SAFEPOINT_ID_OFFSET: i32 = 120;
pub(super) const DEOPT_ID_OFFSET: i32 = 124;
pub(super) const SLOT_SIZE: i32 = 16;
pub(super) const SLOT_PAYLOAD_OFFSET: i32 = 8;
pub(super) const DIRECT_STORAGE_OFFSET: i32 = 128;
pub(super) const DIRECT_STORAGE_RECEIPTS_OFFSET: i32 = 136;
pub(super) const DIRECT_STORAGE_COUNT_OFFSET: i32 = 144;
pub(super) const DIRECT_STORAGE_INDEX_OFFSET: i32 = 152;
pub(super) const DIRECT_MAGIC_OFFSET: i32 = 0;
pub(super) const DIRECT_ABI_OFFSET: i32 = 8;
pub(super) const DIRECT_STRATEGY_OFFSET: i32 = 12;
pub(super) const DIRECT_ALIAS_OFFSET: i32 = 16;
pub(super) const DIRECT_OWNER_OFFSET: i32 = 24;
pub(super) const DIRECT_LAYOUT_EPOCH_OFFSET: i32 = 32;
pub(super) const DIRECT_VERSION_OFFSET: i32 = 40;
pub(super) const DIRECT_LENGTH_OFFSET: i32 = 48;
pub(super) const DIRECT_CAPACITY_OFFSET: i32 = 56;
pub(super) const DIRECT_VALUES_OFFSET: i32 = 64;
pub(super) const RECEIPT_STORAGE_IDENTITY_OFFSET: i32 = 0;
pub(super) const RECEIPT_STRATEGY_OFFSET: i32 = 8;
pub(super) const RECEIPT_ALIAS_OFFSET: i32 = 16;
pub(super) const RECEIPT_OWNER_OFFSET: i32 = 24;
pub(super) const RECEIPT_LAYOUT_EPOCH_OFFSET: i32 = 32;
pub(super) const RECEIPT_VERSION_OFFSET: i32 = 40;

const _: () = {
    assert!(std::mem::offset_of!(NativeFrame, inputs) == INPUTS_OFFSET as usize);
    assert!(std::mem::offset_of!(NativeFrame, outputs) == OUTPUTS_OFFSET as usize);
    assert!(std::mem::offset_of!(NativeFrame, machine_entries) == MACHINE_ENTRIES_OFFSET as usize);
    assert!(std::mem::offset_of!(NativeFrame, deopts) == DEOPTS_OFFSET as usize);
    assert!(std::mem::offset_of!(NativeFrame, exit_id) == EXIT_ID_OFFSET as usize);
    assert!(std::mem::offset_of!(NativeFrame, direct_storage) == DIRECT_STORAGE_OFFSET as usize);
    assert!(
        std::mem::offset_of!(NativeFrame, direct_storage_receipts)
            == DIRECT_STORAGE_RECEIPTS_OFFSET as usize
    );
    assert!(
        std::mem::offset_of!(NativeFrame, direct_storage_count)
            == DIRECT_STORAGE_COUNT_OFFSET as usize
    );
    assert!(
        std::mem::offset_of!(NativeFrame, direct_storage_index)
            == DIRECT_STORAGE_INDEX_OFFSET as usize
    );
    assert!(std::mem::size_of::<NativeSlot>() == SLOT_SIZE as usize);
    assert!(std::mem::size_of::<NativeDirectStorage>() == 72);
    assert!(std::mem::align_of::<NativeDirectStorage>() == 8);
    assert!(std::mem::offset_of!(NativeDirectStorage, values) == DIRECT_VALUES_OFFSET as usize);
    assert!(std::mem::size_of::<NativeDirectStorageReceipt>() == 48);
    assert!(
        std::mem::offset_of!(NativeDirectStorageReceipt, version)
            == RECEIPT_VERSION_OFFSET as usize
    );
};
