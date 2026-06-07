//! Constraint-conflict resolution — a port of
//! `dagre/lib/order/resolve-conflicts.ts`.
//!
//! Given barycenter entries and a constraint graph, coalesces entries that
//! would violate a constraint into merged super-entries that aggregate
//! barycenter and weight. Based on Forster, "A Fast and Simple Heuristic for
//! Constrained Two-Level Crossing Reduction".
//!
//! # Fidelity notes
//!
//! The TS uses object references for the `in`/`out` adjacency, a `merged` flag,
//! and mutates entries in place. Here entries live in a `Vec` and reference one
//! another by index. The `sourceSet` is a stack (`pop` from the end); `in` is
//! reversed before the merge sweep; `mergeEntries` concatenates `source.vs`
//! before `target.vs` and takes the min `i`. All of this is asserted by the
//! oracle tests (merged `vs` order, `barycenter`, `weight`, `i`).

use super::barycenter::BarycenterEntry;
use super::graph::Graph;

/// A resolved entry — TS `{vs, i, barycenter?, weight?}`.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedEntry {
    pub vs: Vec<String>,
    pub i: usize,
    pub barycenter: Option<f64>,
    pub weight: Option<f64>,
}

#[derive(Clone)]
struct MappedEntry {
    indegree: i64,
    in_: Vec<usize>,
    out: Vec<usize>,
    vs: Vec<String>,
    i: usize,
    barycenter: Option<f64>,
    weight: Option<f64>,
    merged: bool,
}

/// `resolveConflicts(entries, constraintGraph)`.
pub fn resolve_conflicts<CgG, CgN, CgE>(
    entries: &[BarycenterEntry],
    constraint_graph: &Graph<CgG, CgN, CgE>,
) -> Vec<ResolvedEntry> {
    use std::collections::HashMap;

    // v -> index into `mapped`, preserving insertion (entry) order.
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut mapped: Vec<MappedEntry> = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        let tmp = MappedEntry {
            indegree: 0,
            in_: Vec::new(),
            out: Vec::new(),
            vs: vec![entry.v.clone()],
            i,
            barycenter: entry.barycenter,
            weight: entry.weight,
            merged: false,
        };
        index_of.insert(entry.v.clone(), i);
        mapped.push(tmp);
    }

    for e in constraint_graph.edges() {
        let ev = index_of.get(&e.v).copied();
        let ew = index_of.get(&e.w).copied();
        if let (Some(iv), Some(iw)) = (ev, ew) {
            mapped[iw].indegree += 1;
            mapped[iv].out.push(iw);
        }
    }

    // sourceSet = entries with no indegree, in insertion order.
    let source_set: Vec<usize> = mapped
        .iter()
        .enumerate()
        .filter(|(_, e)| e.indegree == 0)
        .map(|(i, _)| i)
        .collect();

    do_resolve_conflicts(&mut mapped, source_set)
}

fn do_resolve_conflicts(mapped: &mut [MappedEntry], mut source_set: Vec<usize>) -> Vec<ResolvedEntry> {
    let mut entries: Vec<usize> = Vec::new();

    while let Some(entry) = source_set.pop() {
        entries.push(entry);

        // entry.in.reverse().forEach(handleIn(entry))
        let mut in_list = mapped[entry].in_.clone();
        in_list.reverse();
        for u in in_list {
            if mapped[u].merged {
                continue;
            }
            let u_bc = mapped[u].barycenter;
            let v_bc = mapped[entry].barycenter;
            let should_merge = match (u_bc, v_bc) {
                (Some(ub), Some(vb)) => ub >= vb,
                _ => true, // undefined barycenter always violates
            };
            if should_merge {
                merge_entries(mapped, entry, u);
            }
        }

        // entry.out.forEach(handleOut(entry))
        let out_list = mapped[entry].out.clone();
        for w in out_list {
            mapped[w].in_.push(entry);
            mapped[w].indegree -= 1;
            if mapped[w].indegree == 0 {
                source_set.push(w);
            }
        }
    }

    entries
        .into_iter()
        .filter(|&i| !mapped[i].merged)
        .map(|i| ResolvedEntry {
            vs: mapped[i].vs.clone(),
            i: mapped[i].i,
            barycenter: mapped[i].barycenter,
            weight: mapped[i].weight,
        })
        .collect()
}

