//! Target adapter for the strict ordinary STA/AP TX descriptor policy.
//!
//! This boundary replaces direct Rust management-frame calls immediately.
//! The vendor `ieee80211_output_process` still contains an object-local call
//! to its own descriptor builder; that data-path reference disappears only
//! when the surrounding output consumer is replaced.

use core::ptr;

use crate::net80211_descriptor::{ordinary_descriptor_policy, DescriptorPolicyInput};

const NODE_INTERFACE_OFFSET: usize = 0x00;
const NODE_RATE_CONTEXT_INDEX_OFFSET: usize = 0x26;
const NODE_DESCRIPTOR_NIBBLE_OFFSET: usize = 0x2b8;
const NODE_POLICY_WORD_OFFSET: usize = 0x348;
const NODE_WORD_0X35C_OFFSET: usize = 0x35c;
const NODE_BYTE_0X38C_OFFSET: usize = 0x38c;
const NODE_BYTE_0X45A_OFFSET: usize = 0x45a;
const NODE_BYTE_0X4F3_OFFSET: usize = 0x4f3;
const NODE_TWT_RECORD_INDEX_BASE: usize = 0x74;
const NODE_TWT_RECORD_SIZE: usize = 8;

const INTERFACE_STATE_OFFSET: usize = 0x154;

const ESF_TWT_RECORD_PRESENT_OFFSET: usize = 0x28;
const ESF_RATE_CONTEXT_OFFSET: usize = 0x2c;
const ESF_DESCRIPTOR_OFFSET: usize = 0x34;

const DESCRIPTOR_PRIORITY_OFFSET: usize = 0x04;
const DESCRIPTOR_SECURITY_OFFSET: usize = 0x10;
const DESCRIPTOR_TIMESTAMP_OFFSET: usize = 0x18;
const DESCRIPTOR_BYTE_0X2A_OFFSET: usize = 0x2a;
const DESCRIPTOR_BYTE_0X2E_OFFSET: usize = 0x2e;
const DESCRIPTOR_BYTE_0X2F_OFFSET: usize = 0x2f;
const DESCRIPTOR_HE_CONTROL_OFFSET: usize = 0x30;
const DESCRIPTOR_HE_WORD_0X34_OFFSET: usize = 0x34;
const DESCRIPTOR_BYTE_0X38_OFFSET: usize = 0x38;
const DESCRIPTOR_TWT_FLOW_ID_OFFSET: usize = 0x39;
const DESCRIPTOR_TWT_RECORD_OFFSET: usize = 0x40;

unsafe extern "C" {
    fn __real_ieee80211_set_tx_desc(
        node: *mut u8,
        buffer: *mut u8,
        priority: u32,
        requested_flags: u32,
        unused: u32,
    );
    fn hal_now() -> u32;
    fn ic_get_trc(selector: u32, index: u32) -> *mut u8;
}

pub(crate) fn link_wrapper_active() -> bool {
    // The linker script proves the alias with a final-value ASSERT. Comparing
    // the two Rust function pointers here is unsound as a proof: LLVM does not
    // model linker-script aliases and folds that comparison before linking.
    true
}

#[inline(always)]
unsafe fn trap_invalid_descriptor(
    node: *mut u8,
    buffer: *mut u8,
    priority: u32,
    requested_flags: u32,
) -> ! {
    core::arch::asm!(
        "ebreak",
        in("a0") node,
        in("a1") buffer,
        in("a2") priority,
        in("a3") requested_flags,
        options(noreturn)
    )
}

