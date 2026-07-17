//! Graph topology analysis for knowledge-graph void detection.
use std::collections::{HashMap, HashSet};

/// Betti numbers for a graph viewed as a 2-complex with only nodes and edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BettiNumbers {
    /// Number of connected components.
    pub betti_0: usize,
    /// Number of independent cycles.
    pub betti_1: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoidType {
    /// A node is isolated or barely connected (degree 0 or 1).
    MissingContext,
    /// Two nodes are in the same component but not sufficiently connected.
    MissingLink,
    /// Conflicting edge assertions between the same pair of nodes.
    ContradictionGap,
}

/// Candidate structural gap detected in the knowledge graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologicalVoid {
    /// Human-readable explanation of the gap.
    pub description: String,
    /// Items considered nearby the detected gap.
    pub nearby_items: Vec<String>,
    /// Suggested edges that could reduce this gap.
    pub suggested_connections: Vec<(String, String)>,
    /// The kind of detected topological issue.
    pub void_type: VoidType,
}

/// Computes Betti numbers from an adjacency list.
///
/// The graph is treated as undirected for topological counts.
pub fn compute_betti_numbers(adjacency: &HashMap<String, Vec<String>>) -> BettiNumbers {
    let mut nodes: HashSet<String> = HashSet::new();
    for (node, neighbors) in adjacency {
        nodes.insert(node.clone());
        for neighbor in neighbors {
            nodes.insert(neighbor.clone());
        }
    }

    let nodes: Vec<String> = nodes.into_iter().collect();
    if nodes.is_empty() {
        return BettiNumbers {
            betti_0: 0,
            betti_1: 0,
        };
    }

    let mut index: HashMap<String, usize> = HashMap::new();
    for (idx, node) in nodes.iter().enumerate() {
        index.insert(node.clone(), idx);
    }

    let mut union_find = UnionFind::new(nodes.len());
    let mut undirected_edges: HashSet<(String, String)> = HashSet::new();

    for (node, neighbors) in adjacency {
        for neighbor in neighbors {
            if node == neighbor {
                continue;
            }
            let ordered = canonical_pair(node, neighbor);
            undirected_edges.insert(ordered);

            if let (Some(left), Some(right)) = (index.get(node), index.get(neighbor)) {
                union_find.union(*left, *right);
            }
        }
    }

    let mut roots: HashSet<usize> = HashSet::new();
    for idx in 0..nodes.len() {
        roots.insert(union_find.find(idx));
    }

    let nodes_count = nodes.len();
    let components = roots.len();
    let edge_count = undirected_edges.len();
    let raw_betti_1 = edge_count as isize - nodes_count as isize + components as isize;

    BettiNumbers {
        betti_0: components,
        betti_1: if raw_betti_1 > 0 {
            raw_betti_1 as usize
        } else {
            0
        },
    }
}

