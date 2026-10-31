use super::*;
use crate::{Mapping, Section, Symbol};

const BASE: u64 = 0x1000;

// Test-only declared mappings isolate traversal from the loader, which has its
// own independently laid-out ELF/PE/Mach-O fixtures in systems-core/src/tests.rs.
fn image(bytes: &[u8]) -> BinaryImage {
    BinaryImage {
        bytes: bytes.to_vec().into(),
        format: BinaryFormat::Elf,
        architecture: Architecture::X86_64,
        entry: Some(BASE),
        sections: vec![Section {
            name: ".text".into(),
            address: BASE,
            file_offset: 0,
            size: bytes.len() as u64,
            file_size: bytes.len(),
            executable: true,
        }],
        symbols: Vec::new(),
        source_locations: Vec::new(),
        warnings: Vec::new(),
        mappings: vec![Mapping {
            readonly: true,
            address: BASE,
            offset: 0,
            size: bytes.len(),
        }],
        source_end_prefix: Vec::new(),
        rewrite_locations: Vec::new(),
    }
}

fn analyze(image: &BinaryImage, limits: AnalysisLimits) -> Analysis {
    image
        .analyze(&Architecture::X86_64, limits, &AtomicBool::new(false))
        .unwrap()
}

#[test]
fn range_index_matches_exhaustive_interval_membership() {
    let mut state = 17_u64;
    let mut entries = Vec::new();
    for id in 0..200 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let start = state % 128;
        let end = start + ((state >> 32) % 17);
        entries.push((start, end, id));
    }
    let index = RangeIndex::new(entries.clone());
    for point in 0..150 {
        let owners: Vec<_> = entries
            .iter()
            .filter(|(start, end, _)| *start <= point && point < *end)
            .map(|(_, _, owner)| *owner)
            .collect();
        let expected = match owners.as_slice() {
            [] => Err(EdgeResolution::Unmapped),
            [owner] => Ok(*owner),
            _ => Err(EdgeResolution::Overlap),
        };
        assert_eq!(index.find(point), expected, "address {point}");
    }
}

#[test]
fn range_index_handles_nested_expired_intervals_and_exclusive_ends() {
    let mut entries = vec![(0, 20_000, 0)];
    for id in 1..10_000 {
        entries.push((id as u64, id as u64 + 1, id));
    }
    let index = RangeIndex::new(entries);
    assert_eq!(index.find(0), Ok(0));
    assert_eq!(index.find(9_999), Err(EdgeResolution::Overlap));
    assert_eq!(index.find(10_000), Ok(0));
    assert_eq!(index.find(19_999), Ok(0));
    assert_eq!(index.find(20_000), Err(EdgeResolution::Unmapped));
}

#[test]
fn range_index_preserves_equal_end_ambiguity_and_empty_ranges() {
    let index = RangeIndex::new(vec![(4, 8, 1), (6, 8, 2), (5, 5, 3)]);
    assert_eq!(index.find(3), Err(EdgeResolution::Unmapped));
    assert_eq!(index.find(5), Ok(1));
    assert_eq!(index.find(6), Err(EdgeResolution::Overlap));
    assert_eq!(index.find(8), Err(EdgeResolution::Unmapped));
    assert_eq!(
        RangeIndex::new(vec![]).find(0),
        Err(EdgeResolution::Unmapped)
    );
}

#[test]
fn diamond_branches_split_shared_tail_at_a_precomputed_leader() {
    // je 5; nop; jmp 6; nop; ret
    let result = analyze(
        &image(&[0x74, 3, 0x90, 0xeb, 1, 0x90, 0xc3]),
        AnalysisLimits::default(),
    );
    assert!(!result.truncated);
    assert_eq!(result.instruction_count, 5);
    let blocks = &result.functions[0].blocks;
    assert_eq!(
        blocks
            .iter()
            .map(|block| block.start_offset)
            .collect::<Vec<_>>(),
        [0, 2, 5, 6]
    );
    assert_eq!(blocks[0].edges[0].target_offset, Some(5));
    assert_eq!(blocks[0].edges[1].target_offset, Some(2));
    assert_eq!(blocks[1].edges[0].target_offset, Some(6));
    assert_eq!(blocks[2].edges[0].target_offset, Some(6));
    assert_eq!(blocks[3].edges[0].resolution, EdgeResolution::Terminal);
}

