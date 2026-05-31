# Migrating hiker-core's graph code onto the `hiker-graph` crate

**Status:** NOT YET DONE in hiker-core. This document is the migration plan for a
future agent. The `hiker-graph` crate is complete and green; this describes the
change to make in `/home/bobby/projects/notes` to delete its duplicated graph
layout code and consume `hiker-graph` instead.

**Crate location:** `hiker-graph` lives at `/home/bobby/projects/html-widget/hiker-render/graph/`
(package name `hiker-graph`; it sits under the `hiker-render/` umbrella dir
alongside `hiker-render/htmlview/`, but is an independent, egui-free, std-only,
dependency-free crate — it does NOT depend on the `hiker-render` math package).

**Why:** hiker-core (`/home/bobby/projects/notes`) hand-rolled force-directed +
tidy-tree graph layout in its `widgets/graph-widgets/` crate because the Rust
ecosystem crates were unsuitable. That same layout capability is now needed by the
planned mermaid-style diagram renderer in `/home/bobby/projects/html-widget`.
Rather than maintain two copies, the layout algorithms were lifted into the
`hiker-graph` crate as the single source of truth. hiker-core should depend on
`hiker-graph` and delete its copies.

---

## 1. What was extracted (the new home)

The `hiker-graph` crate at `hiker-render/graph/src/` (egui-free, std-only, no
external deps). It is a **faithful port** of the two hiker-core source files —
`widgets/graph-widgets/src/graph_layouts.rs` and `.../force_layout.rs` — with
exactly ONE intentional change: the egui `Vec2` dependency was replaced by a local
`hiker_graph::Vec2`. Every ported public type/function kept its **exact
name and signature** (including the unused `_area` params on the tree layouts).
(The crate also hosts the in-progress dagre port under `hiker_graph::layered` — not
relevant to this migration.)

### Final public API of the `hiker-graph` crate

```rust
// vec2.rs
pub struct Vec2 { pub x: f32, pub y: f32 }
impl Vec2 {
    pub const ZERO: Vec2;
    pub const fn new(x: f32, y: f32) -> Self;
    pub fn length(&self) -> f32;
    pub fn length_sq(&self) -> f32;
}
// Operators: Add, Sub, Mul<f32>, AddAssign, SubAssign, Div<f32>
// Conversions: From<(f32,f32)>, From<[f32;2]>, From<Vec2> for [f32;2], From<Vec2> for (f32,f32)

// tree.rs  (verbatim port of graph_layouts.rs)
pub enum LayoutKind { ForceDirected, Radial, VerticalTree, HorizontalTree }
impl LayoutKind { pub const fn label(self) -> &'static str; pub const fn all() -> [LayoutKind; 4]; }
pub struct LayoutTree { pub n: usize, pub children: Vec<Vec<usize>>, pub roots: Vec<usize>,
                        pub depth: Vec<usize>, pub subtree_leaves: Vec<usize> }
impl LayoutTree { pub fn from_parents(parent_of: &[Option<usize>]) -> Self; }
pub fn dfs_tree(n: usize, edges: &[(u32, u32)], root: usize) -> LayoutTree;
pub fn bfs_tree(n: usize, edges: &[(u32, u32)], root: usize) -> LayoutTree;
pub fn radial_positions(tree: &LayoutTree, _area: f32) -> Vec<Vec2>;
pub fn vertical_tree_positions(tree: &LayoutTree, _area: f32) -> Vec<Vec2>;
pub fn horizontal_tree_positions(tree: &LayoutTree, area: f32) -> Vec<Vec2>;

// force.rs  (port of force_layout.rs; LayoutParams fields identical to original)
pub struct LayoutParams { pub bound: f32, pub max_iters: u32, pub theta: f32,
    pub convergence_eps: f32, pub convergence_streak: u32, pub scaling_ratio: f32,
    pub gravity: f32, pub strong_gravity: bool, pub slow_down: f32, pub lin_log: bool,
    pub outbound_attraction_distribution: bool, pub degree_repulsion: bool }
impl Default for LayoutParams { /* identical defaults to the original */ }

#[cfg(not(target_arch = "wasm32"))]
pub struct LayoutWorker { /* private */ }   // native-only (uses std::thread)
#[cfg(not(target_arch = "wasm32"))]
impl LayoutWorker {
    pub fn spawn(initial: Vec<Vec2>, edges: Vec<(u32, u32)>, params: LayoutParams) -> Self;
    pub fn snapshot_into(&self, out: &mut Vec<Vec2>);
    pub fn is_running(&self) -> bool;
    pub fn iters_done(&self) -> u32;
}
// impl Drop for LayoutWorker

// NEW (additive — synchronous entry points, used by the mermaid renderer; hiker-core may ignore)
pub fn force_to_convergence(initial: Vec<Vec2>, edges: &[(u32,u32)], params: &LayoutParams,
                            should_stop: impl Fn() -> bool) -> Vec<Vec2>;
pub fn force_layout(initial: Vec<Vec2>, edges: &[(u32,u32)], params: &LayoutParams) -> Vec<Vec2>;

// mod.rs — forward-looking generic abstraction (mermaid + future dagre engine use this)
pub struct GraphInput<'a> { pub node_count: usize, pub edges: &'a [(u32,u32)],
                            pub node_sizes: Option<&'a [Vec2]>, pub directed: bool }
pub struct LayoutOutput { pub positions: Vec<Vec2>, pub edge_routes: Vec<Vec<Vec2>>, pub size: Vec2 }
pub trait LayoutEngine { fn layout(&self, input: &GraphInput<'_>) -> LayoutOutput; }
pub struct ForceEngine { pub params: LayoutParams, pub seed: Option<Vec<Vec2>> }  // impl LayoutEngine
pub struct TreeEngine  { pub kind: LayoutKind, pub area: f32 }                    // impl LayoutEngine
```

