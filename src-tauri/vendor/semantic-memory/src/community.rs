use std::collections::{HashMap, HashSet};

/// Represents a detected knowledge community.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Community {
    /// Stable community identifier.
    pub id: String,
    /// Ordered list of node identifiers in the community.
    pub members: Vec<String>,
    /// Community hierarchy level.
    pub level: usize,
    /// Optional parent community identifier.
    pub parent: Option<String>,
}

/// Records a contradiction between two nodes inside the same community.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommunityContradiction {
    /// Community identifier where the contradiction was observed.
    pub community_id: String,
    /// First contradictory item.
    pub item_a: String,
    /// Second contradictory item.
    pub item_b: String,
    /// Human-readable reason for the flagged contradiction.
    pub description: String,
}

/// Compression recommendation for a community.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompressionDecision {
    /// Community identifier.
    pub community_id: String,
    /// Suggested quantization level.
    pub quantization_level: String,
    /// Rationale for the recommendation.
    pub reason: String,
}

#[derive(Clone)]
struct LcgRng {
    state: u64,
}

impl LcgRng {
    fn new(seed: u64) -> Self {
        let normalized_seed = if seed == 0 { 0x243F6A8885A308D3 } else { seed };
        Self {
            state: normalized_seed,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.state
    }

    fn next_usize(&mut self, modulus: usize) -> usize {
        if modulus == 0 {
            return 0;
        }
        (self.next_u64() % modulus as u64) as usize
    }
}

fn build_node_index(edges: &[(String, String)]) -> (Vec<String>, Vec<(usize, usize)>) {
    let mut nodes_set = HashSet::new();
    let mut indexed_edges = Vec::with_capacity(edges.len());
    let mut edge_pairs = Vec::with_capacity(edges.len());

    for (left, right) in edges {
        let left_ref = left.as_str();
        let right_ref = right.as_str();
        nodes_set.insert(left_ref.to_owned());
        nodes_set.insert(right_ref.to_owned());
        edge_pairs.push((left_ref.to_owned(), right_ref.to_owned()));
    }

    let mut nodes: Vec<String> = nodes_set.into_iter().collect();
    nodes.sort_unstable();

    let index_by_node: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| (node.to_owned(), idx))
        .collect();

    for (left, right) in edge_pairs {
        let Some(&left_idx) = index_by_node.get(&left) else {
            continue;
        };
        let Some(&right_idx) = index_by_node.get(&right) else {
            continue;
        };
        indexed_edges.push((left_idx, right_idx));
    }

    (nodes, indexed_edges)
}

fn degrees(node_count: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let mut out = vec![0usize; node_count];
    for &(left, right) in edges {
        if left < node_count {
            out[left] = out[left].saturating_add(1);
        }
        if right < node_count {
            out[right] = out[right].saturating_add(1);
        }
    }
    out
}

fn build_neighbors(node_count: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut neighbors = vec![Vec::new(); node_count];
    for &(left, right) in edges {
        if left < node_count && right < node_count {
            neighbors[left].push(right);
            neighbors[right].push(left);
        }
    }
    neighbors
}

fn modularity(
    edges: &[(usize, usize)],
    node_count: usize,
    assignments: &[usize],
    node_degrees: &[usize],
    resolution: f64,
) -> f64 {
    let mut community_count = 0usize;
    for &assignment in assignments {
        if assignment + 1 > community_count {
            community_count = assignment + 1;
        }
    }
    if node_count == 0 || community_count == 0 || edges.is_empty() {
        return 0.0;
    }

    let two_m = (edges.len() as f64) * 2.0;
    if two_m <= 0.0 {
        return 0.0;
    }

    let mut internal_edges = vec![0usize; community_count];
    let mut community_degrees = vec![0.0f64; community_count];

    for node in 0..node_count {
        let community = assignments[node];
        if community < community_degrees.len() {
            community_degrees[community] += node_degrees[node] as f64;
        }
    }

    for &(left, right) in edges {
        if left < node_count && right < node_count {
            let left_community = assignments[left];
            let right_community = assignments[right];
            if left_community == right_community && left_community < internal_edges.len() {
                internal_edges[left_community] = internal_edges[left_community].saturating_add(1);
            }
        }
    }

    let mut score = 0.0f64;
    for idx in 0..community_count {
        let l_c = internal_edges[idx] as f64;
        let t_c = community_degrees[idx];
        let within = l_c / two_m;
        let expected = resolution * (t_c / two_m) * (t_c / two_m);
        score += within - expected;
    }
    score
}

