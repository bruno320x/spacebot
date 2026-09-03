//! Autonomy modes — the dial on Spacebot's autonomy engine.
//!
//! Every unit of work (a worker, a branch, a goal loop) runs in exactly one
//! mode. The mode answers three questions before anything executes:
//!
//! - How much may the agent act *before* human review? (Plan = none)
//! - How strictly must completion be verified? (Goal = tests/evidence gate)
//! - How fast may the loop run? (Vibe = flow, Yolo = guardrails off)
//!
//! The decision is deterministic and pure: given a description of the task
//! (novelty, risk, trust, explicit request), `decide_mode` picks the mode.
//! Cortex and the spawn paths call this when a new task arrives; humans can
//! always override with an explicit mode.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The autonomy dial for a unit of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskMode {
    /// Research only. No files touched, no commands run, no side effects.
    /// Used for novel or high-risk tasks until a human approves a plan.
    Plan,
    /// Goal + loop: plan, execute via worker, verify (tests/evidence),
    /// iterate on failure, then report. The default for real work.
    Goal,
    /// Fast, low-friction execution for stable, low-risk, trusted tasks.
    /// Minimal approval prompts; verification still applies.
    Vibe,
    /// Guardrails off. Never chosen implicitly — only when the user
    /// explicitly requests it in a trusted context.
    Yolo,
}

impl TaskMode {
    /// All modes, in ascending autonomy order. Mirrors the `ALL` pattern
    /// used elsewhere in the codebase (e.g. `TaskWorktreeMode::ALL`).
    pub const ALL: [TaskMode; 4] = [
        TaskMode::Plan,
        TaskMode::Goal,
        TaskMode::Vibe,
        TaskMode::Yolo,
    ];

    /// One-line description of what this mode permits, for prompts and
    /// status blocks. Kept short so it reads well when injected.
    pub fn description(self) -> &'static str {
        match self {
            TaskMode::Plan => "research only; propose a plan, execute nothing",
            TaskMode::Goal => "plan then execute; completion requires passing verification",
            TaskMode::Vibe => "fast flow; minimal approvals for trusted low-risk work",
            TaskMode::Yolo => "guardrails off; act without approval (user-requested)",
        }
    }

    /// Whether the mode allows execution at all (Plan does not).
    pub fn permits_execution(self) -> bool {
        !matches!(self, TaskMode::Plan)
    }

    /// Whether the mode requires verification evidence before reporting
    /// completion (Plan has nothing to verify; Yolo skips the gate).
    pub fn requires_verification(self) -> bool {
        matches!(self, TaskMode::Goal | TaskMode::Vibe)
    }

    /// Autonomy rank: Plan < Goal < Vibe < Yolo. Used to clamp a task's mode
    /// against the autonomy ceiling of the agent that owns it.
    pub fn rank(self) -> u8 {
        match self {
            TaskMode::Plan => 0,
            TaskMode::Goal => 1,
            TaskMode::Vibe => 2,
            TaskMode::Yolo => 3,
        }
    }

    /// Clamp a mode to an autonomy ceiling: a task may never run *above* the
    /// ceiling its agent allows. Equal ranks pass through unchanged.
    pub fn clamp_to(self, ceiling: TaskMode) -> TaskMode {
        if self.rank() <= ceiling.rank() {
            self
        } else {
            ceiling
        }
    }
}

/// The most autonomous `TaskMode` an agent at this `AutonomyLevel` may run.
///
/// Off and Observe permit no execution (Plan = research only); Suggest allows
/// the full goal loop (approval still happens on the board, not by skipping
/// it); Act is the full dial. A task's mode is clamped to this ceiling at
/// creation time, and the default for a task with no explicit mode is
/// `TaskMode::Goal` (never Yolo — that is chosen explicitly or not at all).
pub fn autonomy_ceiling_mode(level: crate::config::AutonomyLevel) -> TaskMode {
    match level {
        crate::config::AutonomyLevel::Off => TaskMode::Plan,
        crate::config::AutonomyLevel::Observe => TaskMode::Plan,
        crate::config::AutonomyLevel::Suggest => TaskMode::Goal,
        crate::config::AutonomyLevel::Act => TaskMode::Yolo,
    }
}

