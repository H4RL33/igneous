//! A Barnes–Hut quadtree for many-body forces.
//!
//! The tree is rebuilt every tick. Points are kept in one index array, sorted
//! so that each cell covers a contiguous run of it; this keeps the tree in two
//! flat vectors that are reused between ticks.

use crate::rng::Rng;
use crate::vec2::Vec2;

const NONE: u32 = u32::MAX;
/// Cells this deep are leaves however many points they hold. Only points that
/// (nearly) coincide get here.
const MAX_DEPTH: u32 = 24;

#[derive(Debug, Clone, Copy)]
struct Cell {
    /// The mean position of the points inside.
    centre: Vec2,
    count: u32,
    /// The squared side length of the square cell, divided by θ²: the cell
    /// is far enough away to approximate once the squared distance to its
    /// centre exceeds this.
    reach_squared: f32,
    children: [u32; 4],
    /// The cell's points are `order[start..end]`.
    start: u32,
    end: u32,
}

impl Cell {
    fn is_leaf(&self) -> bool {
        self.children == [NONE; 4]
    }
}

#[derive(Debug, Default)]
pub(crate) struct QuadTree {
    cells: Vec<Cell>,
    order: Vec<u32>,
    scratch: Vec<u32>,
    stack: Vec<u32>,
    /// The Barnes–Hut accuracy threshold θ², from the last build.
    theta_squared: f32,
}

impl QuadTree {
    pub(crate) fn build(&mut self, positions: &[Vec2], theta_squared: f32) {
        self.theta_squared = theta_squared;
        self.cells.clear();
        self.order.clear();
        self.order.extend(0..positions.len() as u32);
        if positions.is_empty() {
            return;
        }
        let (mut min, mut max) = (positions[0], positions[0]);
        for p in positions {
            min = Vec2::new(min.x.min(p.x), min.y.min(p.y));
            max = Vec2::new(max.x.max(p.x), max.y.max(p.y));
        }
        let size = (max.x - min.x).max(max.y - min.y).max(1.0);
        self.subdivide(positions, 0, positions.len(), min, size, 0);
    }

    fn subdivide(
        &mut self,
        positions: &[Vec2],
        start: usize,
        end: usize,
        origin: Vec2,
        size: f32,
        depth: u32,
    ) -> u32 {
        let index = self.cells.len() as u32;
        let mut sum = Vec2::ZERO;
        for &i in &self.order[start..end] {
            sum += positions[i as usize];
        }
        self.cells.push(Cell {
            centre: sum * (1.0 / (end - start) as f32),
            count: (end - start) as u32,
            reach_squared: size * size / self.theta_squared,
            children: [NONE; 4],
            start: start as u32,
            end: end as u32,
        });
        if end - start == 1 || depth >= MAX_DEPTH {
            return index;
        }

        // Sort this cell's points by quadrant (a counting sort through
        // `scratch`), then recurse into each non-empty quadrant.
        let half = size / 2.0;
        let mid = origin + Vec2::new(half, half);
        let quadrant = |p: Vec2| (p.x >= mid.x) as usize | ((p.y >= mid.y) as usize) << 1;
        let mut counts = [0usize; 4];
        for &i in &self.order[start..end] {
            counts[quadrant(positions[i as usize])] += 1;
        }
        let mut offsets = [start; 4];
        for q in 1..4 {
            offsets[q] = offsets[q - 1] + counts[q - 1];
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.order[start..end]);
        let mut next = offsets;
        for &i in &self.scratch {
            let q = quadrant(positions[i as usize]);
            self.order[next[q]] = i;
            next[q] += 1;
        }

        let mut children = [NONE; 4];
        for q in 0..4 {
            if counts[q] == 0 {
                continue;
            }
            let child_origin = Vec2::new(
                if q & 1 == 1 { mid.x } else { origin.x },
                if q & 2 == 2 { mid.y } else { origin.y },
            );
            children[q] = self.subdivide(
                positions,
                offsets[q],
                offsets[q] + counts[q],
                child_origin,
                half,
                depth + 1,
            );
        }
        self.cells[index as usize].children = children;
        index
    }

