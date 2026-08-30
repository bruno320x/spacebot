//! Task creation tool for branch processes.

use crate::notifications::{NewNotification, NotificationKind, NotificationSeverity};
use crate::tasks::{CreateTaskInput, TaskPriority, TaskStatus, TaskStore, TaskSubtask};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct TaskCreateTool {
    task_store: Arc<TaskStore>,
    agent_id: String,
    created_by: String,
    working_memory: Option<Arc<crate::memory::WorkingMemoryStore>>,
    api_state: Option<Arc<crate::api::ApiState>>,
    project_store: Option<Arc<crate::projects::ProjectStore>>,
    runtime_config: Option<Arc<crate::config::RuntimeConfig>>,
}

impl std::fmt::Debug for TaskCreateTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskCreateTool")
            .field("agent_id", &self.agent_id)
            .field("created_by", &self.created_by)
            .finish()
    }
}

impl TaskCreateTool {
    pub fn new(
        task_store: Arc<TaskStore>,
        agent_id: impl Into<String>,
        created_by: impl Into<String>,
    ) -> Self {
        Self {
            task_store,
            agent_id: agent_id.into(),
            created_by: created_by.into(),
            working_memory: None,
            api_state: None,
            project_store: None,
            runtime_config: None,
        }
    }

    pub fn with_working_memory(mut self, store: Arc<crate::memory::WorkingMemoryStore>) -> Self {
        self.working_memory = Some(store);
        self
    }

    pub fn with_api_state(mut self, state: Arc<crate::api::ApiState>) -> Self {
        self.api_state = Some(state);
        self
    }

    /// Enables execution-plan validation: project name/ID resolution and
    /// required-skill existence checks.
    pub fn with_execution_context(
        mut self,
        project_store: Arc<crate::projects::ProjectStore>,
        runtime_config: Arc<crate::config::RuntimeConfig>,
    ) -> Self {
        self.project_store = Some(project_store);
        self.runtime_config = Some(runtime_config);
        self
    }
}

/// Resolve a project reference (ID or name, case-insensitive) to its ID.
pub(crate) async fn resolve_project_reference(
    store: &crate::projects::ProjectStore,
    reference: &str,
) -> Result<String, String> {
    if let Ok(Some(project)) = store.get_project(reference).await {
        return Ok(project.id);
    }
    let projects = store
        .list_projects(Some(crate::projects::ProjectStatus::Active))
        .await
        .map_err(|error| format!("failed to list projects: {error}"))?;
    let matched: Vec<_> = projects
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(reference))
        .collect();
    match matched.as_slice() {
        [project] => Ok(project.id.clone()),
        [] => Err(format!(
            "no project matches '{reference}'. Known projects: {}",
            projects
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Err(format!(
            "multiple projects named '{reference}' — use the project ID"
        )),
    }
}

/// Parse tool-level dependency args into store edges.
pub(crate) fn parse_dependency_args(
    args: &[TaskDependencyArg],
) -> Result<Vec<(i64, crate::tasks::TaskDependencyKind)>, String> {
    args.iter()
        .map(|arg| {
            let kind = match arg.kind.as_deref() {
                None => crate::tasks::TaskDependencyKind::Gate,
                Some(value) => crate::tasks::TaskDependencyKind::parse(value)
                    .ok_or_else(|| format!("invalid dependency kind: {value}"))?,
            };
            Ok((arg.task, kind))
        })
        .collect()
}

/// Verify every named skill exists in the registry.
pub(crate) fn validate_required_skills(
    runtime_config: &crate::config::RuntimeConfig,
    skills: &[String],
) -> Result<(), String> {
    let registry = runtime_config.skills.load();
    let missing: Vec<&str> = skills
        .iter()
        .filter(|name| registry.get(name).is_none())
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "unknown skill(s): {}. Skill names must match the skills index.",
            missing.join(", ")
        ))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("task_create failed: {0}")]
pub struct TaskCreateError(String);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskCreateArgs {
    pub title: String,
    pub description: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default)]
    pub subtasks: Vec<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Execution plan: "builtin", "opencode", or "acp".
    #[serde(default)]
    pub worker_type: Option<String>,
    /// Execution plan: project name or ID the work belongs to.
    #[serde(default)]
    pub project: Option<String>,
    /// Execution plan: repo ID within the project (multi-repo projects with
    /// worktree_mode "create").
    #[serde(default)]
    pub repo_id: Option<String>,
    /// Execution plan: "root", "existing", or "create".
    #[serde(default)]
    pub worktree_mode: Option<String>,
    /// Execution plan: existing worktree ID (worktree_mode "existing").
    #[serde(default)]
    pub worktree_id: Option<String>,
    /// Skills the executing worker must receive; validated against the
    /// skills index.
    #[serde(default)]
    pub required_skills: Vec<String>,
    /// Tasks this one depends on. `kind` "gate" (default) blocks until the
    /// dependency is done; "stack" blocks only until its branch exists.
    #[serde(default)]
    pub depends_on: Vec<TaskDependencyArg>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskDependencyArg {
    pub task: i64,
    #[serde(default)]
    pub kind: Option<String>,
}