/// Signals the decision function uses to pick a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskSignals {
    /// True when the task is novel (never done before, no recipe, no memory).
    /// Novel tasks default to Plan/Goal; repeated tasks may run faster.
    pub is_novel: bool,
    /// Blast radius of a wrong execution.
    pub risk: RiskLevel,
    /// True when the context is trusted (human present, sandboxed, or the
    /// user has granted autonomous permission for this workspace).
    pub trusted: bool,
    /// True when the user explicitly asked for guardrails-off mode.
    pub explicit_yolo: bool,
}

impl TaskSignals {
    pub fn new(is_novel: bool, risk: RiskLevel, trusted: bool) -> Self {
        Self {
            is_novel,
            risk,
            trusted,
            explicit_yolo: false,
        }
    }

    pub fn with_explicit_yolo(mut self, explicit: bool) -> Self {
        self.explicit_yolo = explicit;
        self
    }
}

/// Estimated blast radius of a wrong execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Decide the mode for a task.
///
/// Rules (documented so behavior is predictable):
/// 1. Explicit Yolo is honored only in a trusted context; elsewhere it
///    falls back to Goal — guardrails never drop silently.
/// 2. Novel + high risk → Plan: research first, execute nothing.
/// 3. Novel (but not high risk) → Goal: plan then execute with gates.
/// 4. Repeated but risky → Goal: familiarity does not drop the gate.
/// 5. Repeated + low risk + trusted → Vibe: fast flow.
/// 6. Everything else → Goal: the safe default for real work.
pub fn decide_mode(signals: TaskSignals) -> TaskMode {
    if signals.explicit_yolo {
        return if signals.trusted {
            TaskMode::Yolo
        } else {
            TaskMode::Goal
        };
    }
    match (signals.is_novel, signals.risk, signals.trusted) {
        (true, RiskLevel::High, _) => TaskMode::Plan,
        (true, _, _) => TaskMode::Goal,
        (false, RiskLevel::High, _) => TaskMode::Goal,
        (false, RiskLevel::Low, true) => TaskMode::Vibe,
        (false, RiskLevel::Low, false) => TaskMode::Goal,
        (false, RiskLevel::Medium, _) => TaskMode::Goal,
    }
}

impl fmt::Display for TaskMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TaskMode::Plan => "plan",
            TaskMode::Goal => "goal",
            TaskMode::Vibe => "vibe",
            TaskMode::Yolo => "yolo",
        };
        f.write_str(s)
    }
}