fn merge_entries(mapped: &mut [MappedEntry], target: usize, source: usize) {
    let mut sum = 0.0_f64;
    let mut weight = 0.0_f64;

    if let Some(tw) = mapped[target].weight {
        if tw != 0.0 {
            sum += mapped[target].barycenter.unwrap() * tw;
            weight += tw;
        }
    }
    if let Some(sw) = mapped[source].weight {
        if sw != 0.0 {
            sum += mapped[source].barycenter.unwrap() * sw;
            weight += sw;
        }
    }

    let mut new_vs = mapped[source].vs.clone();
    new_vs.extend(mapped[target].vs.clone());
    mapped[target].vs = new_vs;
    mapped[target].barycenter = Some(sum / weight);
    mapped[target].weight = Some(weight);
    mapped[target].i = mapped[source].i.min(mapped[target].i);
    mapped[source].merged = true;
}

#[cfg(test)]
mod tests {
    use super::super::new_constraint_graph;
    use super::*;

    fn bc(v: &str, b: f64, w: f64) -> BarycenterEntry {
        BarycenterEntry {
            v: v.into(),
            barycenter: Some(b),
            weight: Some(w),
        }
    }
    fn bare(v: &str) -> BarycenterEntry {
        BarycenterEntry {
            v: v.into(),
            barycenter: None,
            weight: None,
        }
    }

    fn sort_func(mut r: Vec<ResolvedEntry>) -> Vec<ResolvedEntry> {
        r.sort_by(|a, b| a.vs[0].cmp(&b.vs[0]));
        r
    }

    #[test]
    fn returns_nodes_unchanged_when_no_constraints() {
        let input = vec![bc("a", 2.0, 3.0), bc("b", 1.0, 2.0)];
        let cg = new_constraint_graph();
        assert_eq!(
            sort_func(resolve_conflicts(&input, &cg)),
            vec![
                ResolvedEntry { vs: vec!["a".into()], i: 0, barycenter: Some(2.0), weight: Some(3.0) },
                ResolvedEntry { vs: vec!["b".into()], i: 1, barycenter: Some(1.0), weight: Some(2.0) },
            ]
        );
    }

    #[test]
    fn returns_nodes_unchanged_when_no_conflicts() {
        let input = vec![bc("a", 2.0, 3.0), bc("b", 1.0, 2.0)];
        let mut cg = new_constraint_graph();
        cg.set_edge("b", "a", (), None);
        assert_eq!(
            sort_func(resolve_conflicts(&input, &cg)),
            vec![
                ResolvedEntry { vs: vec!["a".into()], i: 0, barycenter: Some(2.0), weight: Some(3.0) },
                ResolvedEntry { vs: vec!["b".into()], i: 1, barycenter: Some(1.0), weight: Some(2.0) },
            ]
        );
    }

    #[test]
    fn coalesces_nodes_when_conflict() {
        let input = vec![bc("a", 2.0, 3.0), bc("b", 1.0, 2.0)];
        let mut cg = new_constraint_graph();
        cg.set_edge("a", "b", (), None);
        assert_eq!(
            resolve_conflicts(&input, &cg),
            vec![ResolvedEntry {
                vs: vec!["a".into(), "b".into()],
                i: 0,
                barycenter: Some((3.0 * 2.0 + 2.0 * 1.0) / (3.0 + 2.0)),
                weight: Some(3.0 + 2.0),
            }]
        );
    }

    #[test]
    fn coalesces_nodes_when_conflict_2() {
        let input = vec![
            bc("a", 4.0, 1.0),
            bc("b", 3.0, 1.0),
            bc("c", 2.0, 1.0),
            bc("d", 1.0, 1.0),
        ];
        let mut cg = new_constraint_graph();
        cg.set_path(&["a", "b", "c", "d"], ());
        assert_eq!(
            resolve_conflicts(&input, &cg),
            vec![ResolvedEntry {
                vs: vec!["a".into(), "b".into(), "c".into(), "d".into()],
                i: 0,
                barycenter: Some((4.0 + 3.0 + 2.0 + 1.0) / 4.0),
                weight: Some(4.0),
            }]
        );
    }

