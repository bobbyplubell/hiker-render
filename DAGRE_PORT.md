# Dagre → Rust port plan (`hiker_graph::layered`, crate at hiker-render/graph/)

Faithful pure-Rust reimplementation of dagre.js's layered (Sugiyama) graph layout,
to power flowchart/state/class/ER-style mermaid diagrams. Lives in
`hiker-render/graph/src/layered/`, implementing the `LayoutEngine` trait already
defined in `hiker-render/graph/src/lib.rs`.

## Reference (read-only, vendored, gitignored)
- `references/dagre/lib/**` — the algorithm (TS). ~3,457 LOC.
- `references/dagre/test/**` — per-module tests with expected values baked in. ~3,842 LOC.
- `references/graphlib/lib/graph.ts` (+ `test/graph-test.ts`) — the compound-multigraph data structure dagre runs on.

## Oracle strategy
**Port each module together with its test file.** dagre's tests embed expected
numeric outputs, so porting `foo-test.ts` → Rust `#[test]`s gives unit-level
conformance WITHOUT executing any JS. Reserve Docker (NEVER run the JS on the host)
only for generating end-to-end golden fixtures (full `layout()` on sample graphs) at
the very end — and even then, prefer porting `test/layout-test.ts`.

## Build order (bottom-up; each row = one subagent task, ported WITH its tests, must go green before the next)
| # | Module | Source (lib/) | Test | ~LOC | Target file |
|---|--------|---------------|------|------|-------------|
| 0 | **Graph data structure** (compound multigraph) | graphlib `graph.ts` | graphlib `graph-test.ts` | 860+1015 | `layered/graph.rs` |
| 1 | util helpers | `util.ts` | `util-test.ts` | 359+303 | `layered/util.rs` |
| 2 | data/list | `data/list.ts` | `data/list-test.ts` | 69+59 | `layered/list.rs` |
| 3 | acyclic + greedy-fas (cycle removal) | `acyclic.ts`,`greedy-fas.ts` | `acyclic-test`,`greedy-fas-test` | 214+211 | `layered/acyclic.rs` |
| 4 | rank: util, feasible-tree, network-simplex | `rank/*` | `rank/*-test` | 494+605 | `layered/rank/` |
| 5 | normalize (dummy nodes for long edges) | `normalize.ts` | `normalize-test.ts` | 92+225 | `layered/normalize.rs` |
| 6 | clusters: nesting-graph, parent-dummy-chains, add-border-segments | those 3 | their tests | 286+453 | `layered/nesting.rs` |
| 7 | order (crossing minimization, 9 files) | `order/*` | `order/*-test` | 692+~600 | `layered/order/` |
| 8 | coordinate-system (rankdir transforms) | `coordinate-system.ts` | `coordinate-system-test` | 65+66 | `layered/coordinate_system.rs` |
| 9 | position/bk (Brandes–Köpf — HARDEST) | `position/bk.ts`,`position/index.ts` | `position/bk-test`,`position-test` | 568+735 | `layered/position/` |
| 10 | layout orchestrator (26 passes, self-edges, edge-label proxies, translate, intersects) | `layout.ts` | `layout-test.ts` | 441+308 | `layered/layout.rs` |
| 11 | **`LayeredEngine: LayoutEngine`** — map `GraphInput`(uses `node_sizes`!) → dagre graph → run → `LayoutOutput`(positions + `edge_routes` from edge `points[]` + `size`) | — | new | — | `layered/mod.rs` |

## Notes / decisions
- **Node sizes are mandatory input** (the gap that doomed dagre-rs). `GraphInput.node_sizes`
  feeds dagre node width/height; the layered engine REQUIRES them (fall back to a
  default box only if absent). Output `edge_routes` come from dagre's per-edge `points[]`.
- **Ranker**: port `network-simplex` for fidelity; dagre also has `tight-tree`/`longest-path`
  fallbacks (`ranker` option) — port `longest-path` first (in rank/util) so the pipeline
  runs end-to-end before network-simplex lands, then add network-simplex.
- **Clusters (step 6) are woven through** ordering (subgraph constraints) and the
  orchestrator (border nodes). They can be stubbed to no-ops initially so a
  non-subgraph flowchart works, then filled in — mermaid flowcharts DO use subgraphs,
  so they're required for real coverage, not optional long-term.
- Keep it egui-free / std-only, consistent with the rest of the `hiker-graph` crate.
- Determinism: no `Math.random`/time. dagre is deterministic; keep it so.