fn default_priority() -> String {
    "medium".to_string()
}

#[derive(Debug, Serialize)]
pub struct TaskCreateOutput {
    pub success: bool,
    pub task_number: i64,
    pub status: String,
    pub message: String,
}

impl Tool for TaskCreateTool {
    const NAME: &'static str = "task_create";

    type Error = TaskCreateError;
    type Args = TaskCreateArgs;
    type Output = TaskCreateOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/task_create").to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short task title" },
                    "description": { "type": "string", "description": "Optional detailed description" },
                    "priority": {
                        "type": "string",
                        "enum": crate::tasks::TaskPriority::ALL.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
                        "description": "Task priority"
                    },
                    "subtasks": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional checklist items"
                    },
                    "metadata": {
                        "type": "object",
                        "description": "Optional metadata object"
                    },
                    "worker_type": {
                        "type": "string",
                        "enum": crate::tasks::TaskWorkerType::ALL.iter().map(|w| w.to_string()).collect::<Vec<_>>(),
                        "description": "Execution plan: which worker kind runs this task. Omit to inherit the project default."
                    },
                    "project": {
                        "type": "string",
                        "description": "Execution plan: project name or ID the work belongs to. Required for opencode tasks."
                    },
                    "repo_id": {
                        "type": "string",
                        "description": "Execution plan: repo ID within the project. Needed with worktree_mode \"create\" on multi-repo projects."
                    },
                    "worktree_mode": {
                        "type": "string",
                        "enum": crate::tasks::TaskWorktreeMode::ALL.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
                        "description": "Execution plan: \"root\" runs in the checkout, \"existing\" in the worktree named by worktree_id, \"create\" makes a fresh worktree at spawn."
                    },
                    "worktree_id": {
                        "type": "string",
                        "description": "Execution plan: existing worktree ID (worktree_mode \"existing\")."
                    },
                    "required_skills": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Skills injected into the executing worker unconditionally. Use for procedures the worker must follow, not suggestions."
                    },
                    "depends_on": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "task": { "type": "integer", "description": "Task number this task depends on" },
                                "kind": {
                                    "type": "string",
                                    "enum": crate::tasks::TaskDependencyKind::ALL.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
                                    "description": "\"gate\" (default): blocked until the dependency is done. \"stack\": builds on the dependency's branch, blocked only until that branch exists."
                                }
                            },
                            "required": ["task"]
                        },
                        "description": "Tasks that must precede this one. Use when creating a pipeline so the board holds the ordering — dependents stay blocked until their edges clear."
                    }
                },
                "required": ["title"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let priority = TaskPriority::parse(&args.priority)
            .ok_or_else(|| TaskCreateError(format!("invalid priority: {}", args.priority)))?;
        let status = TaskStatus::PendingApproval;

        let depends_on = parse_dependency_args(&args.depends_on).map_err(TaskCreateError)?;

        let worker_type = args
            .worker_type
            .as_deref()
            .map(|value| {
                crate::tasks::TaskWorkerType::parse(value)
                    .ok_or_else(|| TaskCreateError(format!("invalid worker_type: {value}")))
            })
            .transpose()?;
        let worktree_mode = args
            .worktree_mode
            .as_deref()
            .map(|value| {
                crate::tasks::TaskWorktreeMode::parse(value)
                    .ok_or_else(|| TaskCreateError(format!("invalid worktree_mode: {value}")))
            })
            .transpose()?;

        if worktree_mode == Some(crate::tasks::TaskWorktreeMode::Existing)
            && args.worktree_id.is_none()
        {
            return Err(TaskCreateError(
                "worktree_mode \"existing\" requires worktree_id".into(),
            ));
        }

        let project_id = match &args.project {
            Some(reference) => match &self.project_store {
                Some(store) => Some(
                    resolve_project_reference(store, reference)
                        .await
                        .map_err(TaskCreateError)?,
                ),
                // No store wired here — keep the reference as given rather
                // than dropping the intent.
                None => Some(reference.clone()),
            },
            None => None,
        };

        if matches!(
            worker_type,
            Some(
                crate::tasks::TaskWorkerType::Opencode | crate::tasks::TaskWorkerType::Acp
            )
        ) && project_id.is_none()
            && args.worktree_id.is_none()
        {
            return Err(TaskCreateError(
                "opencode and acp tasks need a project (or an explicit worktree_id) so \
                 the worker has a directory to run in"
                    .into(),
            ));
        }

        if !args.required_skills.is_empty()
            && let Some(rc) = &self.runtime_config
        {
            validate_required_skills(rc, &args.required_skills).map_err(TaskCreateError)?;
        }

        let subtasks = args
            .subtasks
            .into_iter()
            .map(|title| TaskSubtask {
                title,
                completed: false,
            })
            .collect::<Vec<_>>();

        let task = self
            .task_store
            .create_with_dependencies(
                CreateTaskInput {
                    owner_agent_id: self.agent_id.clone(),
                    assigned_agent_id: Some(self.agent_id.clone()),
                    title: args.title,
                    description: args.description,
                    status,
                    priority,
                    subtasks,
                    metadata: args.metadata.unwrap_or_else(|| serde_json::json!({})),
                    source_memory_id: None,
                    created_by: self.created_by.clone(),
                    worker_type,
                    project_id,
                    repo_id: args.repo_id,
                    worktree_mode,
                    worktree_id: args.worktree_id,
                    required_skills: args.required_skills,
                    context: crate::tasks::TaskMutationContext::new(
                        crate::tasks::TaskAuthorKind::Agent,
                        Some(self.agent_id.to_string()),
                        crate::tasks::TaskMutationSource::Tool,
                    )
                    .with_summary(Some("Task created".to_string())),
                },
                &depends_on,
            )
            .await
            .map_err(|error| TaskCreateError(format!("{error}")))?;

        // Emit SSE event + notification so the dashboard updates in real time.
        if let Some(api_state) = &self.api_state {
            api_state
                .event_tx
                .send(crate::api::ApiEvent::TaskUpdated {
                    agent_id: task.effective_agent_id().to_string(),
                    task_number: task.task_number,
                    status: task.status.to_string(),
                    action: "created".to_string(),
                })
                .ok();
            if task.status == TaskStatus::PendingApproval {
                api_state.emit_notification(NewNotification {
                    kind: NotificationKind::TaskApproval,
                    severity: NotificationSeverity::Info,
                    title: task.title.clone(),
                    body: task.description.clone(),
                    agent_id: Some(task.effective_agent_id().to_string()),
                    related_entity_type: Some("task".to_string()),
                    related_entity_id: Some(task.task_number.to_string()),
                    action_url: Some(format!("/tasks/{}", task.task_number)),
                    metadata: None,
                });
            }
        }

        if let Some(working_memory) = &self.working_memory {
            let (event_type, summary, importance) = if task.status == TaskStatus::Done {
                (
                    crate::memory::WorkingMemoryEventType::Outcome,
                    format!("Task #{} completed: {}", task.task_number, task.title),
                    0.7,
                )
            } else {
                (
                    crate::memory::WorkingMemoryEventType::TaskUpdate,
                    format!(
                        "Task created #{}: {} (status: {})",
                        task.task_number, task.title, task.status
                    ),
                    0.5,
                )
            };
            working_memory
                .emit(event_type, summary)
                .importance(importance)
                .record();
        }

        Ok(TaskCreateOutput {
            success: true,
            task_number: task.task_number,
            status: task.status.to_string(),
            message: format!("Created task #{}: {}", task.task_number, task.title),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::memory::working::WorkingMemoryEvent;
    use crate::memory::{WorkingMemoryEventType, WorkingMemoryStore};
    use crate::tasks::store::setup_test_store;
    use chrono_tz::Tz;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::time::Duration;

    async fn wait_for_single_event(store: &WorkingMemoryStore) -> WorkingMemoryEvent {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let events = store
                    .get_recent_events(10, 0.0)
                    .await
                    .expect("working memory query");
                if let Some(event) = events.into_iter().next() {
                    break event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for working memory event")
    }

    #[tokio::test]
    async fn task_create_emits_task_update_for_new_tasks() {
        let task_store = Arc::new(setup_test_store().await);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite connect");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        let working_memory = WorkingMemoryStore::new(pool, Tz::UTC);

        let tool = TaskCreateTool::new(task_store, "agent-test", "branch")
            .with_working_memory(working_memory.clone());

        let output = tool
            .call(TaskCreateArgs {
                title: "Ship observation MVP".to_string(),
                description: Some("land the first packet".to_string()),
                priority: "medium".to_string(),
                subtasks: Vec::new(),
                metadata: None,
                worker_type: None,
                project: None,
                repo_id: None,
                worktree_mode: None,
                worktree_id: None,
                required_skills: Vec::new(),
                depends_on: Vec::new(),
            })
            .await
            .expect("task create should succeed");

        assert_eq!(output.status, "pending_approval");

        let event = wait_for_single_event(&working_memory).await;
        assert_eq!(event.event_type, WorkingMemoryEventType::TaskUpdate);
        assert_eq!(
            event.summary,
            "Task created #1: Ship observation MVP (status: pending_approval)"
        );
    }
}