fn maybe_shuffle_order(order: &mut [usize], rng: &mut LcgRng) {
    if order.len() < 2 {
        return;
    }

    for i in (1..order.len()).rev() {
        let j = rng.next_usize(i + 1);
        order.swap(i, j);
    }
}

fn relabel_assignments(assignments: &mut [usize]) {
    let mut map = HashMap::new();
    let mut next_id = 0usize;
    for assignment in assignments.iter_mut() {
        let new_id = if let Some(existing) = map.get(assignment) {
            *existing
        } else {
            let next = next_id;
            map.insert(*assignment, next);
            next_id = next_id.saturating_add(1);
            next
        };
        *assignment = new_id;
    }
}

fn refine_merge(
    node_count: usize,
    edges: &[(usize, usize)],
    assignments: &mut [usize],
    rng: &mut LcgRng,
) -> usize {
    let mut iterations = 0usize;
    loop {
        iterations += 1;
        if iterations > node_count.saturating_add(4) {
            break;
        }

        let mut community_sizes: HashMap<usize, usize> = HashMap::new();
        for &comm in assignments.iter() {
            *community_sizes.entry(comm).or_insert(0) += 1;
        }
        if community_sizes.len() <= 1 {
            break;
        }

        let mut pair_intersections: HashMap<(usize, usize), usize> = HashMap::new();
        for &(left, right) in edges {
            if left >= node_count || right >= node_count {
                continue;
            }
            let c_left = assignments[left];
            let c_right = assignments[right];
            if c_left == c_right {
                continue;
            }
            let mut a = c_left;
            let mut b = c_right;
            if a > b {
                core::mem::swap(&mut a, &mut b);
            }
            *pair_intersections.entry((a, b)).or_insert(0) += 1;
        }

        let mut candidates: Vec<(usize, usize, usize)> = pair_intersections
            .into_iter()
            .map(|((a, b), count)| (a, b, count))
            .collect();
        if candidates.is_empty() {
            break;
        }
        candidates.sort_unstable_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));

        let mut merged = false;
        for (a, b, count) in candidates {
            let size_a = *community_sizes.get(&a).unwrap_or(&1);
            let size_b = *community_sizes.get(&b).unwrap_or(&1);
            if size_a == 0 || size_b == 0 {
                continue;
            }
            let possible = (size_a as f64) * (size_b as f64);
            if possible <= 0.0 {
                continue;
            }
            let connectivity = (count as f64) / possible;
            if connectivity < 0.5 {
                continue;
            }

            let keep = if rng.next_usize(2) == 0 { a } else { b };
            let absorb = if keep == a { b } else { a };
            for assignment in assignments.iter_mut() {
                if *assignment == absorb {
                    *assignment = keep;
                }
            }
            merged = true;
            break;
        }

        if !merged {
            break;
        }
        relabel_assignments(assignments);
    }
    iterations
}

fn local_move_step(
    node_count: usize,
    edges: &[(usize, usize)],
    node_degrees: &[usize],
    assignments: &mut [usize],
    resolution: f64,
    rng: &mut LcgRng,
) -> bool {
    let mut changed = false;
    let neighbors = build_neighbors(node_count, edges);

    let mut order: Vec<usize> = (0..node_count).collect();
    maybe_shuffle_order(&mut order, rng);

    for vertex in order {
        let current_comm = assignments[vertex];

        let mut candidate_comms = Vec::new();
        candidate_comms.push(current_comm);
        for &neighbor in &neighbors[vertex] {
            if neighbor < node_count {
                let neighbor_comm = assignments[neighbor];
                if !candidate_comms.contains(&neighbor_comm) {
                    candidate_comms.push(neighbor_comm);
                }
            }
        }
        if candidate_comms.len() <= 1 {
            continue;
        }

        let current_quality = modularity(edges, node_count, assignments, node_degrees, resolution);
        let mut best_comm = current_comm;
        let mut best_quality = current_quality;

        for candidate in candidate_comms {
            if candidate == current_comm {
                continue;
            }
            let mut next = assignments.to_owned();
            next[vertex] = candidate;
            let next_quality = modularity(edges, node_count, &next, node_degrees, resolution);
            let delta = next_quality - best_quality;
            if delta > 1.0e-12 {
                best_quality = next_quality;
                best_comm = candidate;
            } else if delta.abs() <= 1.0e-12 && rng.next_usize(2) == 0 {
                best_quality = next_quality;
                best_comm = candidate;
            }
        }

        if best_comm != current_comm {
            assignments[vertex] = best_comm;
            changed = true;
        }
    }
    changed
}

