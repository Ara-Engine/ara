use super::*;

fn declaration(
    index: usize,
    columns: Vec<(u32, bool)>,
    resources: Vec<(u32, bool)>,
    structural: bool,
) -> SystemDecl {
    SystemDecl {
        id: (index * 17 + 3) as u32,
        name: format!("System{index}"),
        order: index as i32,
        columns,
        resources,
        with: Vec::new(),
        without: Vec::new(),
        structural,
    }
}

/// The original pairwise conflict definition, independent of hazard history.
fn legacy_conflict(earlier: &SystemDecl, system: &SystemDecl) -> bool {
    let clash = |a: &[(u32, bool)], b: &[(u32, bool)]| {
        a.iter().any(|(id, write)| {
            b.iter()
                .any(|(other, writable)| id == other && (*write || *writable))
        })
    };
    clash(&earlier.columns, &system.columns)
        || clash(&earlier.resources, &system.resources)
        || (earlier.structural && system.structural)
}

fn legacy(systems: &[SystemDecl]) -> Vec<Vec<u32>> {
    systems
        .iter()
        .enumerate()
        .map(|(index, system)| {
            systems[..index]
                .iter()
                .filter(|earlier| legacy_conflict(earlier, system))
                .map(|earlier| earlier.id)
                .collect()
        })
        .collect()
}

fn planned(systems: &[SystemDecl]) -> (Vec<Vec<u32>>, bool) {
    let mut plan = Dependencies::default();
    let edges = systems.iter().map(|system| plan.push(system)).collect();
    (edges, plan.is_chain())
}

fn reachability(systems: &[SystemDecl], dependencies: &[Vec<u32>]) -> Vec<Vec<bool>> {
    let indexes = systems
        .iter()
        .enumerate()
        .map(|(index, system)| (system.id, index))
        .collect::<BTreeMap<_, _>>();
    let mut successors = vec![Vec::new(); systems.len()];
    for (index, predecessors) in dependencies.iter().enumerate() {
        for id in predecessors {
            let predecessor = indexes[id];
            assert!(predecessor < index, "all dependencies are earlier nodes");
            successors[predecessor].push(index);
        }
    }
    (0..systems.len())
        .map(|start| {
            let mut seen = vec![false; systems.len()];
            let mut queue = vec![start];
            while let Some(node) = queue.pop() {
                for &next in &successors[node] {
                    if !seen[next] {
                        seen[next] = true;
                        queue.push(next);
                    }
                }
            }
            seen
        })
        .collect()
}

#[test]
fn mixed_hazard_graphs_preserve_every_original_ordering_relation() {
    let mut seed = 0x3b19_762du32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for count in (0..32).cycle().take(1024) {
        let systems = (0..count)
            .map(|index| {
                let mut accesses = || {
                    (0..next() % 5)
                        .map(|_| (next() % 8, next() & 1 != 0))
                        .collect::<Vec<_>>()
                };
                let columns = accesses();
                let resources = accesses();
                declaration(index, columns, resources, next().is_multiple_of(7))
            })
            .collect::<Vec<_>>();
        let expected = reachability(&systems, &legacy(&systems));
        let (edges, chain) = planned(&systems);
        assert_eq!(reachability(&systems, &edges), expected, "{systems:?}");
        assert_eq!(chain, (1..count).all(|index| expected[index - 1][index]));
    }
}

#[test]
fn chains_need_not_form_a_clique_and_forks_keep_outer_parallelism() {
    let mut systems = vec![
        declaration(0, vec![(0, true)], vec![], false),
        declaration(1, vec![(0, false), (1, true)], vec![], false),
        declaration(2, vec![(1, false), (2, true)], vec![], false),
    ];
    let (edges, chain) = planned(&systems);
    assert!(chain);
    assert_eq!(
        edges,
        vec![vec![], vec![systems[0].id], vec![systems[1].id]]
    );

    systems[2].columns = vec![(0, false), (2, true)];
    systems.push(declaration(3, vec![(1, false), (2, false)], vec![], false));
    let (edges, chain) = planned(&systems);
    assert!(!chain);
    assert_eq!(edges[3], vec![systems[1].id, systems[2].id]);
    assert_eq!(
        reachability(&systems, &edges),
        reachability(&systems, &legacy(&systems))
    );
}

#[test]
fn namespaces_duplicates_and_readers_before_a_writer_are_distinct() {
    let systems = vec![
        declaration(0, vec![(0, false), (0, false)], vec![], false),
        declaration(1, vec![(0, false)], vec![(0, true)], false),
        declaration(2, vec![(0, false), (0, true)], vec![], false),
        declaration(3, vec![], vec![(0, false)], false),
    ];
    let (edges, chain) = planned(&systems);
    assert!(!chain);
    assert!(edges[1].is_empty(), "resource zero is not component zero");
    assert_eq!(edges[2], vec![systems[0].id, systems[1].id]);
    assert_eq!(edges[3], vec![systems[1].id]);
    assert_eq!(
        reachability(&systems, &edges),
        reachability(&systems, &legacy(&systems))
    );
}

#[test]
fn ten_thousand_writers_or_structural_systems_need_linear_edges() {
    for structural in [false, true] {
        let systems = (0..10_000)
            .map(|index| {
                declaration(
                    index,
                    if structural { vec![] } else { vec![(0, true)] },
                    vec![],
                    structural,
                )
            })
            .collect::<Vec<_>>();
        let (edges, chain) = planned(&systems);
        assert!(chain);
        assert_eq!(edges.iter().map(Vec::len).sum::<usize>(), systems.len() - 1);
        for (index, edge) in edges.iter().enumerate().skip(1) {
            assert_eq!(edge, &[systems[index - 1].id]);
        }
    }
}

#[path = "dependencies_bench.rs"]
mod bench;
