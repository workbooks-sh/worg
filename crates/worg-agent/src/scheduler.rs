//! Topological wave-planner for task DAGs.
//!
//! Pure data transformation — no concurrency primitives, no Tokio
//! task-spawning here. The Scheduler's role is the *plan*; the
//! integration that *dispatches* a wave via `tokio::spawn` /
//! `JoinSet` lives at the Loop edge.
//!
//! Mirrors Elixir's `WorgAgent.Scheduler` field-for-field at the
//! contract level so a single test corpus can validate both
//! runtimes (Phase 7).
//!
//! ## Example
//!
//! ```
//! use worg_agent::scheduler::{topological_waves, TaskSpec};
//!
//! let tasks: Vec<TaskSpec> = vec![
//!     ("research".into(), vec![]),
//!     ("script".into(), vec!["research".into()]),
//!     ("velocity".into(), vec!["research".into()]),
//!     ("storyboard".into(), vec!["script".into(), "velocity".into()]),
//!     ("shots".into(), vec!["storyboard".into()]),
//! ];
//! let waves = topological_waves(&tasks).unwrap();
//! assert_eq!(waves.len(), 4);
//! assert_eq!(waves[0], vec!["research"]);
//! assert_eq!(waves[1], vec!["script", "velocity"]);
//! ```

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

/// `(task_id, depends_on)`. Order in the input list is preserved
/// inside each emitted wave (deterministic output for reproducibility).
pub type TaskSpec = (String, Vec<String>);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScheduleError {
    /// At least one cycle among the remaining tasks. The returned
    /// vec is the set of task ids that could not be ordered.
    #[error("cycle detected among tasks: {0:?}")]
    Cycle(Vec<String>),
    /// A task lists a `depends_on` that isn't in the input task
    /// list. Silently treating unknown deps as satisfied would mask
    /// real wiring bugs.
    #[error("task {task:?} depends on unknown id {dep:?}")]
    UnknownDep { task: String, dep: String },
}

/// Plan execution waves for a list of `(task_id, depends_on)` pairs.
///
/// Returns `Ok(vec_of_waves)` where each wave is a `Vec<String>` of
/// task ids that can run concurrently. Wave N's tasks all have
/// their dependencies satisfied by waves 1..N-1.
pub fn topological_waves(tasks: &[TaskSpec]) -> Result<Vec<Vec<String>>, ScheduleError> {
    check_unknown_deps(tasks)?;

    let ids: Vec<String> = tasks.iter().map(|(id, _)| id.clone()).collect();
    let deps_map: BTreeMap<String, Vec<String>> = tasks.iter().cloned().collect();
    build_waves(ids, &deps_map)
}

/// Given a task list + a set of already-completed task ids, return
/// the list of ids ready to dispatch NOW: their deps are all in
/// `completed` and they themselves are not. Order matches the input
/// list so a dispatcher can reproduce wave decisions identically.
pub fn next_ready(tasks: &[TaskSpec], completed: &BTreeSet<String>) -> Vec<String> {
    tasks
        .iter()
        .filter_map(|(id, deps)| {
            if completed.contains(id) {
                return None;
            }
            if deps.iter().all(|d| completed.contains(d)) {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect()
}

fn check_unknown_deps(tasks: &[TaskSpec]) -> Result<(), ScheduleError> {
    let known: BTreeSet<&str> = tasks.iter().map(|(id, _)| id.as_str()).collect();
    for (id, deps) in tasks {
        if let Some(bad) = deps.iter().find(|d| !known.contains(d.as_str())) {
            return Err(ScheduleError::UnknownDep {
                task: id.clone(),
                dep: bad.clone(),
            });
        }
    }
    Ok(())
}

fn build_waves(
    mut remaining: Vec<String>,
    deps_map: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<Vec<String>>, ScheduleError> {
    let mut completed: BTreeSet<String> = BTreeSet::new();
    let mut acc: Vec<Vec<String>> = Vec::new();

    loop {
        let (ready, still): (Vec<String>, Vec<String>) =
            remaining.iter().cloned().partition(|id| {
                deps_map
                    .get(id)
                    .map(|deps| deps.iter().all(|d| completed.contains(d)))
                    .unwrap_or(true)
            });

        if ready.is_empty() && still.is_empty() {
            return Ok(acc);
        }
        if ready.is_empty() {
            return Err(ScheduleError::Cycle(still));
        }
        for r in &ready {
            completed.insert(r.clone());
        }
        acc.push(ready);
        remaining = still;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, deps: &[&str]) -> TaskSpec {
        (id.to_string(), deps.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn linear_chain_yields_one_task_per_wave() {
        let tasks = vec![t("a", &[]), t("b", &["a"]), t("c", &["b"])];
        let waves = topological_waves(&tasks).unwrap();
        assert_eq!(waves, vec![vec!["a"], vec!["b"], vec!["c"]]);
    }

    #[test]
    fn parallel_branches_collapse_into_one_wave() {
        let tasks = vec![
            t("research", &[]),
            t("script", &["research"]),
            t("velocity", &["research"]),
            t("storyboard", &["script", "velocity"]),
            t("shots", &["storyboard"]),
        ];
        let waves = topological_waves(&tasks).unwrap();
        assert_eq!(waves.len(), 4);
        assert_eq!(waves[1], vec!["script", "velocity"]);
    }

    #[test]
    fn cycle_is_detected_and_surfaced() {
        let tasks = vec![t("a", &["b"]), t("b", &["a"])];
        match topological_waves(&tasks).unwrap_err() {
            ScheduleError::Cycle(ids) => {
                assert!(ids.contains(&"a".to_string()));
                assert!(ids.contains(&"b".to_string()));
            }
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    #[test]
    fn unknown_dep_is_rejected() {
        let tasks = vec![t("a", &["ghost"])];
        match topological_waves(&tasks).unwrap_err() {
            ScheduleError::UnknownDep { task, dep } => {
                assert_eq!(task, "a");
                assert_eq!(dep, "ghost");
            }
            other => panic!("expected UnknownDep, got {other:?}"),
        }
    }

    #[test]
    fn input_order_is_preserved_inside_each_wave() {
        let tasks = vec![
            t("a", &[]),
            t("c", &[]),
            t("b", &[]), // wave 1 should be a, c, b — input order
            t("d", &["a", "c", "b"]),
        ];
        let waves = topological_waves(&tasks).unwrap();
        assert_eq!(waves[0], vec!["a", "c", "b"]);
        assert_eq!(waves[1], vec!["d"]);
    }

    #[test]
    fn next_ready_skips_completed_and_unmet() {
        let tasks = vec![t("a", &[]), t("b", &["a"]), t("c", &["a"])];
        let mut done = BTreeSet::new();
        assert_eq!(next_ready(&tasks, &done), vec!["a"]);
        done.insert("a".to_string());
        assert_eq!(next_ready(&tasks, &done), vec!["b", "c"]);
        done.insert("b".to_string());
        done.insert("c".to_string());
        assert!(next_ready(&tasks, &done).is_empty());
    }

    #[test]
    fn empty_input_returns_empty_plan() {
        let tasks: Vec<TaskSpec> = vec![];
        assert!(topological_waves(&tasks).unwrap().is_empty());
    }
}