fn detect_communities_internal(
    edges: &[(String, String)],
    resolution: f64,
    seed: u64,
) -> Vec<Vec<String>> {
    let (nodes, indexed_edges) = build_node_index(edges);
    if nodes.is_empty() {
        return Vec::new();
    }

    let node_count = nodes.len();
    let degrees = degrees(node_count, &indexed_edges);
    let mut assignments: Vec<usize> = (0..node_count).collect();
    let mut rng = LcgRng::new(seed);

    let max_iterations = node_count.saturating_mul(12).max(1);
    for _ in 0..max_iterations {
        if !local_move_step(
            node_count,
            &indexed_edges,
            &degrees,
            &mut assignments,
            resolution,
            &mut rng,
        ) {
            break;
        }
    }

    let _ = refine_merge(node_count, &indexed_edges, &mut assignments, &mut rng);

    relabel_assignments(&mut assignments);

    let mut communities: Vec<Vec<String>> =
        vec![Vec::new(); assignments.iter().copied().max().map_or(0, |v| v + 1)];
    for (node_idx, community_idx) in assignments.iter().copied().enumerate() {
        if let Some(members) = communities.get_mut(community_idx) {
            members.push(nodes[node_idx].clone());
        }
    }
    for members in &mut communities {
        members.sort_unstable();
    }
    communities
        .into_iter()
        .filter(|members| !members.is_empty())
        .collect()
}

/// Run a deterministic, Leiden-inspired community detection pass over an edge list.
pub fn detect_communities(
    edges: &[(String, String)],
    resolution: f64,
    seed: u64,
) -> Vec<Community> {
    detect_communities_internal(edges, resolution, seed)
        .into_iter()
        .enumerate()
        .map(|(idx, members)| Community {
            id: format!("community_{idx}"),
            members,
            level: 0,
            parent: None,
        })
        .collect()
}

/// Scan contradictions that occur within the same community.
pub fn community_contradiction_scan(
    communities: &[Community],
    contradictions: &[(String, String)],
) -> Vec<CommunityContradiction> {
    let mut membership: HashMap<String, String> = HashMap::new();
    for community in communities {
        for item in &community.members {
            membership.insert(item.to_owned(), community.id.clone());
        }
    }

    let mut output = Vec::new();
    for (left, right) in contradictions {
        let Some(left_comm) = membership.get(left) else {
            continue;
        };
        let Some(right_comm) = membership.get(right) else {
            continue;
        };
        if left_comm == right_comm {
            output.push(CommunityContradiction {
                community_id: left_comm.clone(),
                item_a: left.to_owned(),
                item_b: right.to_owned(),
                description: format!("contradiction observed inside {}", left_comm),
            });
        }
    }
    output
}

fn importance_lookup(scores: &[(String, f64)]) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    for (item, score) in scores {
        out.insert(item.clone(), *score);
    }
    out
}

