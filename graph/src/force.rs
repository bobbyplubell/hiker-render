//! Force-directed graph layout (ForceAtlas2 model).
//!
//! The synchronous [`force_layout`] / [`force_to_convergence`] entry points
//! run the FA2 iteration to convergence on the calling thread. The
//! native-only [`LayoutWorker`] drives the *same* physics on a background
//! thread, publishing position snapshots through an
//! `Arc<RwLock<Vec<Vec2>>>`. Repulsion uses a Barnes–Hut quadtree
//! (O(n log n) per iteration).
//!
//! The force model and convergence mechanism follow ForceAtlas2 — see
//! Jacomy et al. 2014 ("ForceAtlas2, a Continuous Graph Layout Algorithm
//! for Handy Network Visualization") and the canonical implementation
//! in `graphology-layout-forceatlas2`. The earlier port used classic
//! Fruchterman–Reingold with a linear temperature schedule, which
//! oscillated indefinitely on real graphs: once near equilibrium, small
//! flip-flopping forces are indistinguishable from genuine drift, so
//! the global cooling schedule can't stabilize them.
//!
//! FA2's fix is **per-node adaptive damping** computed each step from
//! two scalars derived from the current and previous force vectors:
//!
//! * **swinging**  = |F_prev − F_curr| (large when direction flips)
//! * **traction**  = |F_prev + F_curr| (large when direction is consistent)
//!
//! and `nodespeed = convergence · log(1 + traction) / (1 + √swinging)`.
//! Oscillating nodes (F_prev ≈ −F_curr) get `traction ≈ 0` and
//! therefore `nodespeed ≈ 0` — they literally stop. Nodes drifting
//! steadily get fast nodespeed. This is the only known practical
//! convergence mechanism for force-directed layouts at this scale.
//!
//! Other FA2 ingredients ported here:
//! * Mass-weighted repulsion: `mass(v) = deg(v) + 1`, so hubs repel
//!   harder than leaves and the hub-and-spoke "exploded flower"
//!   pattern collapses into a tight cluster.
//! * Gravity toward origin (configurable strong/weak), so disconnected
//!   components don't drift to infinity.
//! * Outbound-attraction-distribution: edge attraction divided by the
//!   source's mass, so hubs don't get yanked around by their many
//!   leaves.

use super::vec2::Vec2;

/// Tunable parameters for the ForceAtlas2 layout. The defaults match
/// what graphology-layout-forceatlas2 uses for force-directed graph
/// visuals (modest gravity, linear attraction, degree-distributed
/// outbound attraction so hubs don't get dragged around).
pub struct LayoutParams {
    /// Hard clamp on position magnitude per axis. Layouts that escape
    /// this box get pulled back — useful as a safety belt while a
    /// poorly seeded graph is settling.
    pub bound: f32,
    /// Maximum iterations before the worker gives up. FA2 typically
    /// settles in 300–800 steps for graphs in our size range.
    pub max_iters: u32,
    /// Barnes–Hut opening criterion. Larger = faster, less accurate.
    pub theta: f32,
    /// Stop once **global** swinging stays below this for
    /// `convergence_streak` consecutive iterations. Global swinging is
    /// the sum of per-node swinging values; when it's small, the
    /// system has stopped oscillating.
    pub convergence_eps: f32,
    pub convergence_streak: u32,

    // ── FA2 knobs ──────────────────────────────────────────────────
    /// Global multiplier on repulsion and gravity. Default 1.0. Bigger
    /// = more spread-out layout.
    pub scaling_ratio: f32,
    /// Pull toward origin. 0 = none. Default 1.0 produces a compact
    /// layout; raise to keep big graphs from spreading too wide.
    pub gravity: f32,
    /// Strong gravity grows with distance (≈ linear pull). Weak gravity
    /// is constant magnitude regardless of distance. Strong is better
    /// for highly disconnected vaults; weak is gentler.
    pub strong_gravity: bool,
    /// Damping divisor on the applied step. 1.0 = no damping; 5–10 =
    /// safer but slower convergence. Higher slow_down trades speed for
    /// stability when the seed is very far from equilibrium.
    pub slow_down: f32,
    /// LinLog attraction: force ∝ log(1+dist) instead of constant per
    /// edge. Produces sharper cluster separation but takes longer to
    /// converge. Off by default; flip on for "discrete clusters" look.
    pub lin_log: bool,
    /// Divide each edge's attraction by the source's mass. Stops
    /// high-degree hubs from being jerked around by their many leaves.
    pub outbound_attraction_distribution: bool,
    /// Use `deg+1` as the per-node mass in repulsion. Fixes the
    /// hub-and-spoke "exploded flower" pattern by making hubs repel
    /// each other much harder than their satellite leaves repel each
    /// other.
    pub degree_repulsion: bool,