## Status
- [x] Step 0 — graph data structure (`layered/graph.rs`, 119 conformance tests from graph-test.ts). Graph<G,N,E>, String node ids, multigraph edge keying, compound parent/children, insertion-order determinism via internal OrderMap. No external deps.
- [x] Step 1 — types + util (`layered/types.rs` + `layered/util.rs`, 32 tests from util-test.ts). `DagreGraph = Graph<GraphLabel,NodeLabel,EdgeLabel>`; enums for rankdir/labelpos/dummy/etc.; util helpers (add_dummy_node, simplify, as_non_compound_graph, succ/pred_weights, intersect_rect, build_layer_matrix, normalize_ranks, remove_empty_ranks, max_rank, partition, range, pick, map_values). idCounter → static AtomicUsize (deterministic).
- [x] Step 2 — data/list (`layered/list.rs`, arena/index-based intrusive list, 6 tests).
- [x] Step 3 — acyclic + greedy-fas (`layered/acyclic.rs` + `greedy_fas.rs`, 20 tests). Edge reversal sets reversed/forward_name; run/undo round-trips. rank input = acyclic graph.
- [x] Step 4 — rank: util(longest_path+slack) + feasible_tree + network_simplex (`layered/rank/`, 44 tests incl. the 500-LOC network-simplex oracle). `rank(&mut DagreGraph)` dispatcher; ranks NOT normalized here (orchestrator calls util::normalize_ranks). TreeGraph{TreeNodeLabel(low/lim/parent), TreeEdgeLabel(cutvalue)}.
- [x] Step 5 — normalize (`layered/normalize.rs`, 12 tests). run() inserts dummy-node chains for multi-rank edges (head→GraphLabel.dummy_chains; each dummy carries edge_obj/edge_label); edge-label dummy at label_rank carries width/height. undo() recovers edge points[].
- [x] Step 6 — clusters: nesting_graph + parent_dummy_chains + add_border_segments (`layered/`, 29 tests). nesting_graph::run before rank / cleanup after; parent_dummy_chains after normalize; add_border_segments before order. Enables subgraph support.
- [x] Step 7 — order (`layered/order/`, 9 sub-modules, 56 tests). `order(&mut DagreGraph, &OrderOptions)`; writes node.order. cross-count accumulator tree, resolve-conflicts merge, sort interleave, sort-subgraph (cluster-aware via compound API). Runs after normalize.
- [x] Step 8 — coordinate-system (`layered/coordinate_system.rs`, 8 tests). adjust()/undo() for rankdir LR/RL/BT (swap w/h, reverse y, swap xy).
- [x] Step 9 — position/bk (`layered/position/`, 49 tests). Brandes–Köpf: type-1/2 conflicts, 4-way vertical alignment, horizontal compaction, align-to-narrowest, median balance. `position::position(&mut DagreGraph)` writes node.x/node.y; runs after order on the (internally as_non_compound) graph; reads ranksep/nodesep/edgesep/align.
- [x] Step 10 — layout orchestrator (`layered/layout.rs`, 20 tests, passes ALL of dagre's layout-test). `layout(&mut DagreGraph)` + `layout_with_opts`. The exact 26-pass pipeline + buildLayoutGraph/updateInputGraph + makeSpaceForEdgeLabels, edge-label proxies, self-edges, translateGraph, assignNodeIntersects, reversePoints.
- [x] Step 11 — `LayeredEngine: LayoutEngine` (`layered/engine.rs`, 7 tests). GraphInput.node_sizes → DagreGraph (string ids, edge idx as multigraph name) → layout() → LayoutOutput{positions, edge_routes from edge points[] in input order, size}. Re-exported `hiker_graph::LayeredEngine`. All three engines (Force/Tree/Layered) now share the crate-root `LayoutEngine` trait.

**✅ DAGRE PORT COMPLETE — all 11 steps done. 411 tests green in `cargo test -p hiker-graph`** (every module ported from dagre's own test files as the conformance oracle; no JS ever executed). Full pure-Rust Sugiyama layout incl. subgraphs/clusters, network-simplex ranking, Brandes–Köpf coordinates, edge routing. Public API: `hiker_graph::LayeredEngine` (impl `LayoutEngine`) for the mermaid renderer; `hiker_graph::layered::layout::layout` for direct DagreGraph use. egui-free, std-only, deterministic. Math tests live separately in the hiker-render crate (no collision).