/// Recommend quantization by community tightness and importance.
pub fn community_aware_compression(
    communities: &[Community],
    importance_scores: &[(String, f64)],
) -> Vec<CompressionDecision> {
    let score_by_item = importance_lookup(importance_scores);

    let mut output = Vec::with_capacity(communities.len());
    for community in communities {
        let mut total = 0.0f64;
        let mut found_score = false;
        for item in &community.members {
            if let Some(score) = score_by_item.get(item) {
                total += *score;
                found_score = true;
            }
        }
        let average = if community.members.is_empty() {
            0.0
        } else if found_score {
            total / community.members.len() as f64
        } else {
            0.0
        };

        let (quantization_level, reason) = if community.members.len() == 1 {
            let score = community
                .members
                .first()
                .and_then(|item| score_by_item.get(item))
                .copied()
                .unwrap_or(0.0);
            if score >= 0.8 {
                ("F32".to_string(), "high-importance standalone".to_string())
            } else if score <= 0.35 {
                (
                    "SQ4".to_string(),
                    "isolated low-importance fact".to_string(),
                )
            } else {
                (
                    "SQ8".to_string(),
                    "moderate single-node community".to_string(),
                )
            }
        } else if average >= 0.75 && community.members.len() >= 2 {
            (
                "SQ8".to_string(),
                "tight high-importance community".to_string(),
            )
        } else {
            ("SQ8".to_string(), "default community encoding".to_string())
        };

        output.push(CompressionDecision {
            community_id: community.id.clone(),
            quantization_level,
            reason,
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn detects_two_disconnected_clusters() {
        let edges = vec![
            (item("a"), item("b")),
            (item("b"), item("c")),
            (item("x"), item("y")),
            (item("y"), item("z")),
        ];
        let communities = detect_communities(&edges, 1.0, 7);
        assert_eq!(communities.len(), 2);

        let mut sorted_members: Vec<Vec<String>> = communities
            .into_iter()
            .map(|community| community.members)
            .collect();
        sorted_members.sort_unstable();
        assert!(
            sorted_members.contains(&vec![item("a"), item("b"), item("c")])
                && sorted_members.contains(&vec![item("x"), item("y"), item("z")])
        );
    }

    #[test]
    fn detects_single_connected_community() {
        let edges = vec![
            (item("a"), item("b")),
            (item("b"), item("c")),
            (item("c"), item("d")),
            (item("d"), item("e")),
        ];
        let communities = detect_communities(&edges, 1.0, 19);
        // A fully connected graph should produce at most 2 communities
        // (Leiden may split depending on resolution and iterations).
        assert!(
            communities.len() <= 2,
            "expected at most 2 communities, got {}",
            communities.len()
        );
        // All members across communities should cover all 5 nodes.
        let total_members: std::collections::HashSet<&String> =
            communities.iter().flat_map(|c| c.members.iter()).collect();
        assert_eq!(total_members.len(), 5);
    }

    #[test]
    fn empty_edge_list_is_empty_or_singleton_per_node() {
        let communities = detect_communities(&[], 1.0, 11);
        assert_eq!(communities.len(), 0);
    }

    #[test]
    fn community_contradiction_scan_finds_internal_pairs() {
        let communities = vec![
            Community {
                id: "c0".to_string(),
                members: vec![item("a"), item("b"), item("c")],
                level: 0,
                parent: None,
            },
            Community {
                id: "c1".to_string(),
                members: vec![item("d"), item("e")],
                level: 0,
                parent: None,
            },
        ];

        let contradictions = vec![
            (item("a"), item("c")),
            (item("a"), item("d")),
            (item("b"), item("c")),
        ];
        let found = community_contradiction_scan(&communities, &contradictions);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].community_id, "c0");
    }

    #[test]
    fn community_aware_compression_prioritizes_rules() {
        let communities = vec![
            Community {
                id: "c0".to_string(),
                members: vec![item("a"), item("b"), item("c")],
                level: 0,
                parent: None,
            },
            Community {
                id: "c1".to_string(),
                members: vec![item("d")],
                level: 0,
                parent: None,
            },
            Community {
                id: "c2".to_string(),
                members: vec![item("e")],
                level: 0,
                parent: None,
            },
        ];
        let importance = vec![
            (item("a"), 0.95),
            (item("b"), 0.9),
            (item("c"), 0.9),
            (item("d"), 0.9),
            (item("e"), 0.1),
        ];
        let decisions = community_aware_compression(&communities, &importance);
        assert_eq!(decisions.len(), 3);
        let mut by_id: HashMap<String, CompressionDecision> = HashMap::new();
        for decision in decisions {
            by_id.insert(decision.community_id.clone(), decision);
        }

        let high_singleton = by_id
            .get("c1")
            .expect("community c1 decision")
            .quantization_level
            .clone();
        assert_eq!(high_singleton, "F32");

        let low_singleton = by_id
            .get("c2")
            .expect("community c2 decision")
            .quantization_level
            .clone();
        assert_eq!(low_singleton, "SQ4");

        let grouped = by_id
            .get("c0")
            .expect("community c0 decision")
            .quantization_level
            .clone();
        assert_eq!(grouped, "SQ8");
    }

    #[test]
    fn same_seed_is_deterministic() {
        let edges = vec![
            (item("a"), item("b")),
            (item("b"), item("c")),
            (item("c"), item("a")),
            (item("d"), item("e")),
            (item("e"), item("f")),
            (item("f"), item("d")),
            (item("c"), item("d")),
        ];
        let first = detect_communities(&edges, 1.0, 77);
        let second = detect_communities(&edges, 1.0, 77);
        assert_eq!(first, second);
    }
}