    /// Weak spring constant pulling each *anchored* node toward its
    /// anchor position (the position it held in the previous layout).
    /// This is the knob for **temporal layout stability**: when a graph
    /// is re-solved after nodes are added/removed, retained nodes are
    /// anchored to where they were, so the layout morphs coherently
    /// instead of reshuffling. `0.0` = off (no anchoring); the anchor
    /// force is skipped entirely and the solver behaves exactly as it
    /// did before anchors existed. Typical useful range is small
    /// (`~0.01`–`0.2`): big enough to tether retained nodes, small
    /// enough that newly added nodes can still pull the layout into a
    /// sensible new shape. Only takes effect when anchors are supplied
    /// (e.g. via [`force_to_convergence_anchored`]).
    pub anchor_stiffness: f32,
}

impl Default for LayoutParams {
    fn default() -> Self {
        // `scaling_ratio=100` keeps repulsion strong enough that the
        // settled graph spans ~100s of world units, which our zoom-defaults
        // render at reasonable pixel sizes. Dropping it lower collapses
        // everything onto each other in the panel.
        //
        // `outbound_attraction_distribution=false` is the load-bearing
        // change vs. the prior defaults: with it on, every edge's spring
        // is divided by the source's mass, so a high-degree hub's pull
        // on each child is `1/mass` — its leaves don't have a consistent
        // radius and drift outward into a scrappy blob. With it off,
        // the spring stiffness is uniform per edge and the leaves
        // settle into the clean ring around their parent that sigma /
        // graphology produce.
        Self {
            bound: 5000.0,
            max_iters: 800,
            theta: 0.9,
            convergence_eps: 0.5,
            convergence_streak: 20,

            scaling_ratio: 100.0,
            gravity: 1.0,
            strong_gravity: false,
            slow_down: 5.0,
            lin_log: false,
            outbound_attraction_distribution: false,
            degree_repulsion: true,
            anchor_stiffness: 0.0,
        }
    }
}