Crate-root re-exports (`hiker_graph::*`): `Vec2, LayoutKind, LayoutTree,
dfs_tree, bfs_tree, radial_positions, vertical_tree_positions, horizontal_tree_positions,
LayoutParams, force_layout, force_to_convergence, GraphInput, LayoutOutput, LayoutEngine,
ForceEngine, TreeEngine`. `LayoutWorker` is reachable as `hiker_graph::LayoutWorker`
(not crate-root re-exported, because it is native-only-gated).

**Single source of truth for the FA2 physics:** `hiker-render/graph/src/force.rs`,
private `run_fa2` (≈line 169); `force_to_convergence`, `force_layout`, and
`LayoutWorker::spawn` all route through it — no duplicated force math.

---

## 2. The ONLY real friction: `Vec2`

hiker-core's call sites use `egui::Vec2`; the extracted code uses
`hiker_graph::Vec2`. Both are `{ x: f32, y: f32 }`. `LayoutWorker::spawn`
takes `Vec<hiker_graph::Vec2>` and `snapshot_into` writes into
`Vec<hiker_graph::Vec2>`. So hiker-core must convert at the boundary.

Bridges provided on the `hiker_graph::Vec2` (use the array bridge — egui's Vec2 has
`.x`/`.y` and a `From<[f32;2]>`):
- egui → hiker-graph: `hiker_graph::Vec2::from([ev.x, ev.y])`
- hiker-graph → egui: `egui::Vec2::from(<[f32;2]>::from(hv))`  (i.e. `egui::vec2(hv.x, hv.y)`)

**Recommended approach:** make hiker-core's graph panels store positions as
`Vec<hiker_graph::Vec2>` end-to-end and only convert to `egui::Vec2` at the
actual egui paint calls (`painter.circle`, `to_screen * pos`, etc.). That confines
conversion to the few draw sites instead of threading two vector types. A tiny local
helper `fn to_egui(v: hiker_graph::Vec2) -> egui::Vec2 { egui::vec2(v.x, v.y) }`
(and inverse) in each panel keeps it readable.

---

## 3. Concrete edit list in `/home/bobby/projects/notes`

### 3a. Add the dependency
- In the workspace `Cargo.toml`, add `hiker-graph` (path dep to
  `../html-widget/hiker-render/graph`, or whatever the agreed location/publish form is).
  `hiker-graph` is egui-free and dependency-free, so there is no egui version
  conflict; conversion to `egui::Vec2` lives entirely on the hiker-core side.
