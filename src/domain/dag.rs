use std::collections::{BTreeMap, BTreeSet};

use super::task::{TaskContract, TaskStatus};

#[derive(Clone, Copy, Debug)]
pub struct TaskNode<'a> {
    pub contract: &'a TaskContract,
    pub status: TaskStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DagError {
    DuplicateTask(String),
    MissingDependency { task: String, dependency: String },
    SelfDependency(String),
    Cycle,
}

impl std::fmt::Display for DagError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateTask(id) => write!(formatter, "duplicate task: {id}"),
            Self::MissingDependency { task, dependency } => {
                write!(
                    formatter,
                    "task {task} has missing dependency: {dependency}"
                )
            }
            Self::SelfDependency(id) => write!(formatter, "task {id} depends on itself"),
            Self::Cycle => write!(formatter, "task dependency cycle"),
        }
    }
}

impl std::error::Error for DagError {}

pub struct Dag<'a> {
    nodes: BTreeMap<&'a str, (BTreeSet<&'a str>, TaskStatus)>,
    order: Vec<&'a str>,
}

impl<'a> Dag<'a> {
    pub fn new(tasks: &[TaskNode<'a>]) -> Result<Self, DagError> {
        let mut nodes = BTreeMap::new();
        for task in tasks {
            let id = task.contract.id.as_str();
            if nodes
                .insert(
                    id,
                    (
                        task.contract
                            .depends_on
                            .iter()
                            .map(|dependency| dependency.as_str())
                            .collect::<BTreeSet<_>>(),
                        task.status,
                    ),
                )
                .is_some()
            {
                return Err(DagError::DuplicateTask(id.to_owned()));
            }
        }

        let mut dependents: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        let mut indegree = BTreeMap::new();
        for (&id, (dependencies, _)) in &nodes {
            indegree.insert(id, dependencies.len());
            for &dependency in dependencies {
                if dependency == id {
                    return Err(DagError::SelfDependency(id.to_owned()));
                }
                if !nodes.contains_key(dependency) {
                    return Err(DagError::MissingDependency {
                        task: id.to_owned(),
                        dependency: dependency.to_owned(),
                    });
                }
                dependents.entry(dependency).or_default().insert(id);
            }
        }

        // ponytail: ID leksikografis memecah seri; prioritas menjadi urusan scheduler.
        let mut available: BTreeSet<&str> = indegree
            .iter()
            .filter_map(|(&id, &degree)| (degree == 0).then_some(id))
            .collect();
        let mut order = Vec::with_capacity(nodes.len());
        while let Some(id) = available.pop_first() {
            order.push(id);
            if let Some(children) = dependents.get(id) {
                for &child in children {
                    let degree = indegree.get_mut(child).expect("validated task");
                    *degree -= 1;
                    if *degree == 0 {
                        available.insert(child);
                    }
                }
            }
        }
        if order.len() != nodes.len() {
            return Err(DagError::Cycle);
        }
        Ok(Self { nodes, order })
    }

    pub fn topological_order(&self) -> &[&'a str] {
        &self.order
    }

    pub fn ready(&self) -> Vec<&'a str> {
        self.nodes
            .iter()
            .filter_map(|(&id, (dependencies, status))| {
                (*status == TaskStatus::Ready
                    && dependencies.iter().all(|dependency| {
                        self.nodes
                            .get(dependency)
                            .is_some_and(|(_, status)| *status == TaskStatus::Done)
                    }))
                .then_some(id)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::task::{AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskLimits};
    use super::*;

    fn text(value: &str) -> NonEmptyString {
        NonEmptyString::parse("id", value).unwrap()
    }

    fn contract(id: &str, dependencies: &[&str]) -> TaskContract {
        TaskContract {
            id: text(id),
            project_id: text("project"),
            project_run_id: text("run"),
            title: text("title"),
            role: text("worker"),
            objective: text("objective"),
            depends_on: dependencies.iter().map(|id| text(id)).collect(),
            allowed_paths: vec![AllowedPath::parse("src/**").unwrap()],
            context_refs: vec![],
            acceptance_criteria: vec![text("check")],
            verification_commands: vec![text("cargo test")],
            limits: TaskLimits {
                max_input_tokens: PositiveLimit::new("input", 1).unwrap(),
                max_output_tokens: PositiveLimit::new("output", 1).unwrap(),
                max_tool_calls: PositiveLimit::new("calls", 1).unwrap(),
                max_attempts: MaxAttempts::new(1).unwrap(),
                timeout_seconds: PositiveLimit::new("timeout", 1).unwrap(),
            },
        }
    }

    #[test]
    fn rejects_invalid_edges_and_duplicate_ids() {
        for (tasks, expected) in [
            (
                vec![contract("a", &["missing"])],
                DagError::MissingDependency {
                    task: "a".into(),
                    dependency: "missing".into(),
                },
            ),
            (
                vec![contract("a", &["a"])],
                DagError::SelfDependency("a".into()),
            ),
            (
                vec![contract("a", &["b"]), contract("b", &["a"])],
                DagError::Cycle,
            ),
            (
                vec![
                    contract("a", &["b"]),
                    contract("b", &["c"]),
                    contract("c", &["a"]),
                ],
                DagError::Cycle,
            ),
            (
                vec![contract("a", &[]), contract("a", &[])],
                DagError::DuplicateTask("a".into()),
            ),
        ] {
            let nodes: Vec<_> = tasks
                .iter()
                .map(|contract| TaskNode {
                    contract,
                    status: TaskStatus::Ready,
                })
                .collect();
            assert!(matches!(Dag::new(&nodes), Err(error) if error == expected));
        }
    }

    #[test]
    fn ready_requires_done_dependencies_and_ready_status() {
        let tasks = [
            contract("root", &[]),
            contract("child", &["root"]),
            contract("other", &[]),
        ];
        for (root, child, other, expected) in [
            (
                TaskStatus::Planned,
                TaskStatus::Ready,
                TaskStatus::Ready,
                vec!["other"],
            ),
            (
                TaskStatus::Done,
                TaskStatus::Ready,
                TaskStatus::Ready,
                vec!["child", "other"],
            ),
            (
                TaskStatus::Cancelled,
                TaskStatus::Ready,
                TaskStatus::Ready,
                vec!["other"],
            ),
            (
                TaskStatus::Done,
                TaskStatus::NeedsHuman,
                TaskStatus::Ready,
                vec!["other"],
            ),
            (
                TaskStatus::Done,
                TaskStatus::Ready,
                TaskStatus::Running,
                vec!["child"],
            ),
        ] {
            let nodes: Vec<_> = tasks
                .iter()
                .zip([root, child, other])
                .map(|(contract, status)| TaskNode { contract, status })
                .collect();
            assert_eq!(Dag::new(&nodes).unwrap().ready(), expected);
        }
    }

    #[test]
    fn ordering_is_deterministic_and_respects_every_edge() {
        let tasks = [
            contract("d", &["b", "c"]),
            contract("c", &["a"]),
            contract("b", &["a", "a"]),
            contract("a", &[]),
            contract("e", &[]),
        ];
        let nodes: Vec<_> = tasks
            .iter()
            .map(|contract| TaskNode {
                contract,
                status: TaskStatus::Ready,
            })
            .collect();
        let expected = ["a", "b", "c", "d", "e"];
        for shift in 0..nodes.len() {
            let mut shuffled = nodes.clone();
            shuffled.rotate_left(shift);
            let dag = Dag::new(&shuffled).unwrap();
            assert_eq!(dag.topological_order(), expected);
            assert_eq!(dag.ready(), ["a", "e"]);
            for task in &tasks {
                let current = dag
                    .topological_order()
                    .iter()
                    .position(|&id| id == task.id.as_str())
                    .unwrap();
                for dependency in &task.depends_on {
                    assert!(
                        dag.topological_order()
                            .iter()
                            .position(|&id| id == dependency.as_str())
                            .unwrap()
                            < current
                    );
                }
            }
        }
    }
}