/// Single source of truth for the FA2 physics. Runs the iteration loop
/// in place over `pos`, stopping when convergence is reached, the
/// iteration cap is hit, or `should_stop()` returns true. After each
/// completed iteration `on_iter(iter, &pos)` is invoked — the worker
/// uses it to publish a snapshot, bump its iteration counter, and yield
/// the core; the synchronous entry points pass a no-op.
fn run_fa2(
    pos: &mut [Vec2],
    edges: &[(u32, u32)],
    params: &LayoutParams,
    anchors: Option<&[Option<Vec2>]>,
    should_stop: impl Fn() -> bool,
    mut on_iter: impl FnMut(u32, &[Vec2]),
) {
    let n = pos.len();
    if n == 0 {
        return;
    }
    let nf = n as f32;

    // mass = deg + 1 (FA2 convention). Used in repulsion and
    // in outbound-attraction-distribution.
    let mut mass = vec![1.0f32; n];
    if params.degree_repulsion || params.outbound_attraction_distribution {
        for &(a, b) in edges {
            let (a, b) = (a as usize, b as usize);
            if a < n {
                mass[a] += 1.0;
            }
            if b < n && b != a {
                mass[b] += 1.0;
            }
        }
    }
    // Repulsion weights — only mass-weighted when
    // degree_repulsion is on; attraction-distribution can
    // still use mass below independently.
    let rep_mass: Vec<f32> = if params.degree_repulsion {
        mass.clone()
    } else {
        vec![1.0; n]
    };

    let mut disp = vec![Vec2::ZERO; n];
    let mut prev_disp = vec![Vec2::ZERO; n];
    let mut convergence = vec![1.0f32; n];

    let c_rep = params.scaling_ratio;
    let slow_down = params.slow_down.max(0.1);
    // FA2 splits gravity through `coefficient`:
    // `g = gravity / scaling_ratio` then
    // `factor = scaling_ratio * mass * g / dist` — the ratios
    // cancel, leaving an effective `mass * gravity / dist`.
    // We computed it as `scaling_ratio² · mass · gravity / dist`
    // for one painful day; that crushed everything toward the
    // origin and produced visible chaos at small zooms.
    let g_eff = params.gravity;

    // Anchoring is active only when a caller supplied an anchor slice and
    // the spring constant is positive. When inactive, the anchor force is
    // never touched, so the solver is byte-identical to the un-anchored
    // path (and `force_to_convergence`/`force_layout`, which pass `None`).
    let anchored = match anchors {
        Some(a) => params.anchor_stiffness > 0.0 && a.len() == n,
        None => false,
    };

    let mut converged_streak = 0u32;

    for iter in 0..params.max_iters {
        if should_stop() {
            break;
        }

        // Save previous disp; reset current.
        for i in 0..n {
            prev_disp[i] = disp[i];
            disp[i] = Vec2::ZERO;
        }

        // 1) Repulsion via Barnes–Hut. Build the quadtree
        // inline: bounding-box → root cell → insert each
        // point with its mass.
        let tree = {
            let points: &[Vec2] = pos;
            let weights: &[f32] = &rep_mass;
            if points.is_empty() {
                QuadTree { nodes: Vec::new() }
            } else {
                let mut min = points[0];
                let mut max = points[0];
                for &p in points.iter().skip(1) {
                    min.x = min.x.min(p.x);
                    min.y = min.y.min(p.y);
                    max.x = max.x.max(p.x);
                    max.y = max.y.max(p.y);
                }
                let center = (min + max) * 0.5;
                let span = (max.x - min.x).max(max.y - min.y).max(1.0);
                let half = span * 0.5 + 1.0;

                let mut t = QuadTree {
                    nodes: Vec::with_capacity(points.len() * 2),
                };
                t.nodes.push(Quad {
                    center,
                    half,
                    com: Vec2::ZERO,
                    mass: 0.0,
                    children: [NO_CHILD; 4],
                    leaf_point: None,
                });
                for (i, &p) in points.iter().enumerate() {
                    let w = weights.get(i).copied().unwrap_or(1.0);
                    t.insert(0, p, w, 0);
                }
                t
            }
        };
        for i in 0..n {
            // Pass `c_rep * rep_mass[i]` so the tree's
            // internal `force * com_mass` produces
            // `c_rep * mass_i * mass_com / dist²` per
            // component (FA2 linear repulsion).
            let coeff = c_rep * rep_mass[i];
            disp[i] += tree.repulsion_on(pos[i], coeff, params.theta);
        }

        // 2) Gravity (toward origin).
        if params.gravity > 0.0 {
            for i in 0..n {
                let p = pos[i];
                let dist = p.length().max(1e-3);
                let factor = if params.strong_gravity {
                    mass[i] * g_eff
                } else {
                    mass[i] * g_eff / dist
                };
                disp[i] -= p * factor;
            }
        }

        // 3) Attraction along edges.
        for &(a, b) in edges {
            let a = a as usize;
            let b = b as usize;
            if a >= n || b >= n || a == b {
                continue;
            }
            let d = pos[a] - pos[b];
            let attr_factor = if params.lin_log {
                let dist = d.length().max(1e-3);
                let f = -(1.0 + dist).ln() / dist;
                if params.outbound_attraction_distribution {
                    f / mass[a]
                } else {
                    f
                }
            } else {
                // Linear attraction: constant force per edge
                // component (per FA2, "distance is set to 1"
                // — dx += xDist*factor gives a force
                // component proportional to xDist).
                if params.outbound_attraction_distribution {
                    -1.0 / mass[a]
                } else {
                    -1.0
                }
            };
            disp[a] += d * attr_factor;
            disp[b] -= d * attr_factor;
        }

        // 3b) Anchor springs (temporal stability). Each anchored node
        // feels a weak Hooke spring toward the position it held in the
        // previous layout. Nodes with no anchor (e.g. newly added) feel
        // nothing here and settle freely under the other forces. The
        // spring participates in the normal adaptive-speed apply step
        // below like any other accumulated force.
        if anchored {
            let anchors = anchors.expect("anchored implies Some(anchors)");
            let k = params.anchor_stiffness;
            for i in 0..n {
                if let Some(a) = anchors[i] {
                    disp[i] += (a - pos[i]) * k;
                }
            }
        }

        // 4) Apply forces with FA2 adaptive node speed.
        // This is the load-bearing piece for convergence
        // — see module docs.
        let mut global_swinging = 0.0f32;
        let mut total_step = 0.0f32;
        for i in 0..n {
            let d = disp[i];
            let od = prev_disp[i];

            let swing_vec = od - d;
            let swinging = rep_mass[i] * swing_vec.length();
            let trac_vec = od + d;
            let traction = trac_vec.length() * 0.5;

            let denom = 1.0 + swinging.sqrt();
            let nodespeed = convergence[i] * (1.0 + traction).ln() / denom;

            // Update per-node convergence (capped at 1.0).
            let f2 = d.length_sq();
            convergence[i] = ((nodespeed * f2 / denom).sqrt()).min(1.0);

            let step = d * (nodespeed / slow_down);
            let np = pos[i] + step;
            pos[i] = Vec2::new(
                np.x.clamp(-params.bound, params.bound),
                np.y.clamp(-params.bound, params.bound),
            );
            global_swinging += swinging;
            total_step += step.length();
        }

        on_iter(iter + 1, pos);

        // Stop once the system has nearly stopped oscillating.
        // Mean step magnitude is a more useful per-frame
        // metric than swinging for our slow_down range; use
        // both as belts.
        let mean_step = total_step / nf;
        let mean_swing = global_swinging / nf;
        if mean_step < params.convergence_eps && mean_swing < params.convergence_eps {
            converged_streak += 1;
            if converged_streak >= params.convergence_streak {
                break;
            }
        } else {
            converged_streak = 0;
        }
    }
}