#[test]
fn cycle_terminates_without_duplicating_instructions() {
    let result = analyze(&image(&[0x90, 0xeb, 0xfd]), AnalysisLimits::default());
    assert!(!result.truncated);
    assert_eq!(result.instruction_count, 2);
    assert_eq!(result.functions[0].blocks.len(), 1);
    let edge = &result.functions[0].blocks[0].edges[0];
    assert_eq!(edge.target_offset, Some(0));
    assert_eq!(edge.resolution, EdgeResolution::Resolved);
}

#[test]
fn direct_call_discovers_callee_without_merging_its_body_into_caller() {
    let result = analyze(
        &image(&[0xe8, 1, 0, 0, 0, 0xc3, 0xc3]),
        AnalysisLimits::default(),
    );
    assert_eq!(result.functions.len(), 2);
    assert_eq!(result.functions[1].offset, 6);
    assert_eq!(
        result.functions[1].provenance,
        FunctionProvenance::DirectCall
    );
    assert_eq!(result.instruction_count, 3);
    assert_eq!(result.references.len(), 1);
    assert_eq!(result.references[0].kind, ReferenceKind::Call);
    assert_eq!(result.references[0].resolution, EdgeResolution::Resolved);
}

#[test]
fn indirect_flow_remains_unresolved() {
    let result = analyze(&image(&[0xff, 0xd0, 0xff, 0xe0]), AnalysisLimits::default());
    assert_eq!(result.functions.len(), 1);
    assert_eq!(result.instruction_count, 2);
    assert_eq!(result.references.len(), 2);
    assert!(result.references.iter().all(|reference| {
        reference.resolution == EdgeResolution::Indirect && reference.target_address.is_none()
    }));
}

#[test]
fn branch_into_decoded_instruction_is_an_overlap_not_a_second_instruction() {
    // mov eax,0; jmp 2 (inside mov's immediate)
    let result = analyze(
        &image(&[0xb8, 0, 0, 0, 0, 0xeb, 0xfb]),
        AnalysisLimits::default(),
    );
    assert_eq!(result.instruction_count, 2);
    assert_eq!(result.references[0].resolution, EdgeResolution::Overlap);
    assert_eq!(result.references[0].target_offset, Some(2));
}

#[test]
fn branch_to_a_displayed_invalid_byte_is_not_resolved_code() {
    let result = analyze(&image(&[0xeb, 0, 0x0f]), AnalysisLimits::default());
    assert_eq!(result.instruction_count, 2);
    assert_eq!(result.references[0].target_offset, Some(2));
    assert_eq!(result.references[0].resolution, EdgeResolution::Invalid);
    assert!(!result.functions[0].blocks[1].instructions[0].valid);
}

#[test]
fn branch_to_nonexecutable_mapping_keeps_its_failure_reason() {
    let mut input = image(&[0xe9, 0xfb, 0, 0, 0, 0xc3]);
    input.mappings[0].size = 5;
    input.sections[0].size = 5;
    input.sections[0].file_size = 5;
    input.mappings.push(Mapping {
        readonly: true,
        address: BASE + 0x100,
        offset: 5,
        size: 1,
    });
    let result = analyze(&input, AnalysisLimits::default());
    assert_eq!(result.instruction_count, 1);
    assert_eq!(
        result.references[0].resolution,
        EdgeResolution::NonExecutable
    );
    assert_eq!(result.references[0].target_offset, Some(5));
}

#[test]
fn instruction_limit_marks_omitted_successor() {
    let result = analyze(
        &image(&[0x90, 0x90, 0x90, 0xc3]),
        AnalysisLimits {
            max_instructions: 2,
            ..AnalysisLimits::default()
        },
    );
    assert!(result.truncated);
    assert_eq!(result.instruction_count, 2);
    assert_eq!(
        result.functions[0].blocks[0].edges[0].resolution,
        EdgeResolution::Limit
    );
}