/// Finds likely voids in a graph described by a simple edge list.
///
/// `edges` are interpreted as unordered pairs.
pub fn find_voids(edges: &[(String, String)]) -> Vec<TopologicalVoid> {
    if edges.is_empty() {
        return Vec::new();
    }

    let mut adjacency: HashMap<String, HashSet<String>> = HashMap::new();
    let mut pair_counts: HashMap<(String, String), usize> = HashMap::new();

    for (left, right) in edges {
        adjacency.entry(left.clone()).or_default();
        adjacency.entry(right.clone()).or_default();

        if left != right {
            adjacency
                .entry(left.clone())
                .or_default()
                .insert(right.clone());
            adjacency
                .entry(right.clone())
                .or_default()
                .insert(left.clone());
        }

        let ordered = canonical_pair(left, right);
        let count = pair_counts.entry(ordered).or_insert(0);
        *count += 1;
    }

    let mut nodes: Vec<String> = adjacency.keys().cloned().collect();
    if nodes.is_empty() {
        return Vec::new();
    }
    nodes.sort_unstable();

    let mut index: HashMap<String, usize> = HashMap::new();
    for (idx, node) in nodes.iter().enumerate() {
        index.insert(node.clone(), idx);
    }
    let mut union_find = UnionFind::new(nodes.len());
    let mut linked_pairs: HashSet<(String, String)> = HashSet::new();

    for (node, neighbors) in &adjacency {
        for neighbor in neighbors {
            if node == neighbor {
                continue;
            }
            let ordered = canonical_pair(node, neighbor);
            if linked_pairs.insert(ordered) {
                if let (Some(left), Some(right)) = (index.get(node), index.get(neighbor)) {
                    union_find.union(*left, *right);
                }
            }
        }
    }

    let mut roots: HashMap<String, usize> = HashMap::new();
    for node in &nodes {
        if let Some(idx) = index.get(node) {
            roots.insert(node.clone(), union_find.find(*idx));
        }
    }

    let mut voids: Vec<TopologicalVoid> = Vec::new();

    for (pair, count) in pair_counts {
        if count > 1 {
            let (left, right) = pair;
            voids.push(TopologicalVoid {
                description: format!(
                    "Multiple edge assertions exist between '{left}' and '{right}', suggesting contradictory edge semantics."
                ),
                nearby_items: vec![left.clone(), right.clone()],
                suggested_connections: vec![(left.clone(), right.clone())],
                void_type: VoidType::ContradictionGap,
            });
        }
    }

    for node in &nodes {
        let degree = adjacency.get(node).map_or(0, HashSet::len);
        if degree <= 1 {
            let suggested_connections = if degree == 0 {
                if let Some(other) = nodes.iter().find(|candidate| *candidate != node) {
                    vec![(node.clone(), other.clone())]
                } else {
                    Vec::new()
                }
            } else if let Some(neighbor) = adjacency
                .get(node)
                .and_then(|neighbors| neighbors.iter().next())
            {
                vec![(node.clone(), neighbor.clone())]
            } else {
                Vec::new()
            };

            voids.push(TopologicalVoid {
                description: format!("Node '{node}' is weakly connected with degree {degree}, suggesting a context gap."),
                nearby_items: vec![node.clone()],
                suggested_connections,
                void_type: VoidType::MissingContext,
            });
        }
    }

    let mut i = 0;
    while i < nodes.len() {
        let mut j = i + 1;
        while j < nodes.len() {
            let left = nodes[i].clone();
            let right = nodes[j].clone();
            let same_component = roots.get(&left) == roots.get(&right);
            let direct = adjacency
                .get(&left)
                .is_some_and(|neighbors| neighbors.contains(&right));
            let two_hop = has_two_hop(&left, &right, &adjacency);
            if same_component && !direct && !two_hop {
                voids.push(TopologicalVoid {
                    description: format!(
                        "Nodes '{left}' and '{right}' are in the same connected component without direct or 2-hop connectivity."
                    ),
                    nearby_items: vec![left.clone(), right.clone()],
                    suggested_connections: vec![(left, right)],
                    void_type: VoidType::MissingLink,
                });
            }
            j += 1;
        }
        i += 1;
    }

    voids
}

/// Builds a short human-readable summary of detected voids.
pub fn gap_report(voids: &[TopologicalVoid]) -> String {
    let mut missing_link = 0usize;
    let mut missing_context = 0usize;
    let mut contradiction_gap = 0usize;
    for void in voids {
        match void.void_type {
            VoidType::MissingLink => missing_link += 1,
            VoidType::MissingContext => missing_context += 1,
            VoidType::ContradictionGap => contradiction_gap += 1,
        }
    }

    let mut lines = vec![
        format!("Topological void report: {} void(s)", voids.len()),
        format!("MissingLink: {missing_link}"),
        format!("MissingContext: {missing_context}"),
        format!("ContradictionGap: {contradiction_gap}"),
        "Details:".to_string(),
    ];
    for void in voids {
        lines.push(format!("- {:?}: {}", void.void_type, void.description));
    }

    lines.join("\n")
}