/// Run the FA2 layout synchronously until it converges (or hits the
/// iteration cap), with a caller-supplied `should_stop` predicate
/// checked at the top of every iteration. Returns the settled
/// positions. Drives the same [`run_fa2`] physics as [`LayoutWorker`].
pub fn force_to_convergence(
    initial: Vec<Vec2>,
    edges: &[(u32, u32)],
    params: &LayoutParams,
    should_stop: impl Fn() -> bool,
) -> Vec<Vec2> {
    let mut pos = initial;
    run_fa2(&mut pos, edges, params, None, should_stop, |_, _| {});
    pos
}

/// Convenience wrapper: [`force_to_convergence`] with a `should_stop`
/// that never fires.
pub fn force_layout(initial: Vec<Vec2>, edges: &[(u32, u32)], params: &LayoutParams) -> Vec<Vec2> {
    force_to_convergence(initial, edges, params, || false)
}

/// Like [`force_to_convergence`], but with **anchor springs** for
/// temporal layout stability. `anchors[i] == Some(p)` tethers node `i`
/// to position `p` (where it sat in the previous layout) via a weak
/// spring of strength `params.anchor_stiffness`; `anchors[i] == None`
/// (e.g. a newly added node) lets the node settle freely. `anchors`
/// must be the same length as `initial`.
///
/// With `params.anchor_stiffness == 0.0` (or an `anchors` slice of the
/// wrong length) the anchor force is skipped and this is identical to
/// [`force_to_convergence`]. Drives the same [`run_fa2`] physics.
pub fn force_to_convergence_anchored(
    initial: Vec<Vec2>,
    edges: &[(u32, u32)],
    params: &LayoutParams,
    anchors: &[Option<Vec2>],
    should_stop: impl Fn() -> bool,
) -> Vec<Vec2> {
    let mut pos = initial;
    run_fa2(&mut pos, edges, params, Some(anchors), should_stop, |_, _| {});
    pos
}

/// Background-thread driver for the FA2 layout. **Native only** — gated
/// behind `cfg(not(target_arch = "wasm32"))` because it spins up an OS
/// thread; wasm builds use the synchronous [`force_to_convergence`].
#[cfg(not(target_arch = "wasm32"))]
pub use worker::LayoutWorker;

#[cfg(not(target_arch = "wasm32"))]
mod worker {
    use super::{run_fa2, LayoutParams, Vec2};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, RwLock};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    pub struct LayoutWorker {
        positions: Arc<RwLock<Vec<Vec2>>>,
        iter_count: Arc<AtomicU32>,
        done: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl LayoutWorker {
        /// Spawn the background FA2 solver with no anchoring.
        pub fn spawn(initial: Vec<Vec2>, edges: Vec<(u32, u32)>, params: LayoutParams) -> Self {
            Self::spawn_with_optional_anchors(initial, edges, params, None)
        }

        /// Spawn the background FA2 solver with **anchor springs** for
        /// temporal stability (see [`super::force_to_convergence_anchored`]).
        /// `anchors[i] == Some(p)` tethers node `i` to `p`; `None` lets it
        /// settle freely. `anchors` should match `initial` in length and is
        /// honoured only when `params.anchor_stiffness > 0.0`.
        pub fn spawn_anchored(
            initial: Vec<Vec2>,
            edges: Vec<(u32, u32)>,
            params: LayoutParams,
            anchors: Vec<Option<Vec2>>,
        ) -> Self {
            Self::spawn_with_optional_anchors(initial, edges, params, Some(anchors))
        }

        fn spawn_with_optional_anchors(
            initial: Vec<Vec2>,
            edges: Vec<(u32, u32)>,
            params: LayoutParams,
            anchors: Option<Vec<Option<Vec2>>>,
        ) -> Self {
            let positions = Arc::new(RwLock::new(initial.clone()));
            let iter_count = Arc::new(AtomicU32::new(0));
            let done = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));

            let pos_arc = positions.clone();
            let iter_arc = iter_count.clone();
            let done_arc = done.clone();
            let stop_arc = stop.clone();

            let handle = thread::Builder::new()
                .name("graph-layout".into())
                .spawn(move || {
                    // Force-directed layout loop (FA2). Runs to convergence
                    // or until `stop` flips on Drop. Drives the shared
                    // `run_fa2` physics via a `should_stop` closure reading
                    // the atomic stop flag; per-iteration it publishes a
                    // position snapshot, bumps the iteration counter, and
                    // yields the core so we don't peg a CPU while the UI is
                    // busy.
                    let mut pos = initial;
                    run_fa2(
                        &mut pos,
                        &edges,
                        &params,
                        anchors.as_deref(),
                        || stop_arc.load(Ordering::Relaxed),
                        |iter, pos| {
                            if let Ok(mut w) = pos_arc.write() {
                                w.clear();
                                w.extend_from_slice(pos);
                            }
                            iter_arc.store(iter, Ordering::Relaxed);
                            thread::sleep(Duration::from_micros(100));
                        },
                    );

                    done_arc.store(true, Ordering::Relaxed);
                })
                .expect("failed to spawn graph-layout thread");

            Self {
                positions,
                iter_count,
                done,
                stop,
                handle: Some(handle),
            }
        }

        /// Copy the worker's current position snapshot into `out`. Resizes
        /// `out` to match. Cheap — the worker writes the same `Vec` shape
        /// every iteration so the RwLock contention is short.
        pub fn snapshot_into(&self, out: &mut Vec<Vec2>) {
            if let Ok(p) = self.positions.read() {
                out.clear();
                out.extend_from_slice(&p);
            }
        }

        pub fn is_running(&self) -> bool {
            !self.done.load(Ordering::Relaxed)
        }

        pub fn iters_done(&self) -> u32 {
            self.iter_count.load(Ordering::Relaxed)
        }
    }

    impl Drop for LayoutWorker {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }
}