/// Build one finite non-HE STA/AP TX descriptor from Rust-owned handoff state.
///
/// No allocation, wait, retry, lock, indirect callback, NVS access, or `g_ic`
/// read occurs. `hal_now` is a direct finite timestamp leaf. `ic_get_trc` is
/// retained temporarily as the next explicit ownership boundary for the
/// fixed rate-control context table.
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn wifi_strict_ieee80211_set_tx_desc(
    node: *mut u8,
    buffer: *mut u8,
    priority: u32,
    requested_flags: u32,
    unused: u32,
) {
    if !crate::critical::strict_wifi_hart_armed() {
        __real_ieee80211_set_tx_desc(node, buffer, priority, requested_flags, unused);
        return;
    }
    if !crate::critical::on_strict_wifi_hart()
        || !crate::context::in_radio_context()
        || !crate::net80211_state::ordinary_sta_ap_profile()
        || node.is_null()
        || buffer.is_null()
        || priority > 7
    {
        trap_invalid_descriptor(node, buffer, priority, requested_flags);
    }

    let interface = node.add(NODE_INTERFACE_OFFSET).cast::<*mut u8>().read();
    let Some(role) = crate::net80211_state::role_for_interface(interface) else {
        trap_invalid_descriptor(node, buffer, priority, requested_flags);
    };
    let Some((adopted_config_byte_0x44a, individual_twt_flow_id)) =
        crate::net80211_state::descriptor_config()
    else {
        trap_invalid_descriptor(node, buffer, priority, requested_flags);
    };
    let descriptor = buffer.add(ESF_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    if descriptor.is_null() {
        trap_invalid_descriptor(node, buffer, priority, requested_flags);
    }

    let Some(policy) = ordinary_descriptor_policy(DescriptorPolicyInput {
        role,
        priority: priority as u8,
        existing_flags: descriptor.cast::<u32>().read(),
        requested_flags,
        existing_security: descriptor
            .add(DESCRIPTOR_SECURITY_OFFSET)
            .cast::<u32>()
            .read(),
        existing_he_control: descriptor
            .add(DESCRIPTOR_HE_CONTROL_OFFSET)
            .cast::<u16>()
            .read(),
        node_descriptor_nibble_0x2b8: node.add(NODE_DESCRIPTOR_NIBBLE_OFFSET).read(),
        node_rate_context_index_0x26: node.add(NODE_RATE_CONTEXT_INDEX_OFFSET).read(),
        interface_state_0x154: interface.add(INTERFACE_STATE_OFFSET).read(),
        node_policy_word_0x348: node.add(NODE_POLICY_WORD_OFFSET).cast::<u32>().read(),
        adopted_config_byte_0x44a,
        node_word_0x35c: node.add(NODE_WORD_0X35C_OFFSET).cast::<u16>().read(),
        node_byte_0x38c: node.add(NODE_BYTE_0X38C_OFFSET).read(),
        node_byte_0x45a: node.add(NODE_BYTE_0X45A_OFFSET).read(),
        node_byte_0x4f3: node.add(NODE_BYTE_0X4F3_OFFSET).read(),
    }) else {
        trap_invalid_descriptor(node, buffer, priority, requested_flags);
    };
    let rate_context = ic_get_trc(
        u32::from(policy.rate_context_selector),
        u32::from(policy.rate_context_index),
    );
    if rate_context.is_null() {
        trap_invalid_descriptor(node, buffer, priority, requested_flags);
    }

    // All fallible validation is complete before the first descriptor write.
    descriptor
        .add(DESCRIPTOR_PRIORITY_OFFSET)
        .write(policy.priority_byte);
    descriptor
        .add(DESCRIPTOR_TIMESTAMP_OFFSET)
        .cast::<u32>()
        .write(hal_now());
    descriptor.cast::<u32>().write(policy.flags);
    descriptor
        .add(DESCRIPTOR_SECURITY_OFFSET)
        .cast::<u32>()
        .write(policy.security);
    descriptor
        .add(DESCRIPTOR_HE_CONTROL_OFFSET)
        .cast::<u16>()
        .write(policy.he_control);
    descriptor
        .add(DESCRIPTOR_HE_WORD_0X34_OFFSET)
        .cast::<u32>()
        .write(0);
    descriptor.add(DESCRIPTOR_BYTE_0X2A_OFFSET).write(1);
    descriptor.add(DESCRIPTOR_BYTE_0X2E_OFFSET).write(1);

    if buffer.add(ESF_TWT_RECORD_PRESENT_OFFSET).read() == 0 {
        descriptor
            .add(DESCRIPTOR_TWT_FLOW_ID_OFFSET)
            .write(individual_twt_flow_id);
        let source = node.add(
            (NODE_TWT_RECORD_INDEX_BASE + usize::from(individual_twt_flow_id))
                * NODE_TWT_RECORD_SIZE,
        );
        ptr::copy_nonoverlapping(
            source,
            descriptor.add(DESCRIPTOR_TWT_RECORD_OFFSET),
            NODE_TWT_RECORD_SIZE,
        );
    }
    descriptor
        .add(DESCRIPTOR_BYTE_0X38_OFFSET)
        .write(policy.byte_0x38);
    let opaque = descriptor.add(DESCRIPTOR_BYTE_0X2F_OFFSET);
    opaque.write((opaque.read() & !(1 << 2)) | if policy.byte_0x2f_bit_2 { 1 << 2 } else { 0 });

    buffer
        .add(ESF_RATE_CONTEXT_OFFSET)
        .cast::<*mut u8>()
        .write(rate_context);
}