fn canonical_pair(left: &str, right: &str) -> (String, String) {
    if left <= right {
        (left.to_string(), right.to_string())
    } else {
        (right.to_string(), left.to_string())
    }
}

fn has_two_hop(left: &str, right: &str, adjacency: &HashMap<String, HashSet<String>>) -> bool {
    let first = match adjacency.get(left) {
        Some(items) => items,
        None => return false,
    };
    let second = match adjacency.get(right) {
        Some(items) => items,
        None => return false,
    };
    for hop in first {
        if second.contains(hop) {
            return true;
        }
    }
    false
}

#[derive(Debug, Default)]
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(size: usize) -> Self {
        let parent = (0..size).collect();
        let rank = vec![0; size];
        UnionFind { parent, rank }
    }

    fn find(&mut self, idx: usize) -> usize {
        if self.parent[idx] == idx {
            return idx;
        }
        let root = self.find(self.parent[idx]);
        self.parent[idx] = root;
        root
    }

    fn union(&mut self, a: usize, b: usize) -> bool {
        let mut root_a = self.find(a);
        let mut root_b = self.find(b);
        if root_a == root_b {
            return false;
        }

        if self.rank[root_a] < self.rank[root_b] {
            std::mem::swap(&mut root_a, &mut root_b);
        }
        self.parent[root_b] = root_a;
        if self.rank[root_a] == self.rank[root_b] {
            self.rank[root_a] = self.rank[root_a].saturating_add(1);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn compute_betti_numbers_triangle() {
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        adjacency.insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);
        adjacency.insert("b".to_string(), vec!["a".to_string(), "c".to_string()]);
        adjacency.insert("c".to_string(), vec!["a".to_string(), "b".to_string()]);

        let result = compute_betti_numbers(&adjacency);
        assert_eq!(
            result,
            BettiNumbers {
                betti_0: 1,
                betti_1: 1,
            }
        );
    }

    #[test]
    fn compute_betti_numbers_two_disconnected_edges() {
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        adjacency.insert("a".to_string(), vec!["b".to_string()]);
        adjacency.insert("b".to_string(), vec!["a".to_string()]);
        adjacency.insert("c".to_string(), vec!["d".to_string()]);
        adjacency.insert("d".to_string(), vec!["c".to_string()]);

        let result = compute_betti_numbers(&adjacency);
        assert_eq!(
            result,
            BettiNumbers {
                betti_0: 2,
                betti_1: 0,
            }
        );
    }

    #[test]
    fn find_voids_detects_missing_context_for_isolated_node() {
        let voids = find_voids(&[("isolated".to_string(), "isolated".to_string())]);
        assert!(voids
            .iter()
            .any(|item| matches!(item.void_type, VoidType::MissingContext)));
    }

    #[test]
    fn find_voids_detects_missing_link_between_distant_nodes() {
        let edges = vec![
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "c".to_string()),
            ("c".to_string(), "d".to_string()),
            ("d".to_string(), "e".to_string()),
        ];
        let voids = find_voids(&edges);
        assert!(voids.iter().any(|item| {
            matches!(item.void_type, VoidType::MissingLink)
                && item.nearby_items.contains(&"a".to_string())
                && item.nearby_items.contains(&"e".to_string())
        }));
    }

    #[test]
    fn find_voids_empty_graph_is_empty() {
        let voids = find_voids(&[]);
        assert!(voids.is_empty());
    }

    #[test]
    fn gap_report_is_readable() {
        let voids = vec![TopologicalVoid {
            description: "example".to_string(),
            nearby_items: vec!["a".to_string(), "b".to_string()],
            suggested_connections: vec![("a".to_string(), "b".to_string())],
            void_type: VoidType::MissingLink,
        }];
        let report = gap_report(&voids);
        assert!(report.contains("Topological void report"));
        assert!(report.contains("MissingLink"));
    }
}