    /// Adds d3's many-body force to every node's velocity, where each node has
    /// `strength` (negative repels). Distances below 1 are clamped, as in d3.
    ///
    /// Nodes are visited in tree order, so consecutive nodes walk mostly the
    /// same cells and those stay in the cache.
    pub(crate) fn apply_many_body(
        &mut self,
        positions: &[Vec2],
        velocities: &mut [Vec2],
        strength: f32,
        rng: &mut Rng,
    ) {
        const DISTANCE_MIN_SQUARED: f32 = 1.0;
        if self.cells.is_empty() || strength == 0.0 {
            return;
        }
        let mut stack = std::mem::take(&mut self.stack);
        for &i in &self.order {
            let i = i as usize;
            let p = positions[i];
            let mut v = Vec2::ZERO;
            stack.clear();
            stack.push(0);
            while let Some(c) = stack.pop() {
                let cell = &self.cells[c as usize];
                let mut d = cell.centre - p;
                let mut l = d.length_squared();
                if cell.reach_squared < l {
                    // Far enough away to treat the whole cell as one body.
                    if l < DISTANCE_MIN_SQUARED {
                        l = (DISTANCE_MIN_SQUARED * l).sqrt();
                    }
                    v += d * (cell.count as f32 * strength / l);
                    continue;
                }
                if !cell.is_leaf() {
                    for &child in &cell.children {
                        if child != NONE {
                            stack.push(child);
                        }
                    }
                    continue;
                }
                for &j in &self.order[cell.start as usize..cell.end as usize] {
                    if j as usize == i {
                        continue;
                    }
                    d = positions[j as usize] - p;
                    if d.x == 0.0 {
                        d.x = rng.jiggle();
                    }
                    if d.y == 0.0 {
                        d.y = rng.jiggle();
                    }
                    l = d.length_squared();
                    if l < DISTANCE_MIN_SQUARED {
                        l = (DISTANCE_MIN_SQUARED * l).sqrt();
                    }
                    v += d * (strength / l);
                }
            }
            velocities[i] += v;
        }
        self.stack = stack;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_points(n: usize, seed: u64, spread: f32) -> Vec<Vec2> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| Vec2::new(rng.next_f32() * spread, rng.next_f32() * spread))
            .collect()
    }

    fn brute_force(positions: &[Vec2], strength: f32) -> Vec<Vec2> {
        positions
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let mut v = Vec2::ZERO;
                for (j, &q) in positions.iter().enumerate() {
                    if i != j {
                        let d = q - p;
                        let mut l = d.length_squared();
                        if l < 1.0 {
                            l = l.sqrt();
                        }
                        v += d * (strength / l);
                    }
                }
                v
            })
            .collect()
    }

    fn barnes_hut(positions: &[Vec2], strength: f32, theta: f32) -> Vec<Vec2> {
        let mut tree = QuadTree::default();
        tree.build(positions, theta * theta);
        let mut velocities = vec![Vec2::ZERO; positions.len()];
        tree.apply_many_body(positions, &mut velocities, strength, &mut Rng::new(0));
        velocities
    }

    #[test]
    fn every_point_is_in_exactly_one_leaf() {
        let positions = random_points(1000, 1, 500.0);
        let mut tree = QuadTree::default();
        tree.build(&positions, 0.81);
        let mut seen = vec![0; positions.len()];
        for cell in tree.cells.iter().filter(|c| c.is_leaf()) {
            for &i in &tree.order[cell.start as usize..cell.end as usize] {
                seen[i as usize] += 1;
            }
        }
        assert!(seen.iter().all(|&n| n == 1));
        assert_eq!(tree.cells[0].count, 1000);
        for cell in &tree.cells {
            let children: u32 = cell
                .children
                .iter()
                .filter(|&&c| c != NONE)
                .map(|&c| tree.cells[c as usize].count)
                .sum();
            assert!(cell.is_leaf() || children == cell.count);
        }
    }

    #[test]
    fn tiny_theta_is_exact() {
        let positions = random_points(300, 2, 400.0);
        let exact = brute_force(&positions, -30.0);
        let approx = barnes_hut(&positions, -30.0, 1e-3);
        for (a, b) in exact.iter().zip(&approx) {
            assert!(
                (*a - *b).length() <= 1e-3 * a.length().max(1e-3),
                "{a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn approximation_is_close_to_brute_force() {
        let positions = random_points(2000, 3, 2000.0);
        let exact = brute_force(&positions, -300.0);
        let approx = barnes_hut(&positions, -300.0, 0.9);
        let error: f32 = exact
            .iter()
            .zip(&approx)
            .map(|(a, b)| (*a - *b).length())
            .sum();
        let total: f32 = exact.iter().map(|a| a.length()).sum();
        assert!(error / total < 0.05, "relative error {}", error / total);
    }

    #[test]
    fn coincident_points_are_pushed_apart() {
        let positions = vec![Vec2::new(5.0, 5.0); 3];
        let forces = barnes_hut(&positions, -30.0, 0.9);
        assert!(forces.iter().all(|f| f.length() > 0.0 && f.x.is_finite()));
    }
}
