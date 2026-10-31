//! Checked x86-64 PC-relative relocation arithmetic.
use super::{RebuildError, RebuildErrorCode};

#[allow(dead_code)]
pub(super) fn evaluate_pc32(symbol: u64, addend: i64, place: u64) -> Result<[u8; 4], RebuildError> {
    let value = i128::from(symbol) + i128::from(addend) - i128::from(place);
    let value = i32::try_from(value).map_err(|_| RebuildError {
        code: RebuildErrorCode::RelocationOverflow,
        detail: "PC-relative displacement exceeds signed 32-bit range".into(),
    })?;
    Ok(value.to_le_bytes())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DefinitionKind {
    Function,
    Data,
}
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(super) struct Definition {
    pub(super) name: String,
    pub(super) address: u64,
    pub(super) size: u64,
    pub(super) kind: DefinitionKind,
    pub(super) binding: u8,
    pub(super) visibility: u8,
    pub(super) file_backed: bool,
    pub(super) readable: bool,
    pub(super) executable: bool,
    pub(super) writable: bool,
    pub(super) dynamic_relocated: bool,
}
#[allow(dead_code)]
pub(super) fn bind_external(
    definitions: &[Definition],
    name: &str,
    kind: DefinitionKind,
    write: bool,
) -> Result<u64, RebuildError> {
    let mut candidates = definitions.iter().filter(|d| d.name == name);
    let definition = candidates.next().ok_or_else(|| RebuildError {
        code: RebuildErrorCode::UndefinedSymbol,
        detail: format!("No same-image definition for {name}"),
    })?;
    if candidates.next().is_some() {
        return Err(RebuildError {
            code: RebuildErrorCode::AmbiguousSymbol,
            detail: format!("Multiple same-image definitions for {name}"),
        });
    }
    if definition.kind != kind
        || definition.binding == 2
        || (definition.binding != 0
            && !(definition.binding == 1 && matches!(definition.visibility, 1..=3)))
        || !definition.file_backed
        || !definition.readable
        || definition.dynamic_relocated
        || (kind == DefinitionKind::Function && !definition.executable)
        || (kind == DefinitionKind::Data
            && (definition.size == 0 || (write && !definition.writable)))
    {
        return Err(RebuildError {
            code: RebuildErrorCode::UnsupportedObject,
            detail: format!(
                "Symbol {name} is not a supported non-preemptible file-backed definition"
            ),
        });
    }
    let span = definition.size.max(1);
    let end = definition
        .address
        .checked_add(span)
        .ok_or_else(|| RebuildError {
            code: RebuildErrorCode::UnsupportedObject,
            detail: "Symbol range overflows".into(),
        })?;
    if definitions.iter().any(|other| {
        !std::ptr::eq(other, definition)
            && other.kind == kind
            && other.address < end
            && other
                .address
                .checked_add(other.size.max(1))
                .is_some_and(|other_end| other_end > definition.address)
    }) {
        return Err(RebuildError {
            code: RebuildErrorCode::AmbiguousSymbol,
            detail: format!("Symbol {name} overlaps another definition"),
        });
    }
    Ok(definition.address)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelocationSite {
    Call,
    Data { write: bool, width: u64 },
}
fn cancelled(cancel: &std::sync::atomic::AtomicBool) -> Result<(), RebuildError> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        Err(RebuildError {
            code: RebuildErrorCode::Cancelled,
            detail: "Relocation cancelled".into(),
        })
    } else {
        Ok(())
    }
}
fn classify_sites(
    code: &[u8],
    needed: &std::collections::BTreeSet<u64>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<std::collections::BTreeMap<u64, RelocationSite>, RebuildError> {
    use iced_x86::{
        Decoder, DecoderOptions, FlowControl, InstructionInfoFactory, OpAccess, OpKind,
    };
    let bad = || RebuildError {
        code: RebuildErrorCode::UnsupportedRelocation,
        detail: "Invalid replacement instruction".into(),
    };
    let mut decoder = Decoder::with_ip(64, code, 0, DecoderOptions::NONE);
    let mut result = std::collections::BTreeMap::new();
    let mut info = InstructionInfoFactory::new();
    while decoder.can_decode() {
        cancelled(cancel)?;
        let ins = decoder.decode();
        if ins.is_invalid() {
            return Err(bad());
        }
        let start = ins.ip();
        let constants = decoder.get_constant_offsets(&ins);
        let immediate = start
            .checked_add(constants.immediate_offset() as u64)
            .ok_or_else(bad)?;
        if ins.flow_control() == FlowControl::Call
            && code.get(start as usize) == Some(&0xe8)
            && constants.immediate_size() == 4
            && needed.contains(&immediate)
        {
            result.insert(immediate, RelocationSite::Call);
        }
        let displacement = start
            .checked_add(constants.displacement_offset() as u64)
            .ok_or_else(bad)?;
        if ins.is_ip_rel_memory_operand()
            && constants.displacement_size() == 4
            && needed.contains(&displacement)
            && !matches!(
                ins.flow_control(),
                FlowControl::IndirectBranch | FlowControl::IndirectCall
            )
        {
            let details = info.info(&ins);
            for i in 0..ins.op_count() {
                if ins.op_kind(i) == OpKind::Memory {
                    let write = matches!(
                        details.op_access(i),
                        OpAccess::Write
                            | OpAccess::CondWrite
                            | OpAccess::ReadWrite
                            | OpAccess::ReadCondWrite
                    );
                    result.insert(
                        displacement,
                        RelocationSite::Data {
                            write,
                            width: ins.memory_size().size().max(1) as u64,
                        },
                    );
                }
            }
        }
    }
    Ok(result)
}
#[cfg(test)]
pub(super) fn classify_site(
    code: &[u8],
    field: u64,
    kind: u32,
) -> Result<RelocationSite, RebuildError> {
    let bad = || RebuildError {
        code: RebuildErrorCode::UnsupportedRelocation,
        detail: "Unsupported relocation site".into(),
    };
    let sites = classify_sites(
        code,
        &std::collections::BTreeSet::from([field]),
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    let site = *sites.get(&field).ok_or_else(bad)?;
    if kind == 4 && site != RelocationSite::Call {
        return Err(bad());
    }
    Ok(site)
}

#[derive(Debug)]
#[allow(dead_code)]
pub(super) struct ResolvedUnit {
    pub(super) sections: std::collections::BTreeMap<u32, Vec<u8>>,
    pub(super) relocations: Vec<super::ResolvedRelocation>,
}
#[allow(dead_code)]
pub(super) fn resolve_unit(
    unit: &super::unit::RelocatableUnit,
    image: &super::elf::ElfLayout,
    placement: &super::layout::AppendLayout,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<ResolvedUnit, RebuildError> {
    use super::unit::UnitSectionKind;
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::atomic::Ordering,
    };
    let bad = |detail: &str| RebuildError {
        code: RebuildErrorCode::UnsupportedRelocation,
        detail: detail.into(),
    };
    cancelled(cancel)?;
    let mut sections: BTreeMap<_, _> = unit
        .sections
        .iter()
        .map(|s| (s.index, s.bytes.clone()))
        .collect();
    let mut records = Vec::with_capacity(unit.relocations.len());
    let mut allowed_calls = BTreeSet::new();
    let mut data_fields = BTreeSet::new();
    let text = unit
        .sections
        .iter()
        .find(|s| s.kind == UnitSectionKind::Text)
        .ok_or_else(|| bad("Missing replacement text"))?;
    if placement.section_addresses.get(&text.index) != Some(&placement.body_address) {
        return Err(bad("Text placement does not match body address"));
    }
    let needed: BTreeSet<_> = unit
        .relocations
        .iter()
        .filter(|r| r.section == text.index && r.raw_kind != 0)
        .map(|r| r.offset)
        .collect();
    let sites = classify_sites(&text.bytes, &needed, cancel)?;
    for relocation in &unit.relocations {
        if cancel.load(Ordering::Relaxed) {
            return Err(RebuildError {
                code: RebuildErrorCode::Cancelled,
                detail: "Relocation cancelled".into(),
            });
        }
        if relocation.raw_kind == 0 {
            continue;
        }
        let section = unit
            .sections
            .iter()
            .find(|s| s.index == relocation.section)
            .ok_or_else(|| bad("Relocation section has no placement"))?;
        let section_address = *placement
            .section_addresses
            .get(&section.index)
            .ok_or_else(|| bad("Relocation section has no address"))?;
        let place = section_address
            .checked_add(relocation.offset)
            .ok_or_else(|| bad("Relocation field address overflow"))?;
        let site = if section.kind == UnitSectionKind::Text {
            let site = *sites
                .get(&relocation.offset)
                .ok_or_else(|| bad("Relocation does not match an instruction field"))?;
            if relocation.raw_kind == 4 && site != RelocationSite::Call {
                return Err(bad("PLT32 requires a direct call"));
            }
            Some(site)
        } else {
            if relocation.raw_kind != 2 {
                return Err(bad("Read-only section requires PC32 relocation"));
            }
            None
        };
        let symbol = unit
            .symbols
            .get(relocation.symbol as usize)
            .filter(|s| s.index == relocation.symbol)
            .ok_or_else(|| bad("Invalid relocation symbol"))?;
        let (target, range, kind) = if let Some(index) = symbol.section {
            let target_section = unit
                .sections
                .iter()
                .find(|s| s.index == index)
                .ok_or_else(|| bad("Symbol refers to unsupported unit section"))?;
            let base = *placement
                .section_addresses
                .get(&index)
                .ok_or_else(|| bad("Symbol section has no address"))?;
            let address = base
                .checked_add(symbol.offset)
                .ok_or_else(|| bad("Unit symbol address overflow"))?;
            let end = base
                .checked_add(target_section.bytes.len() as u64)
                .ok_or_else(|| bad("Unit section address overflow"))?;
            let kind = if target_section.kind == UnitSectionKind::Text {
                DefinitionKind::Function
            } else {
                DefinitionKind::Data
            };
            let range = if symbol.raw_type == 1 {
                if symbol.size == 0 {
                    return Err(bad("Unit data symbol has no known size"));
                }
                let symbol_end = address
                    .checked_add(symbol.size)
                    .ok_or_else(|| bad("Unit data symbol range overflow"))?;
                if symbol_end > end {
                    return Err(bad("Unit data symbol exceeds its section"));
                }
                address..symbol_end
            } else {
                base..end
            };
            (address, range, kind)
        } else {
            let kind = match site {
                Some(RelocationSite::Call) => DefinitionKind::Function,
                Some(RelocationSite::Data { .. }) => DefinitionKind::Data,
                None => match symbol.raw_type {
                    2 => DefinitionKind::Function,
                    _ => DefinitionKind::Data,
                },
            };
            let write = matches!(site, Some(RelocationSite::Data { write: true, .. }));
            let address = bind_external(&image.definitions, &symbol.name, kind, write)?;
            let definition = image
                .definitions
                .iter()
                .find(|d| d.name == symbol.name)
                .ok_or_else(|| bad("Missing bound definition"))?;
            let end = address
                .checked_add(definition.size.max(1))
                .ok_or_else(|| bad("Definition range overflow"))?;
            (address, address..end, kind)
        };
        if let Some(site) = site {
            let effective = i128::from(target) + i128::from(relocation.addend) + 4;
            let effective =
                u64::try_from(effective).map_err(|_| bad("Relocation target address overflow"))?;
            match site {
                RelocationSite::Call => {
                    if kind != DefinitionKind::Function || effective != target {
                        return Err(bad("Call does not target an admitted function entry"));
                    }
                    allowed_calls.insert(target);
                }
                RelocationSite::Data { write, width } => {
                    data_fields.insert(place);
                    if kind != DefinitionKind::Data
                        || effective < range.start
                        || effective
                            .checked_add(width)
                            .is_none_or(|end| end > range.end)
                        || (write && symbol.section.is_some())
                    {
                        return Err(bad("Data reference is outside an admitted data range"));
                    }
                }
            }
        }
        let patch = evaluate_pc32(target, relocation.addend, place)?;
        let data = sections
            .get_mut(&section.index)
            .ok_or_else(|| bad("Missing output section"))?;
        let offset =
            usize::try_from(relocation.offset).map_err(|_| bad("Relocation offset overflow"))?;
        let end = offset
            .checked_add(4)
            .ok_or_else(|| bad("Relocation offset overflow"))?;
        data.get_mut(offset..end)
            .ok_or_else(|| bad("Relocation outside output section"))?
            .copy_from_slice(&patch);
        records.push(super::ResolvedRelocation {
            raw_kind: relocation.raw_kind,
            field_address: place,
            target_address: target,
            addend: relocation.addend,
            symbol: symbol.name.clone(),
        });
    }
    validate_text(
        &sections[&text.index],
        placement.body_address,
        &allowed_calls,
        &data_fields,
        cancel,
    )?;
    Ok(ResolvedUnit {
        sections,
        relocations: records,
    })
}
fn validate_text(
    code: &[u8],
    base: u64,
    allowed_calls: &std::collections::BTreeSet<u64>,
    data_fields: &std::collections::BTreeSet<u64>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), RebuildError> {
    use iced_x86::{Decoder, DecoderOptions, FlowControl, OpKind};
    let bad = || RebuildError {
        code: RebuildErrorCode::UnsupportedRelocation,
        detail: "Replacement contains unsupported control flow".into(),
    };
    let end = base.checked_add(code.len() as u64).ok_or_else(bad)?;
    let mut decoder = Decoder::with_ip(64, code, base, DecoderOptions::NONE);
    let mut instructions = Vec::new();
    let mut boundaries = std::collections::BTreeSet::new();
    while decoder.can_decode() {
        cancelled(cancel)?;
        let instruction = decoder.decode();
        if instruction.is_invalid() {
            return Err(bad());
        }
        if instruction.is_ip_rel_memory_operand() {
            let constants = decoder.get_constant_offsets(&instruction);
            let field = instruction
                .ip()
                .checked_add(constants.displacement_offset() as u64)
                .ok_or_else(bad)?;
            if constants.displacement_size() != 4 || !data_fields.contains(&field) {
                return Err(bad());
            }
        }
        boundaries.insert(instruction.ip());
        instructions.push(instruction);
    }
    for ins in &instructions {
        cancelled(cancel)?;
        let direct = (0..ins.op_count()).any(|i| {
            matches!(
                ins.op_kind(i),
                OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
            )
        });
        match ins.flow_control() {
            FlowControl::IndirectBranch | FlowControl::IndirectCall => return Err(bad()),
            FlowControl::UnconditionalBranch | FlowControl::ConditionalBranch if direct => {
                let target = ins.near_branch_target();
                if target < base || target >= end || !boundaries.contains(&target) {
                    return Err(bad());
                }
            }
            FlowControl::Call if direct => {
                let target = ins.near_branch_target();
                if !(allowed_calls.contains(&target)
                    || target >= base && target < end && boundaries.contains(&target))
                {
                    return Err(bad());
                }
            }
            FlowControl::Call
            | FlowControl::UnconditionalBranch
            | FlowControl::ConditionalBranch => return Err(bad()),
            _ => {}
        }
    }
    if instructions.last().is_none_or(|i| {
        !matches!(
            i.flow_control(),
            FlowControl::Return | FlowControl::UnconditionalBranch | FlowControl::Exception
        )
    }) {
        return Err(bad());
    }
    Ok(())
}