impl FromStr for TaskMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "plan" => Ok(TaskMode::Plan),
            "goal" => Ok(TaskMode::Goal),
            "vibe" => Ok(TaskMode::Vibe),
            "yolo" => Ok(TaskMode::Yolo),
            other => Err(format!(
                "unknown task mode '{other}' (expected one of: {})",
                TaskMode::ALL
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_signals() -> TaskSignals {
        TaskSignals::new(true, RiskLevel::High, true)
    }

    #[test]
    fn novel_high_risk_plans() {
        assert_eq!(decide_mode(plan_signals()), TaskMode::Plan);
    }

    #[test]
    fn novel_low_risk_goals() {
        let signals = TaskSignals::new(true, RiskLevel::Low, true);
        assert_eq!(decide_mode(signals), TaskMode::Goal);
    }

    #[test]
    fn repeated_high_risk_still_goals() {
        let signals = TaskSignals::new(false, RiskLevel::High, true);
        assert_eq!(decide_mode(signals), TaskMode::Goal);
    }

    #[test]
    fn stable_low_risk_trusted_vibes() {
        let signals = TaskSignals::new(false, RiskLevel::Low, true);
        assert_eq!(decide_mode(signals), TaskMode::Vibe);
    }

    #[test]
    fn stable_low_risk_untrusted_goals() {
        let signals = TaskSignals::new(false, RiskLevel::Low, false);
        assert_eq!(decide_mode(signals), TaskMode::Goal);
    }

    #[test]
    fn explicit_yolo_requires_trust() {
        let trusted = TaskSignals::new(true, RiskLevel::High, true).with_explicit_yolo(true);
        assert_eq!(decide_mode(trusted), TaskMode::Yolo);

        // Untrusted context: explicit yolo falls back to Goal, never silent
        // guardrail drops.
        let untrusted = TaskSignals::new(true, RiskLevel::High, false).with_explicit_yolo(true);
        assert_eq!(decide_mode(untrusted), TaskMode::Goal);
    }

    #[test]
    fn ranks_follow_ascending_autonomy() {
        assert!(TaskMode::Plan.rank() < TaskMode::Goal.rank());
        assert!(TaskMode::Goal.rank() < TaskMode::Vibe.rank());
        assert!(TaskMode::Vibe.rank() < TaskMode::Yolo.rank());
    }

    #[test]
    fn clamp_never_raises_a_mode_above_its_ceiling() {
        assert_eq!(TaskMode::Yolo.clamp_to(TaskMode::Vibe), TaskMode::Vibe);
        assert_eq!(TaskMode::Vibe.clamp_to(TaskMode::Vibe), TaskMode::Vibe);
        assert_eq!(TaskMode::Vibe.clamp_to(TaskMode::Yolo), TaskMode::Vibe);
        assert_eq!(TaskMode::Plan.clamp_to(TaskMode::Goal), TaskMode::Plan);
        assert_eq!(TaskMode::Goal.clamp_to(TaskMode::Plan), TaskMode::Plan);
        // A plan ceiling only ever admits Plan.
        assert_eq!(TaskMode::Yolo.clamp_to(TaskMode::Plan), TaskMode::Plan);
    }

    #[test]
    fn plan_never_executes() {
        assert!(!TaskMode::Plan.permits_execution());
        assert!(TaskMode::Goal.permits_execution());
        assert!(TaskMode::Vibe.permits_execution());
        assert!(TaskMode::Yolo.permits_execution());
    }

    #[test]
    fn verification_required_except_plan_and_yolo() {
        assert!(!TaskMode::Plan.requires_verification());
        assert!(TaskMode::Goal.requires_verification());
        assert!(TaskMode::Vibe.requires_verification());
        assert!(!TaskMode::Yolo.requires_verification());
    }

    #[test]
    fn display_and_parse_round_trip() {
        for mode in TaskMode::ALL {
            let text = mode.to_string();
            assert_eq!(text.parse::<TaskMode>().unwrap(), mode);
        }
    }

    #[test]
    fn parse_rejects_unknown_with_helpful_error() {
        let err = "banana".parse::<TaskMode>().unwrap_err();
        assert!(err.contains("banana"));
        assert!(err.contains("plan"));
        assert!(err.contains("yolo"));
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!("PLAN".parse::<TaskMode>().unwrap(), TaskMode::Plan);
        assert_eq!("  Goal  ".parse::<TaskMode>().unwrap(), TaskMode::Goal);
    }

    #[test]
    fn serde_round_trip_persists_modes() {
        for mode in TaskMode::ALL {
            let json = serde_json::to_string(&mode).unwrap();
            let back: TaskMode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn serialized_form_matches_display() {
        // Persistence writes the display form ("plan"/"goal"/...) so data on
        // disk reads the same way config and prompts see it.
        assert_eq!(serde_json::to_string(&TaskMode::Plan).unwrap(), "\"plan\"");
        assert_eq!(serde_json::to_string(&TaskMode::Yolo).unwrap(), "\"yolo\"");
    }

    #[test]
    fn descriptions_are_short_and_distinct() {
        let mut seen = std::collections::HashSet::new();
        for mode in TaskMode::ALL {
            let description = mode.description();
            assert!(!description.is_empty());
            assert!(description.len() < 80);
            assert!(seen.insert(description), "duplicate description");
        }
    }
}