    #[test]
    fn multiple_constraints_same_target_1() {
        let input = vec![bc("a", 4.0, 1.0), bc("b", 3.0, 1.0), bc("c", 2.0, 1.0)];
        let mut cg = new_constraint_graph();
        cg.set_edge("a", "c", (), None);
        cg.set_edge("b", "c", (), None);
        let results = resolve_conflicts(&input, &cg);
        assert_eq!(results.len(), 1);
        let r = &results[0];
        let ci = r.vs.iter().position(|x| x == "c").unwrap();
        let ai = r.vs.iter().position(|x| x == "a").unwrap();
        let bi = r.vs.iter().position(|x| x == "b").unwrap();
        assert!(ci > ai);
        assert!(ci > bi);
        assert_eq!(r.i, 0);
        assert_eq!(r.barycenter, Some((4.0 + 3.0 + 2.0) / 3.0));
        assert_eq!(r.weight, Some(3.0));
    }

    #[test]
    fn multiple_constraints_same_target_2() {
        let input = vec![
            bc("a", 4.0, 1.0),
            bc("b", 3.0, 1.0),
            bc("c", 2.0, 1.0),
            bc("d", 1.0, 1.0),
        ];
        let mut cg = new_constraint_graph();
        cg.set_edge("a", "c", (), None);
        cg.set_edge("a", "d", (), None);
        cg.set_edge("b", "c", (), None);
        cg.set_edge("c", "d", (), None);
        let results = resolve_conflicts(&input, &cg);
        assert_eq!(results.len(), 1);
        let r = &results[0];
        let pos = |x: &str| r.vs.iter().position(|y| y == x).unwrap();
        assert!(pos("c") > pos("a"));
        assert!(pos("c") > pos("b"));
        assert!(pos("d") > pos("c"));
        assert_eq!(r.i, 0);
        assert_eq!(r.barycenter, Some((4.0 + 3.0 + 2.0 + 1.0) / 4.0));
        assert_eq!(r.weight, Some(4.0));
    }

    #[test]
    fn does_nothing_to_node_lacking_barycenter_and_constraint() {
        let input = vec![bare("a"), bc("b", 1.0, 2.0)];
        let cg = new_constraint_graph();
        assert_eq!(
            sort_func(resolve_conflicts(&input, &cg)),
            vec![
                ResolvedEntry { vs: vec!["a".into()], i: 0, barycenter: None, weight: None },
                ResolvedEntry { vs: vec!["b".into()], i: 1, barycenter: Some(1.0), weight: Some(2.0) },
            ]
        );
    }

    #[test]
    fn node_without_barycenter_always_violates_1() {
        let input = vec![bare("a"), bc("b", 1.0, 2.0)];
        let mut cg = new_constraint_graph();
        cg.set_edge("a", "b", (), None);
        assert_eq!(
            resolve_conflicts(&input, &cg),
            vec![ResolvedEntry {
                vs: vec!["a".into(), "b".into()],
                i: 0,
                barycenter: Some(1.0),
                weight: Some(2.0),
            }]
        );
    }

    #[test]
    fn node_without_barycenter_always_violates_2() {
        let input = vec![bare("a"), bc("b", 1.0, 2.0)];
        let mut cg = new_constraint_graph();
        cg.set_edge("b", "a", (), None);
        assert_eq!(
            resolve_conflicts(&input, &cg),
            vec![ResolvedEntry {
                vs: vec!["b".into(), "a".into()],
                i: 0,
                barycenter: Some(1.0),
                weight: Some(2.0),
            }]
        );
    }

    #[test]
    fn ignores_edges_not_related_to_entries() {
        let input = vec![bc("a", 2.0, 3.0), bc("b", 1.0, 2.0)];
        let mut cg = new_constraint_graph();
        cg.set_edge("c", "d", (), None);
        assert_eq!(
            sort_func(resolve_conflicts(&input, &cg)),
            vec![
                ResolvedEntry { vs: vec!["a".into()], i: 0, barycenter: Some(2.0), weight: Some(3.0) },
                ResolvedEntry { vs: vec!["b".into()], i: 1, barycenter: Some(1.0), weight: Some(2.0) },
            ]
        );
    }
}