// ── Barnes–Hut quadtree ────────────────────────────────────────────────

const NO_CHILD: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Quad {
    center: Vec2,
    half: f32,
    com: Vec2,
    mass: f32,
    children: [u32; 4],
    /// Position of the single point in this cell, if any. Cleared once
    /// the cell subdivides.
    leaf_point: Option<Vec2>,
}

struct QuadTree {
    nodes: Vec<Quad>,
}

impl QuadTree {
    fn quadrant(center: Vec2, p: Vec2) -> usize {
        match (p.x >= center.x, p.y >= center.y) {
            (false, false) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (true, true) => 3,
        }
    }

    fn insert(&mut self, idx: usize, p: Vec2, weight: f32, depth: u32) {
        if depth > 40 {
            let n = &mut self.nodes[idx];
            let new_mass = n.mass + weight;
            n.com = (n.com * n.mass + p * weight) / new_mass;
            n.mass = new_mass;
            return;
        }

        let (mass, existing_point, center) = {
            let n = &self.nodes[idx];
            (n.mass, n.leaf_point, n.center)
        };

        if mass == 0.0 {
            let n = &mut self.nodes[idx];
            n.com = p;
            n.mass = weight;
            n.leaf_point = Some(p);
            return;
        }

        if let Some(existing) = existing_point {
            // The existing leaf's weight equals the current `mass`
            // (only one point in this cell so far).
            let existing_weight = mass;
            self.nodes[idx].leaf_point = None;
            let q_ex = Self::quadrant(center, existing);
            let cidx = self.ensure_child(idx, q_ex);
            self.insert(cidx, existing, existing_weight, depth + 1);
        }

        {
            let n = &mut self.nodes[idx];
            let new_mass = n.mass + weight;
            n.com = (n.com * n.mass + p * weight) / new_mass;
            n.mass = new_mass;
        }

        let q = Self::quadrant(center, p);
        let cidx = self.ensure_child(idx, q);
        self.insert(cidx, p, weight, depth + 1);
    }

    fn ensure_child(&mut self, parent_idx: usize, q: usize) -> usize {
        if self.nodes[parent_idx].children[q] != NO_CHILD {
            return self.nodes[parent_idx].children[q] as usize;
        }
        let (center, half) = {
            let n = &self.nodes[parent_idx];
            // Child cell center: pick the quadrant offset (NW/NE/SW/SE)
            // and place it half-a-half away from the parent center.
            let h = n.half * 0.5;
            let cc = match q {
                0 => Vec2::new(n.center.x - h, n.center.y - h),
                1 => Vec2::new(n.center.x + h, n.center.y - h),
                2 => Vec2::new(n.center.x - h, n.center.y + h),
                _ => Vec2::new(n.center.x + h, n.center.y + h),
            };
            (cc, h)
        };
        let new_idx = self.nodes.len();
        self.nodes.push(Quad {
            center,
            half,
            com: Vec2::ZERO,
            mass: 0.0,
            children: [NO_CHILD; 4],
            leaf_point: None,
        });
        self.nodes[parent_idx].children[q] = new_idx as u32;
        new_idx
    }

    fn repulsion_on(&self, p: Vec2, k2: f32, theta: f32) -> Vec2 {
        if self.nodes.is_empty() {
            return Vec2::ZERO;
        }
        let mut acc = Vec2::ZERO;
        let theta2 = theta * theta;
        self.repulsion_rec(0, p, k2, theta2, &mut acc);
        acc
    }