#[test]
fn byte_budget_cannot_include_part_of_a_valid_instruction() {
    let result = analyze(
        &image(&[0xb8, 0, 0, 0, 0]),
        AnalysisLimits {
            max_bytes: 4,
            ..AnalysisLimits::default()
        },
    );
    assert!(result.truncated);
    assert_eq!(result.instruction_count, 0);
    assert!(result.functions[0].blocks.is_empty());
}

#[test]
fn block_limit_marks_unemitted_branch_targets() {
    let result = analyze(
        &image(&[0x74, 3, 0x90, 0xeb, 1, 0x90, 0xc3]),
        AnalysisLimits {
            max_blocks: 1,
            ..AnalysisLimits::default()
        },
    );
    assert!(result.truncated);
    assert_eq!(result.instruction_count, 1);
    assert_eq!(result.functions[0].blocks.len(), 1);
    assert!(
        result.functions[0].blocks[0]
            .edges
            .iter()
            .all(|edge| edge.resolution == EdgeResolution::Limit)
    );
}

#[test]
fn function_limit_marks_unvisited_callee() {
    let result = analyze(
        &image(&[0xe8, 1, 0, 0, 0, 0xc3, 0xc3]),
        AnalysisLimits {
            max_functions: 1,
            ..AnalysisLimits::default()
        },
    );
    assert!(result.truncated);
    assert_eq!(result.functions.len(), 1);
    assert_eq!(result.references[0].resolution, EdgeResolution::Limit);
}

#[test]
fn shared_tail_reference_is_deduplicated_without_losing_function_membership() {
    let mut input = image(&[0xeb, 4, 0xeb, 2, 0xcc, 0xcc, 0xe8, 1, 0, 0, 0, 0xc3, 0xc3]);
    input.symbols.push(Symbol {
        name: "second_root".into(),
        address: BASE + 2,
        file_offset: Some(2),
        size: 2,
    });
    let result = analyze(&input, AnalysisLimits::default());
    assert_eq!(result.functions.len(), 3);
    assert_eq!(result.instruction_count, 7);
    assert_eq!(result.references.len(), 3);
    assert_eq!(
        result
            .references
            .iter()
            .filter(|reference| reference.from_offset == 6)
            .count(),
        1
    );
}

#[test]
fn cancellation_and_invalid_limits_fail_before_traversal() {
    let input = image(&[0xc3]);
    assert!(
        input
            .analyze(
                &Architecture::X86_64,
                AnalysisLimits::default(),
                &AtomicBool::new(true)
            )
            .is_err()
    );
    assert!(
        input
            .analyze(
                &Architecture::X86_64,
                AnalysisLimits {
                    max_blocks: 0,
                    ..AnalysisLimits::default()
                },
                &AtomicBool::new(false)
            )
            .is_err()
    );
}

#[test]
fn raw_and_mismatched_architectures_are_rejected() {
    let mut input = image(&[0xc3]);
    assert!(
        input
            .analyze(
                &Architecture::X86,
                AnalysisLimits::default(),
                &AtomicBool::new(false)
            )
            .is_err()
    );
    input.format = BinaryFormat::Raw;
    assert!(
        input
            .analyze(
                &Architecture::X86_64,
                AnalysisLimits::default(),
                &AtomicBool::new(false)
            )
            .is_err()
    );
}

#[test]
fn long_straight_line_remains_one_complete_block() {
    let mut bytes = vec![0x90; 4_000];
    bytes.push(0xc3);
    let result = analyze(&image(&bytes), AnalysisLimits::default());
    assert!(!result.truncated);
    assert_eq!(result.instruction_count, 4_001);
    assert_eq!(result.functions[0].blocks.len(), 1);
    assert_eq!(
        result.functions[0].blocks[0]
            .instructions
            .last()
            .unwrap()
            .offset,
        4_000
    );
}