- Add `hiker-graph.workspace = true` (or direct dep) to:
  - `widgets/graph-widgets/Cargo.toml` (if graph-widgets is kept as a thin binding — see 3c), OR
  - `app/Cargo.toml` and `tools/graph-snapshot/Cargo.toml` directly (if graph-widgets is deleted).

### 3b. Delete the duplicated source
- `widgets/graph-widgets/src/graph_layouts.rs`  → delete (now `hiker_graph::tree`)
- `widgets/graph-widgets/src/force_layout.rs`    → delete (now `hiker_graph::force`)

### 3c. Two migration shapes — pick one
**Option A — keep `graph-widgets` as a thin re-export shim (least churn):**
Replace `widgets/graph-widgets/src/lib.rs` with re-exports so existing
`use graph_widgets::force_layout::{...}` / `graph_widgets::graph_layouts::{...}`
paths keep resolving:
```rust
pub mod force_layout { pub use hiker_graph::{LayoutParams, LayoutWorker,
    force_layout, force_to_convergence}; }
pub mod graph_layouts { pub use hiker_graph::{LayoutKind, LayoutTree, dfs_tree,
    bfs_tree, radial_positions, vertical_tree_positions, horizontal_tree_positions}; }
```
Then only the `Vec2` type at call sites changes (see 3d). Smallest diff; keeps the
"cross-crate so clippy::single_call_fn is exempt" property the graph-snapshot tool
comment relies on.

**Option B — delete `graph-widgets` entirely:** update the three consumers to
`use hiker_graph::{...}` directly and drop the crate from the workspace
members + the `app`/`graph-snapshot` deps. Cleaner long-term, larger diff.

### 3d. Fix the three consumers' `Vec2`
These files currently hold `Vec<egui::Vec2>` for positions and pass them to
`LayoutWorker::spawn` / receive from `snapshot_into`:
- `app/src/panels/graph.rs` (imports at lines ~24–27; `LayoutWorker::spawn` ~637;
  `snapshot_into` ~148; tree fns ~652–658)
- `app/src/panels/cluster_graph.rs` (imports ~19–22; `spawn` ~478; `snapshot_into`
  ~225; `LayoutTree::from_parents` ~439; tree fns ~443–449)
- `tools/graph-snapshot/src/main.rs` (imports ~15–17; `spawn` ~361; `snapshot_into`
  ~383; tree fns ~405–411)

For each: change the position storage to `Vec<hiker_graph::Vec2>` (or convert
at the boundary) and convert to `egui::Vec2` only at paint/`to_screen` time per §2.
The seed-position construction (the initial `Vec<Vec2>` passed to `spawn`) must build
hiker-graph `Vec2`s. `LayoutParams`, `LayoutKind`, `LayoutTree`, and every layout fn
are otherwise call-compatible with no signature change.

### 3e. Verify on the hiker-core side
- `cargo build` and `cargo test` for the notes workspace.
- Run `tools/graph-snapshot` and eyeball a PNG for each layout kind (`force`,
  `radial`, `vertical`, `horizontal`) — output must match pre-migration (the physics
  and tree math are byte-for-byte the same algorithm; only the vector type changed).
- Smoke-test the app's graph panel + cluster-graph panel: force layout animates and
  converges; tree/radial layouts place nodes identically to before.

---

## 4. Things to know / gotchas
- **Determinism preserved:** no randomness was introduced. `ForceEngine`'s default
  seed (when `seed: None`) is a deterministic circle; hiker-core supplies its own
  seed via `spawn`, so this does not affect it.
- **wasm:** `LayoutWorker` is `#[cfg(not(target_arch = "wasm32"))]`. If hiker-core
  ever builds the graph panels for wasm, it must use the synchronous
  `force_to_convergence` instead of the threaded worker. (Native app builds are
  unaffected.)
- **Don't add an egui dep to `hiker-graph`.** The whole point is that it stays
  egui-agnostic; the conversion lives on the hiker-core side.
- **`hiker-graph` has no features or external deps** — depending on it pulls in only
  std. (It is a separate crate from the `hiker-render` LaTeX-math package; you do NOT
  need or want hiker-render for the graph migration.)
- **No behavior change expected.** The extraction report confirmed: ported items have
  zero signature/name changes; the only code motion was factoring the worker loop body
  into the shared `run_fa2`, with the `thread::sleep(100µs)` yield and snapshot writes
  preserved.
