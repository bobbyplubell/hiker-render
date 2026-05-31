//! Parent dummy chains — a port of `dagre/lib/parent-dummy-chains.ts`.
//!
//! After [`normalize::run`](super::normalize::run) has broken long edges into
//! chains of dummy nodes, each chain must be reparented so that its nodes fall
//! into the correct clusters as the chain passes through the compound
//! structure. This walks each chain from tail to head, tracking the path
//! through the lowest common ancestor (LCA) of the original edge's endpoints,
//! and assigns each dummy the appropriate cluster parent.

use std::collections::HashMap;

use super::types::DagreGraph;
use super::util::GRAPH_NODE;

#[derive(Clone, Copy)]
struct PostorderNum {
    low: i32,
    lim: i32,
}

struct PathData {
    path: Vec<Option<String>>,
    lca: Option<String>,
}

/// `parentDummyChains(graph)` — reparent every dummy chain into its clusters.
pub fn parent_dummy_chains(graph: &mut DagreGraph) {
    let postorder_nums = postorder(graph);

    let dummy_chains: Vec<String> = graph
        .graph()
        .and_then(|g| g.dummy_chains.clone())
        .unwrap_or_default();

    for start in dummy_chains {
        let mut v = start;
        let edge_obj = graph
            .node(&v)
            .and_then(|n| n.edge_obj.clone())
            .expect("dummy chain node missing edgeObj");
        let path_data = find_path(graph, &postorder_nums, &edge_obj.v, &edge_obj.w);
        let path = path_data.path;
        let lca = path_data.lca;
        let mut path_idx: usize = 0;
        let mut path_v: Option<String> = path.get(path_idx).cloned().flatten();
        let mut ascending = true;

        while v != edge_obj.w {
            // Accessed lazily: in the no-parent case `node.rank` may be unset
            // and must never be read (mirrors TS `node.rank!` being evaluated
            // only inside the comparisons below).
            let node_rank = || graph.node(&v).and_then(|n| n.rank).unwrap();

            if ascending {
                loop {
                    path_v = path.get(path_idx).cloned().flatten();
                    if path_v == lca {
                        break;
                    }
                    let max_rank = graph
                        .node(path_v.as_ref().unwrap())
                        .and_then(|n| n.max_rank)
                        .unwrap();
                    if max_rank < node_rank() {
                        path_idx += 1;
                    } else {
                        break;
                    }
                }

                if path_v == lca {
                    ascending = false;
                }
            }

            if !ascending {
                while path_idx < path.len().saturating_sub(1) {
                    let next = path[path_idx + 1].as_ref().unwrap();
                    let min_rank = graph.node(next).and_then(|n| n.min_rank).unwrap();
                    if min_rank <= node_rank() {
                        path_idx += 1;
                    } else {
                        break;
                    }
                }
                path_v = path.get(path_idx).cloned().flatten();
            }

            if let Some(pv) = &path_v {
                graph.set_parent(v.clone(), pv.clone());
            }
            v = graph.successors(&v).unwrap()[0].clone();
        }
    }
}

/// Find a path from `v` to `w` through the lowest common ancestor (LCA).
/// Return the full path and the LCA.
fn find_path(
    graph: &DagreGraph,
    postorder_nums: &HashMap<String, PostorderNum>,
    v: &str,
    w: &str,
) -> PathData {
    let mut v_path: Vec<Option<String>> = Vec::new();
    let mut w_path: Vec<Option<String>> = Vec::new();
    let low = postorder_nums[v].low.min(postorder_nums[w].low);
    let lim = postorder_nums[v].lim.max(postorder_nums[w].lim);

    // Traverse up from v to find the LCA.
    let mut parent: Option<String> = Some(v.to_string());
    loop {
        parent = parent.as_ref().and_then(|p| graph.parent(p));
        v_path.push(parent.clone());
        match &parent {
            Some(p) => {
                let pn = &postorder_nums[p];
                if pn.low > low || lim > pn.lim {
                    continue;
                }
                break;
            }
            None => break,
        }
    }
    let lca = parent;

    // Traverse from w to LCA.
    let mut w_parent: Option<String> = Some(w.to_string());
    loop {
        w_parent = w_parent.as_ref().and_then(|p| graph.parent(p));
        if w_parent == lca {
            break;
        }
        w_path.push(w_parent.clone());
    }

    w_path.reverse();
    let mut path = v_path;
    path.extend(w_path);
    PathData { path, lca }
}

fn postorder(graph: &DagreGraph) -> HashMap<String, PostorderNum> {
    let mut result: HashMap<String, PostorderNum> = HashMap::new();
    let mut lim: i32 = 0;

    fn dfs(
        graph: &DagreGraph,
        result: &mut HashMap<String, PostorderNum>,
        lim: &mut i32,
        v: &str,
    ) {
        let low = *lim;
        for child in graph.children(v) {
            dfs(graph, result, lim, &child);
        }
        result.insert(
            v.to_string(),
            PostorderNum {
                low,
                lim: *lim,
            },
        );
        *lim += 1;
    }

    for v in graph.children(GRAPH_NODE) {
        dfs(graph, &mut result, &mut lim, &v);
    }

    result
}

#[cfg(test)]
mod tests;
