//! Placement assignments consumed by relocation; full append planning is separate.
use std::collections::BTreeMap;
#[derive(Debug)]
#[allow(dead_code)]
pub(super) struct AppendLayout {
    pub(super) section_addresses: BTreeMap<u32, u64>,
    pub(super) body_address: u64,
}
