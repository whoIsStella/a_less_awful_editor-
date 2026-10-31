//! Call-site relationships within one bounded analysis snapshot.
use crate::{Analysis, EdgeKind, EdgeResolution};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite {
    /// Function indices refer only to the Analysis used to build this graph.
    pub caller: usize,
    pub callee: Option<usize>,
    pub address: u64,
    pub offset: usize,
    pub target_address: Option<u64>,
    pub resolution: EdgeResolution,
}

#[derive(Clone, Debug, Default)]
pub struct CallGraph {
    pub sites: Vec<CallSite>,
    /// Indices into `sites`, indexed by function. Shared-tail sites may appear
    /// under multiple candidate callers; ownership is not inferred here.
    pub incoming: Vec<Vec<usize>>,
    pub outgoing: Vec<Vec<usize>>,
}

impl Analysis {
    pub fn call_graph(&self, cancelled: &AtomicBool) -> Result<CallGraph, String> {
        let check = || {
            if cancelled.load(Ordering::Relaxed) {
                Err("Call relationship analysis cancelled".to_owned())
            } else {
                Ok(())
            }
        };
        check()?;
        let entries: BTreeMap<_, _> = self
            .functions
            .iter()
            .enumerate()
            .map(|(index, function)| ((function.address, function.offset), index))
            .collect();
        let mut graph = CallGraph {
            sites: Vec::new(),
            incoming: vec![Vec::new(); self.functions.len()],
            outgoing: vec![Vec::new(); self.functions.len()],
        };
        for (caller, function) in self.functions.iter().enumerate() {
            check()?;
            for block in &function.blocks {
                for edge in &block.edges {
                    check()?;
                    if !matches!(edge.kind, EdgeKind::Call | EdgeKind::IndirectCall) {
                        continue;
                    }
                    let callee = if edge.kind == EdgeKind::Call
                        && edge.resolution == EdgeResolution::Resolved
                    {
                        edge.target_address
                            .zip(edge.target_offset)
                            .and_then(|entry| entries.get(&entry).copied())
                    } else {
                        None
                    };
                    let index = graph.sites.len();
                    graph.outgoing[caller].push(index);
                    if let Some(callee) = callee {
                        graph.incoming[callee].push(index);
                    }
                    graph.sites.push(CallSite {
                        caller,
                        callee,
                        address: edge.from_address,
                        offset: edge.from_offset,
                        target_address: edge.target_address,
                        resolution: edge.resolution,
                    });
                }
            }
        }
        Ok(graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnalyzedFunction, BasicBlock, FlowEdge, FunctionProvenance};
    fn function(address: u64, edges: Vec<FlowEdge>) -> AnalyzedFunction {
        AnalyzedFunction {
            name: format!("f_{address:x}"),
            address,
            offset: address as usize,
            provenance: FunctionProvenance::ExecutableSymbol,
            blocks: vec![BasicBlock {
                start_address: address,
                start_offset: address as usize,
                instructions: vec![],
                edges,
            }],
        }
    }
    fn edge(
        from: u64,
        target: Option<u64>,
        kind: EdgeKind,
        resolution: EdgeResolution,
    ) -> FlowEdge {
        FlowEdge {
            from_address: from,
            from_offset: from as usize,
            target_address: target,
            target_offset: target.map(|a| a as usize),
            kind,
            resolution,
        }
    }
    fn analysis(functions: Vec<AnalyzedFunction>) -> Analysis {
        Analysis {
            functions,
            references: vec![],
            warnings: vec![],
            truncated: false,
            instruction_count: 0,
        }
    }
    #[test]
    fn direct_calls_recursion_and_multiple_sites_keep_exact_locations() {
        let analysis = analysis(vec![
            function(
                10,
                vec![
                    edge(11, Some(20), EdgeKind::Call, EdgeResolution::Resolved),
                    edge(12, Some(20), EdgeKind::Call, EdgeResolution::Resolved),
                ],
            ),
            function(
                20,
                vec![edge(21, Some(20), EdgeKind::Call, EdgeResolution::Resolved)],
            ),
        ]);
        let graph = analysis.call_graph(&AtomicBool::new(false)).unwrap();
        assert_eq!(graph.outgoing, vec![vec![0, 1], vec![2]]);
        assert_eq!(graph.incoming, vec![vec![], vec![0, 1, 2]]);
        assert_eq!(
            graph.sites.iter().map(|s| s.offset).collect::<Vec<_>>(),
            [11, 12, 21]
        );
        assert!(graph.sites.iter().all(|s| s.callee == Some(1)));
    }
    #[test]
    fn unknown_targets_limits_and_non_call_edges_do_not_invent_callees() {
        let analysis = analysis(vec![function(
            10,
            vec![
                edge(11, None, EdgeKind::IndirectCall, EdgeResolution::Indirect),
                edge(12, Some(30), EdgeKind::Call, EdgeResolution::Limit),
                edge(13, Some(10), EdgeKind::Jump, EdgeResolution::Resolved),
                edge(14, Some(15), EdgeKind::Call, EdgeResolution::Resolved),
            ],
        )]);
        let graph = analysis.call_graph(&AtomicBool::new(false)).unwrap();
        assert_eq!(graph.sites.len(), 3);
        assert!(graph.sites.iter().all(|s| s.callee.is_none()));
        assert_eq!(graph.sites[1].target_address, Some(30));
        assert_eq!(graph.sites[1].resolution, EdgeResolution::Limit);
        assert!(graph.incoming[0].is_empty());
    }
    #[test]
    fn shared_tail_preserves_candidate_owners_and_cancellation_is_explicit() {
        let call = edge(15, Some(30), EdgeKind::Call, EdgeResolution::Resolved);
        let analysis = analysis(vec![
            function(10, vec![call.clone()]),
            function(12, vec![call]),
            function(30, vec![]),
        ]);
        let graph = analysis.call_graph(&AtomicBool::new(false)).unwrap();
        assert_eq!(graph.sites[0].caller, 0);
        assert_eq!(graph.sites[1].caller, 1);
        assert_eq!(graph.incoming[2], [0, 1]);
        assert_eq!(graph.sites[0].offset, graph.sites[1].offset);
        assert!(analysis.call_graph(&AtomicBool::new(true)).is_err());
    }
}