    fn repulsion_rec(&self, idx: usize, p: Vec2, k2: f32, theta2: f32, acc: &mut Vec2) {
        let n = &self.nodes[idx];
        if n.mass == 0.0 {
            return;
        }
        let d = p - n.com;
        let dist2 = d.length_sq();
        if n.leaf_point.is_some() {
            // Single body in this cell. Skip when the body is `p` itself
            // (distance ~ 0); otherwise apply pairwise repulsion.
            if dist2 < 1e-6 {
                return;
            }
            let dist = dist2.sqrt();
            let force = k2 / dist;
            *acc += d / dist * force * n.mass;
            return;
        }
        let s = n.half * 2.0;
        if s * s < theta2 * dist2 {
            let dist = dist2.sqrt().max(0.01);
            let force = k2 / dist;
            *acc += d / dist * force * n.mass;
        } else {
            for &c in &n.children {
                if c != NO_CHILD {
                    self.repulsion_rec(c as usize, p, k2, theta2, acc);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A small ring graph: 0-1-2-3-4-5-0.
    fn ring(n: u32) -> Vec<(u32, u32)> {
        (0..n).map(|i| (i, (i + 1) % n)).collect()
    }

    // Deterministic circle seed (no randomness).
    fn circle_seed(n: usize) -> Vec<Vec2> {
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec2::new(a.cos() * 100.0, a.sin() * 100.0)
            })
            .collect()
    }

    #[test]
    fn force_layout_converges_finite() {
        let n = 6;
        let edges = ring(n as u32);
        let params = LayoutParams::default();
        let out = force_layout(circle_seed(n), &edges, &params);
        assert_eq!(out.len(), n);
        assert!(out.iter().all(|v| v.x.is_finite() && v.y.is_finite()));
    }

    #[test]
    fn force_layout_is_deterministic() {
        let n = 6;
        let edges = ring(n as u32);
        let params = LayoutParams::default();
        let a = force_layout(circle_seed(n), &edges, &params);
        let b = force_layout(circle_seed(n), &edges, &params);
        assert_eq!(a, b);
    }

    #[test]
    fn empty_input_terminates() {
        let params = LayoutParams::default();
        let out = force_layout(Vec::new(), &[], &params);
        assert!(out.is_empty());
    }

    #[test]
    fn should_stop_short_circuits() {
        let n = 6;
        let edges = ring(n as u32);
        let params = LayoutParams::default();
        let seed = circle_seed(n);
        // Stopping immediately yields the seed unchanged.
        let out = force_to_convergence(seed.clone(), &edges, &params, || true);
        assert_eq!(out, seed);
    }

    fn mean_displacement(a: &[Vec2], b: &[Vec2]) -> f32 {
        assert_eq!(a.len(), b.len());
        if a.is_empty() {
            return 0.0;
        }
        let total: f32 = a
            .iter()
            .zip(b)
            .map(|(p, q)| (*p - *q).length())
            .sum();
        total / a.len() as f32
    }

    /// `anchor_stiffness = 0` (anchors all `None`) must be byte-identical
    /// to the plain un-anchored solve on the same seed/edges/params.
    #[test]
    fn anchored_with_zero_stiffness_matches_plain() {
        let n = 6;
        let edges = ring(n as u32);
        let params = LayoutParams::default();
        assert_eq!(params.anchor_stiffness, 0.0);

        let plain = force_to_convergence(circle_seed(n), &edges, &params, || false);

        // Even with real anchor positions, stiffness 0 disables the force.
        let anchors: Vec<Option<Vec2>> = circle_seed(n).into_iter().map(Some).collect();
        let anchored =
            force_to_convergence_anchored(circle_seed(n), &edges, &params, &anchors, || false);

        assert_eq!(plain, anchored);
    }

    /// A `Some`-everywhere anchors slice with positive stiffness still
    /// equals the un-anchored solve when stiffness is 0 — and crucially,
    /// passing `None` for every node also matches plain regardless of
    /// stiffness (no anchored node = no anchor force).
    #[test]
    fn all_none_anchors_match_plain_even_with_stiffness() {
        let n = 6;
        let edges = ring(n as u32);
        let params = LayoutParams {
            anchor_stiffness: 0.1,
            ..LayoutParams::default()
        };
        let plain = force_to_convergence(circle_seed(n), &edges, &params, || false);
        let anchors: Vec<Option<Vec2>> = vec![None; n];
        let anchored =
            force_to_convergence_anchored(circle_seed(n), &edges, &params, &anchors, || false);
        assert_eq!(plain, anchored);
    }

    // ── Tangle regression: anchored re-solve of a COMPLEX graph ──────────
    //
    // These mirror the live warm-start path (warm seed + adaptive anchor
    // stiffness scaled by structural change) and assert via the tangle
    // metric that a big re-clustering's anchored re-solve is no more tangled
    // than a fresh solve, while a small change stays coherent.

    use crate::tangle::edge_crossings;

    /// A deterministic clustered graph: `clusters` rings of `per` nodes plus
    /// inter-cluster bridges whose pattern depends on `variant`, so two
    /// variants are substantially different wirings of (almost) the same node
    /// set. `variant != 0` also adds one fresh node (an add/remove dimension).
    fn complex_graph(clusters: usize, per: usize, variant: u32) -> (usize, Vec<(u32, u32)>) {
        let n = clusters * per + if variant == 0 { 0 } else { 1 };
        let mut edges: Vec<(u32, u32)> = Vec::new();
        let peru = per as u32;
        for c in 0..clusters {
            let base = (c * per) as u32;
            for k in 0..peru {
                edges.push((base + k, base + (k + 1) % peru));
            }
            edges.push((base, base + peru / 2));
            edges.push((base + 1, base + peru / 2 + 1));
        }
        for c in 0..clusters {
            let a = (c * per) as u32;
            let next = ((c + 1) % clusters * per) as u32;
            let off = (variant * 3 + c as u32) % peru;
            edges.push((a + off, next + (off + variant) % peru));
            let across = ((c + clusters / 2) % clusters * per) as u32;
            edges.push((a + (variant + 1) % peru, across));
        }
        if variant != 0 {
            let extra = (clusters * per) as u32;
            edges.push((extra, 0));
            edges.push((extra, peru * 3 + 2));
        }
        (n, edges)
    }

    /// Deterministic per-index scatter (matches the graph-view seed family) so
    /// the "fresh" baseline uses the same seed distribution the live fresh
    /// path uses — force layout is seed-sensitive, so the comparison must hold
    /// the seed family fixed to measure tangle rather than basin luck.
    fn scatter_seed(n: usize, box_size: f32) -> Vec<Vec2> {
        (0..n)
            .map(|i| {
                let mut s =
                    0x9E37_79B9_7F4A_7C15u64 ^ (i as u64).wrapping_mul(0x2545_F491_4F6C_DD1D);
                let mut rng = || {
                    s = s
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((s >> 33) as u32) as f32 / (u32::MAX as f32)
                };
                Vec2::new((rng() - 0.5) * box_size, (rng() - 0.5) * box_size)
            })
            .collect()
    }

    /// Adaptive anchor stiffness, mirroring `graph_view::adaptive_anchor_stiffness`:
    /// linear from `baseline` at 0% changed to 0 at ≥50% changed.
    fn adaptive_stiffness(baseline: f32, change_fraction: f32) -> f32 {
        let factor = (1.0 - change_fraction / 0.5).clamp(0.0, 1.0);
        baseline * factor
    }

    /// The regression guard. Build a complex graph G1, solve fresh (P1). Build
    /// a substantially restructured G2 and solve it (a) FRESH and (b) WARM +
    /// ANCHORED with the ADAPTIVE policy (stiffness + warm-seed relax scaled by
    /// change). The adaptive anchored re-solve must be no more tangled than the
    /// fresh solve (within a sane bound) — where the OLD flat-stiffness
    /// behaviour was dramatically worse.
    #[test]
    fn adaptive_anchor_untangles_big_change() {
        let params = LayoutParams::default();
        let (n1, e1) = complex_graph(8, 12, 0);
        let p1 = force_to_convergence(scatter_seed(n1, 400.0), &e1, &params, || false);

        let (n2, e2) = complex_graph(8, 12, 1);
        let retained = n1.min(n2);

        // Fresh baseline from the same seed family.
        let fresh = force_to_convergence(scatter_seed(n2, 400.0), &e2, &params, || false);
        let c_fresh = edge_crossings(&fresh, &e2);

        // change_fraction ~ 0.45 here (big rewrite) → near-zero stiffness +
        // near-full seed relax.
        let change_fraction = 0.45;
        let relax = (change_fraction / 0.5_f32).clamp(0.0, 1.0);
        let stiffness = adaptive_stiffness(0.2, change_fraction);

        // Warm seed: retained nodes blended from P1 toward the fresh-scatter
        // by `relax`; the lone new node spawns near the centroid. Anchors are
        // the true P1 positions for retained nodes (strength is what scales).
        let centroid = {
            let s: Vec2 = p1.iter().fold(Vec2::ZERO, |a, p| a + *p);
            s / p1.len() as f32
        };
        let scatter = scatter_seed(n2, 400.0);
        let mut warm = Vec::with_capacity(n2);
        let mut anchors: Vec<Option<Vec2>> = Vec::with_capacity(n2);
        for i in 0..n2 {
            if i < retained {
                let p = p1[i];
                warm.push(p + (scatter[i] - p) * relax);
                anchors.push(Some(p));
            } else {
                warm.push(centroid);
                anchors.push(None);
            }
        }
        let anchor_params = LayoutParams {
            anchor_stiffness: stiffness,
            ..LayoutParams::default()
        };
        let anchored = force_to_convergence_anchored(warm, &e2, &anchor_params, &anchors, || false);
        let c_anchored = edge_crossings(&anchored, &e2);

        // And the OLD flat-stiffness behaviour for contrast (warm seed pinned
        // to P1, full stiffness) — documents the regression this guards.
        let mut warm_old = Vec::with_capacity(n2);
        for i in 0..n2 {
            warm_old.push(if i < retained { p1[i] } else { centroid });
        }
        let old_params = LayoutParams {
            anchor_stiffness: 0.2,
            ..LayoutParams::default()
        };
        let anchored_old =
            force_to_convergence_anchored(warm_old, &e2, &old_params, &anchors, || false);
        let c_old = edge_crossings(&anchored_old, &e2);

        assert!(
            c_anchored as f32 <= c_fresh as f32 * 1.3,
            "adaptive anchored re-solve too tangled: {c_anchored} crossings vs fresh {c_fresh} \
             (bound {:.0}); old flat behaviour was {c_old}",
            c_fresh as f32 * 1.3
        );
        // Sanity: the old behaviour really is the regression we fixed (else
        // this test isn't testing anything).
        assert!(
            c_old > c_fresh,
            "expected old flat-stiffness behaviour to be more tangled than fresh \
             (old {c_old}, fresh {c_fresh})"
        );
    }

    /// Temporal-stability guarantee: re-solving a grown graph with anchors
    /// keeps the retained nodes near their old positions, where a fresh
    /// solve reshuffles them.
    #[test]
    fn anchored_resolve_preserves_retained_layout() {
        // Base graph: a 6-ring. Solve it to get P1.
        let base_n = 6usize;
        let base_edges = ring(base_n as u32);
        let params = LayoutParams::default();
        let p1 = force_to_convergence(circle_seed(base_n), &base_edges, &params, || false);

        // Grow it: add a 4-node cluster attached to base node 0.
        let new_ids = [6u32, 7, 8, 9];
        let mut edges = base_edges.clone();
        // ring among the new nodes + one bridge into the base graph.
        for w in 0..new_ids.len() {
            edges.push((new_ids[w], new_ids[(w + 1) % new_ids.len()]));
        }
        edges.push((0, new_ids[0]));
        let total_n = base_n + new_ids.len();

        // (a) FRESH solve from a scatter seed (deterministic).
        let scatter: Vec<Vec2> = (0..total_n)
            .map(|i| {
                let a = i as f32 / total_n as f32 * std::f32::consts::TAU;
                // offset the angle so it doesn't accidentally match P1
                Vec2::new((a + 0.7).cos() * 120.0, (a + 0.7).sin() * 120.0)
            })
            .collect();
        let fresh = force_to_convergence(scatter, &edges, &params, || false);

        // (b) WARM + ANCHORED: retained nodes start at (and are anchored
        // to) their P1 positions; new nodes spawn near the centroid.
        let centroid = {
            let s: Vec2 = p1.iter().fold(Vec2::ZERO, |acc, p| acc + *p);
            s / p1.len() as f32
        };
        let mut warm_seed = p1.clone();
        let mut anchors: Vec<Option<Vec2>> = p1.iter().map(|p| Some(*p)).collect();
        for (k, _) in new_ids.iter().enumerate() {
            // small deterministic spread around the centroid
            let off = Vec2::new((k as f32 * 1.3).cos() * 5.0, (k as f32 * 1.3).sin() * 5.0);
            warm_seed.push(centroid + off);
            anchors.push(None);
        }
        let anchor_params = LayoutParams {
            anchor_stiffness: 0.1,
            ..LayoutParams::default()
        };
        let warm =
            force_to_convergence_anchored(warm_seed, &edges, &anchor_params, &anchors, || false);

        // Compare retained nodes (0..base_n) against P1.
        let fresh_drift = mean_displacement(&fresh[..base_n], &p1);
        let warm_drift = mean_displacement(&warm[..base_n], &p1);

        // The anchored re-solve must keep retained nodes substantially
        // closer to their original positions than the fresh solve.
        assert!(
            warm_drift < fresh_drift * 0.5,
            "anchored drift {warm_drift} should be << fresh drift {fresh_drift}"
        );

        // And a new node must actually move toward its neighbours, not be
        // stuck at its spawn point near the centroid.
        let spawn0 = centroid + Vec2::new(0.0_f32.cos() * 5.0, 0.0_f32.sin() * 5.0);
        let settled0 = warm[base_n];
        let moved = (settled0 - spawn0).length();
        assert!(
            moved > 5.0,
            "new node should settle away from its spawn ({moved} world units moved)"
        );
        // It should be finite and within bounds.
        assert!(settled0.x.is_finite() && settled0.y.is_finite());
    }
}
