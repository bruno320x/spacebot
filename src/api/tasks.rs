use super::state::ApiState;
use crate::error::{Error as CrateError, TaskError};
use crate::notifications::{NewNotification, NotificationKind, NotificationSeverity};
use crate::tasks::{TaskAuthorKind, TaskMutationContext, TaskMutationSource};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A task handler failure rendered as JSON.
///
/// A stale write needs more than a status code: the response carries the
/// revision the caller expected alongside the one actually stored, so a client
/// can refresh and retry without a second round trip.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TaskErrorBody {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<i64>,
}

#[derive(Debug)]
pub(super) struct TaskApiError {
    status: StatusCode,
    body: TaskErrorBody,
}

impl TaskApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: TaskErrorBody {
                error: message.into(),
                expected_revision: None,
                current_revision: None,
            },
        }
    }

    fn not_found(task_number: i64) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            format!("task #{task_number} not found"),
        )
    }

    fn unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "task store not initialized",
        )
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl IntoResponse for TaskApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<CrateError> for TaskApiError {
    fn from(error: CrateError) -> Self {
        let CrateError::Task(task_error) = error else {
            tracing::warn!(%error, "task handler error");
            return Self::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        };

        match *task_error {
            TaskError::NotFound { .. } | TaskError::RevisionNotFound { .. } => {
                Self::new(StatusCode::NOT_FOUND, task_error.to_string())
            }
            TaskError::RevisionConflict {
                expected, current, ..
            } => Self {
                status: StatusCode::CONFLICT,
                body: TaskErrorBody {
                    error: task_error.to_string(),
                    expected_revision: Some(expected),
                    current_revision: Some(current),
                },
            },
            TaskError::InvalidTransition { .. } | TaskError::Invalid(_) => {
                Self::new(StatusCode::BAD_REQUEST, task_error.to_string())
            }
            TaskError::Other(ref inner) => {
                tracing::warn!(error = %inner, "task store error");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, task_error.to_string())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct TaskListQuery {
    /// Convenience filter: matches tasks where owner OR assigned equals this value.
    #[serde(default)]
    agent_id: Option<String>,
    /// Filter by owner agent. Optional.
    #[serde(default)]
    owner_agent_id: Option<String>,
    /// Filter by assigned agent. Optional.
    #[serde(default)]
    assigned_agent_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    created_by: Option<String>,
    #[serde(default = "default_task_limit")]
    limit: i64,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct CreateTaskRequest {
    /// Agent that owns (created) this task.
    owner_agent_id: String,
    /// Agent assigned to execute. Defaults to `owner_agent_id`.
    #[serde(default)]
    assigned_agent_id: Option<String>,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    subtasks: Vec<crate::tasks::TaskSubtask>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    source_memory_id: Option<String>,
    #[serde(default)]
    created_by: Option<String>,
    #[serde(default)]
    worker_type: Option<crate::tasks::TaskWorkerType>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    worktree_mode: Option<crate::tasks::TaskWorktreeMode>,
    #[serde(default)]
    worktree_id: Option<String>,
    #[serde(default)]
    required_skills: Vec<String>,
    #[serde(default)]
    depends_on: Vec<TaskDependencyRequest>,
    #[serde(flatten)]
    attribution: MutationAttribution,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct TaskDependencyRequest {
    task: i64,
    #[serde(default)]
    kind: Option<crate::tasks::TaskDependencyKind>,
}

/// Who is performing a mutation and why, carried by every write endpoint and
/// recorded on the revision it produces.
#[derive(Default, Deserialize, utoipa::ToSchema)]
pub(super) struct MutationAttribution {
    /// `user` (default), `agent`, `worker`, or `system`.
    #[serde(default)]
    author_type: Option<String>,
    #[serde(default)]
    author_id: Option<String>,
    /// Which surface this call came from: `api` (default), `cli`, `portal`.
    #[serde(default)]
    source: Option<String>,
    /// One line on why the edit was made.
    #[serde(default)]
    edit_summary: Option<String>,
}

impl MutationAttribution {
    fn context(&self) -> Result<TaskMutationContext, TaskApiError> {
        let author_type = match self.author_type.as_deref() {
            None => TaskAuthorKind::User,
            Some(value) => TaskAuthorKind::parse(value).ok_or_else(|| {
                TaskApiError::bad_request(format!("invalid author_type: {value}"))
            })?,
        };
        let source = match self.source.as_deref() {
            None => TaskMutationSource::Api,
            Some(value) => TaskMutationSource::parse(value)
                .ok_or_else(|| TaskApiError::bad_request(format!("invalid source: {value}")))?,
        };
        Ok(
            TaskMutationContext::new(author_type, self.author_id.clone(), source)
                .with_summary(self.edit_summary.clone()),
        )
    }
}

/// Distinguish an omitted field from one explicitly set to `null`, so a client
/// can clear a description or unassign a task rather than only overwrite.
fn explicit_null<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

fn dependency_edges(
    requests: &[TaskDependencyRequest],
) -> Vec<(i64, crate::tasks::TaskDependencyKind)> {
    requests
        .iter()
        .map(|r| {
            (
                r.task,
                r.kind.unwrap_or(crate::tasks::TaskDependencyKind::Gate),
            )
        })
        .collect()
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct UpdateTaskRequest {
    #[serde(default)]
    title: Option<String>,
    /// Send `null` to clear. Omit to leave unchanged.
    #[serde(default, deserialize_with = "explicit_null")]
    description: Option<Option<String>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    /// Send `null` to unassign. Omit to leave unchanged.
    #[serde(default, deserialize_with = "explicit_null")]
    assigned_agent_id: Option<Option<String>>,
    #[serde(default)]
    subtasks: Option<Vec<crate::tasks::TaskSubtask>>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    complete_subtask: Option<usize>,
    #[serde(default)]
    worker_id: Option<String>,
    #[serde(default)]
    approved_by: Option<String>,
    #[serde(default, deserialize_with = "explicit_null")]
    worker_type: Option<Option<crate::tasks::TaskWorkerType>>,
    #[serde(default, deserialize_with = "explicit_null")]
    project_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "explicit_null")]
    repo_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "explicit_null")]
    worktree_mode: Option<Option<crate::tasks::TaskWorktreeMode>>,
    #[serde(default, deserialize_with = "explicit_null")]
    worktree_id: Option<Option<String>>,
    #[serde(default)]
    required_skills: Option<Vec<String>>,
    /// Full replacement of dependency edges; omit to leave unchanged.
    #[serde(default)]
    depends_on: Option<Vec<TaskDependencyRequest>>,
    /// The task's revision as the caller last read it. When supplied and stale,
    /// the update fails with 409 instead of overwriting the newer version.
    #[serde(default)]
    expected_revision: Option<i64>,
    #[serde(flatten)]
    attribution: MutationAttribution,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct ApproveRequest {
    #[serde(default)]
    approved_by: Option<String>,
    #[serde(flatten)]
    attribution: MutationAttribution,
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct TaskCommentListQuery {
    /// Resume after this comment `seq`. Comments are returned oldest-first.
    #[serde(default)]
    after: Option<i64>,
    #[serde(default = "default_comment_limit")]
    limit: i64,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct CreateTaskCommentRequest {
    /// Defaults to `user` — the interface is the human's comment surface.
    #[serde(default)]
    author_type: Option<String>,
    #[serde(default)]
    author_id: Option<String>,
    body: String,
    /// Worker run this comment reports on, when applicable.
    #[serde(default)]
    worker_id: Option<String>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct TaskHistoryQuery {
    #[serde(default = "default_history_limit")]
    limit: i64,
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct TaskDiffQuery {
    /// Revision to diff from.
    from: i64,
    /// Revision to diff to. Defaults to the task's current revision.
    #[serde(default)]
    to: Option<i64>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct RestoreRevisionRequest {
    /// The task's revision as the caller last read it. Required so a restore
    /// never silently discards an edit made while the user was deciding.
    expected_revision: i64,
    #[serde(flatten)]
    attribution: MutationAttribution,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskCommentListResponse {
    pub comments: Vec<crate::tasks::TaskComment>,
    /// Total comments on the task, independent of this page.
    pub total: i64,
    /// Cursor for the next page, absent when this page is the last one.
    pub next_cursor: Option<i64>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskCommentResponse {
    pub comment: crate::tasks::TaskComment,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskAttemptListResponse {
    /// Worker runs attempted against this task, newest first.
    pub attempts: Vec<crate::tasks::TaskAttempt>,
    /// One line summarising what has been tried, as prompt context renders it.
    pub summary: Option<String>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskHistoryResponse {
    pub revisions: Vec<crate::tasks::TaskRevisionSummary>,
    /// The task's current revision number.
    pub current: i64,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskRevisionResponse {
    pub revision: crate::tasks::TaskRevision,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct AssignRequest {
    assigned_agent_id: String,
    #[serde(flatten)]
    attribution: MutationAttribution,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskListResponse {
    pub tasks: Vec<crate::tasks::Task>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskResponse {
    pub task: crate::tasks::Task,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskActionResponse {
    pub success: bool,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_task_limit() -> i64 {
    100
}

fn default_comment_limit() -> i64 {
    50
}

fn default_history_limit() -> i64 {
    50
}

/// Extract the global task store, returning 503 if not yet initialized.
fn get_task_store(state: &ApiState) -> Result<Arc<crate::tasks::TaskStore>, StatusCode> {
    state
        .task_store
        .load()
        .as_ref()
        .clone()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

/// Same, for handlers that return the JSON error body.
fn task_store(state: &ApiState) -> Result<Arc<crate::tasks::TaskStore>, TaskApiError> {
    state
        .task_store
        .load()
        .as_ref()
        .clone()
        .ok_or_else(TaskApiError::unavailable)
}

fn parse_status(value: Option<&str>) -> Result<Option<crate::tasks::TaskStatus>, StatusCode> {
    match value {
        None => Ok(None),
        Some(value) => Ok(Some(
            crate::tasks::TaskStatus::parse(value).ok_or(StatusCode::BAD_REQUEST)?,
        )),
    }
}

fn parse_priority(value: Option<&str>) -> Result<Option<crate::tasks::TaskPriority>, StatusCode> {
    match value {
        None => Ok(None),
        Some(value) => Ok(Some(
            crate::tasks::TaskPriority::parse(value).ok_or(StatusCode::BAD_REQUEST)?,
        )),
    }
}

fn emit_task_event(state: &ApiState, task: &crate::tasks::Task, action: &str) {
    state
        .event_tx
        .send(super::state::ApiEvent::TaskUpdated {
            agent_id: task.effective_agent_id().to_string(),
            task_number: task.task_number,
            status: task.status.to_string(),
            action: action.to_string(),
        })
        .ok();
}

/// Announce a committed revision. Called after the transaction, and only when
/// one was actually written — a no-op update emits nothing.
fn emit_task_revised(
    state: &ApiState,
    task: &crate::tasks::Task,
    new_revision: Option<i64>,
    restored_from: Option<i64>,
) {
    let Some(revision) = new_revision else {
        return;
    };
    state
        .event_tx
        .send(super::state::ApiEvent::TaskRevised {
            agent_id: task.effective_agent_id().to_string(),
            task_number: task.task_number,
            revision,
            restored_from,
        })
        .ok();
}

/// Announce a committed comment.
fn emit_task_commented(
    state: &ApiState,
    task: &crate::tasks::Task,
    comment: &crate::tasks::TaskComment,
) {
    state
        .event_tx
        .send(super::state::ApiEvent::TaskCommented {
            agent_id: task.effective_agent_id().to_string(),
            task_number: task.task_number,
            comment_id: comment.id.clone(),
            seq: comment.seq,
            author_type: comment.author_type.to_string(),
        })
        .ok();
}

/// Emit a task_approval notification when a task enters the pending_approval state.
fn maybe_emit_approval_notification(state: &ApiState, task: &crate::tasks::Task) {
    if task.status != crate::tasks::TaskStatus::PendingApproval {
        return;
    }
    state.emit_notification(NewNotification {
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

/// Post-mutation fan-out shared by the task handlers: SSE event, approval
/// notification, and — when the mutation transitioned the task onto Ready —
/// a task-approved system event routed to the owning agent's wake queue.
async fn finish_task_mutation(
    state: &ApiState,
    task: &crate::tasks::Task,
    action: &str,
    previous_status: Option<crate::tasks::TaskStatus>,
) {
    emit_task_event(state, task, action);
    maybe_emit_approval_notification(state, task);

    let landed_on_ready = task.status == crate::tasks::TaskStatus::Ready
        && previous_status.is_some_and(|previous| previous != crate::tasks::TaskStatus::Ready);
    if !landed_on_ready {
        return;
    }

    let key: crate::AgentId = Arc::from(task.effective_agent_id());
    let deps = state.wake_registry.read().await.get(&key).cloned();
    let Some(deps) = deps else {
        return;
    };

    let mut payload = serde_json::json!({
        "task_number": task.task_number,
        "title": task.title,
        "action": action,
    });
    if let Some(approved_by) = &task.approved_by {
        payload["approved_by"] = serde_json::Value::from(approved_by.clone());
    }
    crate::wakes::emit_system_event(
        &deps,
        crate::wakes::SystemEvent::TaskApproved,
        &format!("task:{}", task.task_number),
        &payload,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /tasks` — list tasks with optional filters.
#[utoipa::path(
    get,
    path = "/tasks",
    params(TaskListQuery),
    responses(
        (status = 200, body = TaskListResponse),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn list_tasks(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<TaskListQuery>,
) -> Result<Json<TaskListResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    let status = parse_status(query.status.as_deref())?;
    let priority = parse_priority(query.priority.as_deref())?;

    let tasks = store
        .list(crate::tasks::TaskListFilter {
            agent_id: query.agent_id,
            owner_agent_id: query.owner_agent_id,
            assigned_agent_id: query.assigned_agent_id,
            status,
            priority,
            created_by: query.created_by,
            limit: Some(query.limit.clamp(1, 500)),
        })
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to list tasks");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(TaskListResponse { tasks }))
}

/// `GET /tasks/{number}` — get a task by globally unique number.
#[utoipa::path(
    get,
    path = "/tasks/{number}",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    responses(
        (status = 200, body = TaskResponse),
        (status = 404, description = "Task not found"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn get_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
) -> Result<Json<TaskResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    let task = store
        .get_by_number(number)
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_number = number, "failed to get task");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(TaskResponse { task }))
}

/// `POST /tasks` — create a task.
#[utoipa::path(
    post,
    path = "/tasks",
    request_body = CreateTaskRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 400, description = "Invalid request"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn create_task(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<CreateTaskRequest>,
) -> Result<Json<TaskResponse>, TaskApiError> {
    let store = task_store(&state)?;

    let status = crate::tasks::TaskStatus::PendingApproval;
    let priority =
        crate::tasks::TaskPriority::parse(request.priority.as_deref().unwrap_or("medium"))
            .ok_or_else(|| TaskApiError::bad_request("invalid priority"))?;

    let assigned = request
        .assigned_agent_id
        .unwrap_or_else(|| request.owner_agent_id.clone());
    let mut context = request.attribution.context()?;
    if context.edit_summary.is_none() {
        context.edit_summary = Some("Task created".to_string());
    }

    let ceiling = **state.autonomy_ceiling.load();
    let ceiling_mode = crate::mode::autonomy_ceiling_mode(ceiling);
    let task = store
        .create_with_dependencies(
            crate::tasks::CreateTaskInput {
                owner_agent_id: request.owner_agent_id,
                assigned_agent_id: Some(assigned),
                title: request.title,
                description: request.description,
                status,
                priority,
                subtasks: request.subtasks,
                metadata: request.metadata.unwrap_or_else(|| serde_json::json!({})),
                // API-created tasks run the standard goal loop, clamped so
                // they never exceed the configured autonomy ceiling and never
                // start at Yolo implicitly.
                mode: Some(crate::mode::TaskMode::Goal.clamp_to(ceiling_mode)),
                source_memory_id: request.source_memory_id,
                created_by: request.created_by.unwrap_or_else(|| "human".to_string()),
                worker_type: request.worker_type,
                project_id: request.project_id,
                repo_id: request.repo_id,
                worktree_mode: request.worktree_mode,
                worktree_id: request.worktree_id,
                required_skills: request.required_skills,
                context,
            },
            &dependency_edges(&request.depends_on),
        )
        .await?;

    finish_task_mutation(&state, &task, "created", None).await;
    emit_task_revised(&state, &task, Some(task.revision), None);
    Ok(Json(TaskResponse { task }))
}

/// `PUT /tasks/{number}` — update a task.
#[utoipa::path(
    put,
    path = "/tasks/{number}",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    request_body = UpdateTaskRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Task not found"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn update_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Json(request): Json<UpdateTaskRequest>,
) -> Result<Json<TaskResponse>, TaskApiError> {
    let store = task_store(&state)?;

    let status = match request.status.as_deref() {
        None => None,
        Some(value) => Some(
            crate::tasks::TaskStatus::parse(value)
                .ok_or_else(|| TaskApiError::bad_request(format!("invalid status: {value}")))?,
        ),
    };
    let priority = match request.priority.as_deref() {
        None => None,
        Some(value) => Some(
            crate::tasks::TaskPriority::parse(value)
                .ok_or_else(|| TaskApiError::bad_request(format!("invalid priority: {value}")))?,
        ),
    };

    let edges = request.depends_on.as_deref().map(dependency_edges);
    let context = request
        .attribution
        .context()?
        .expecting(request.expected_revision);

    let update = store
        .update_with_dependencies_and_status_override(
            number,
            crate::tasks::UpdateTaskInput {
                title: request.title,
                description: request.description,
                status,
                priority,
                assigned_agent_id: request.assigned_agent_id,
                subtasks: request.subtasks,
                metadata: request.metadata,
                replace_metadata: false,
                worker_id: request.worker_id,
                clear_worker_id: false,
                approved_by: request.approved_by,
                complete_subtask: request.complete_subtask,
                worker_type: request.worker_type,
                project_id: request.project_id,
                repo_id: request.repo_id,
                worktree_mode: request.worktree_mode,
                worktree_id: request.worktree_id,
                goal_id: None,
                required_skills: request.required_skills,
                context,
            },
            edges.as_deref(),
        )
        .await?
        .ok_or_else(|| TaskApiError::not_found(number))?;

    finish_task_mutation(
        &state,
        &update.task,
        "updated",
        Some(update.previous_status),
    )
    .await;
    emit_task_revised(&state, &update.task, update.new_revision, None);
    Ok(Json(TaskResponse { task: update.task }))
}

/// `DELETE /tasks/{number}` — delete a task.
#[utoipa::path(
    delete,
    path = "/tasks/{number}",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    responses(
        (status = 200, body = TaskActionResponse),
        (status = 404, description = "Task not found"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn delete_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
) -> Result<Json<TaskActionResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    // Fetch before delete so we can emit an event with the correct agent_id.
    let task = store
        .get_by_number(number)
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_number = number, "failed to get task for deletion");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let deleted = store.delete(number).await.map_err(|error| {
        tracing::warn!(%error, task_number = number, "failed to delete task");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if !deleted {
        return Err(StatusCode::NOT_FOUND);
    }

    state
        .event_tx
        .send(super::state::ApiEvent::TaskUpdated {
            agent_id: task.effective_agent_id().to_string(),
            task_number: number,
            status: "deleted".to_string(),
            action: "deleted".to_string(),
        })
        .ok();

    Ok(Json(TaskActionResponse {
        success: true,
        message: format!("Task #{number} deleted"),
    }))
}

/// `POST /tasks/{number}/approve` — approve a task (move to ready).
#[utoipa::path(
    post,
    path = "/tasks/{number}/approve",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    request_body = ApproveRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 404, description = "Task not found"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn approve_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Json(request): Json<ApproveRequest>,
) -> Result<Json<TaskResponse>, TaskApiError> {
    let store = task_store(&state)?;

    let mut context = request.attribution.context()?;
    if context.edit_summary.is_none() {
        context.edit_summary = Some("Approved".to_string());
    }
    if context.author_id.is_none() {
        context.author_id = request.approved_by.clone();
    }

    let update = store
        .update_with_status_transition(
            number,
            crate::tasks::UpdateTaskInput {
                status: Some(crate::tasks::TaskStatus::Ready),
                approved_by: request.approved_by,
                context,
                ..Default::default()
            },
        )
        .await?
        .ok_or_else(|| TaskApiError::not_found(number))?;

    finish_task_mutation(
        &state,
        &update.task,
        "updated",
        Some(update.previous_status),
    )
    .await;
    emit_task_revised(&state, &update.task, update.new_revision, None);
    // Auto-dismiss any pending task_approval notification for this task.
    if let Some(store) = state.notification_store.load().as_ref().clone()
        && let Err(error) = store
            .dismiss_by_entity("task_approval", "task", &number.to_string())
            .await
    {
        tracing::warn!(%error, task_number = number, "failed to auto-dismiss approval notification");
    }
    Ok(Json(TaskResponse { task: update.task }))
}

/// `POST /tasks/{number}/execute` — move a task to ready for execution.
/// Tasks already in `ready` or `in_progress` are returned as-is.
#[utoipa::path(
    post,
    path = "/tasks/{number}/execute",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    request_body = ApproveRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 404, description = "Task not found"),
        (status = 409, description = "Task pending approval"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn execute_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Json(request): Json<ApproveRequest>,
) -> Result<Json<TaskResponse>, TaskApiError> {
    let store = task_store(&state)?;

    let current = store
        .get_by_number(number)
        .await?
        .ok_or_else(|| TaskApiError::not_found(number))?;

    if matches!(
        current.status,
        crate::tasks::TaskStatus::Ready | crate::tasks::TaskStatus::InProgress
    ) {
        return Ok(Json(TaskResponse { task: current }));
    }

    // Reject pending_approval tasks — they must be approved first.
    if current.status == crate::tasks::TaskStatus::PendingApproval {
        return Err(TaskApiError::new(
            StatusCode::CONFLICT,
            format!("task #{number} is pending approval"),
        ));
    }

    let mut context = request.attribution.context()?;
    if context.edit_summary.is_none() {
        context.edit_summary = Some("Queued for execution".to_string());
    }

    let update = store
        .update_with_status_transition(
            number,
            crate::tasks::UpdateTaskInput {
                status: Some(crate::tasks::TaskStatus::Ready),
                approved_by: request.approved_by,
                context,
                ..Default::default()
            },
        )
        .await?
        .ok_or_else(|| TaskApiError::not_found(number))?;

    finish_task_mutation(
        &state,
        &update.task,
        "updated",
        Some(update.previous_status),
    )
    .await;
    emit_task_revised(&state, &update.task, update.new_revision, None);
    Ok(Json(TaskResponse { task: update.task }))
}

/// `POST /tasks/{number}/assign` — reassign a task to a different agent.
#[utoipa::path(
    post,
    path = "/tasks/{number}/assign",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    request_body = AssignRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 404, description = "Task not found"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn assign_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Json(request): Json<AssignRequest>,
) -> Result<Json<TaskResponse>, TaskApiError> {
    let store = task_store(&state)?;

    let update = store
        .update_with_status_transition(
            number,
            crate::tasks::UpdateTaskInput {
                assigned_agent_id: Some(Some(request.assigned_agent_id.clone())),
                context: request
                    .attribution
                    .context()?
                    .with_summary(Some(format!("Assigned to {}", request.assigned_agent_id))),
                ..Default::default()
            },
        )
        .await?
        .ok_or_else(|| TaskApiError::not_found(number))?;

    finish_task_mutation(&state, &update.task, "updated", None).await;
    emit_task_revised(&state, &update.task, update.new_revision, None);
    Ok(Json(TaskResponse { task: update.task }))
}

// ---------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------

/// `GET /tasks/{number}/comments` — list a task's comments, oldest first.
#[utoipa::path(
    get,
    path = "/tasks/{number}/comments",
    params(
        ("number" = i64, Path, description = "Task number"),
        TaskCommentListQuery,
    ),
    responses(
        (status = 200, body = TaskCommentListResponse),
        (status = 404, description = "Task not found", body = TaskErrorBody),
        (status = 503, description = "Task store not initialized", body = TaskErrorBody),
    ),
    tag = "tasks",
)]
pub(super) async fn list_task_comments(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Query(query): Query<TaskCommentListQuery>,
) -> Result<Json<TaskCommentListResponse>, TaskApiError> {
    let store = task_store(&state)?;

    // Distinguish "no comments" from "no task" before reading the page.
    store
        .get_by_number(number)
        .await?
        .ok_or_else(|| TaskApiError::not_found(number))?;

    let limit = query.limit.clamp(1, crate::tasks::MAX_COMMENT_PAGE);
    let comments = store.list_comments(number, limit, query.after).await?;
    let total = store.count_comments(number).await?;

    let next_cursor = (comments.len() as i64 == limit)
        .then(|| comments.last().map(|comment| comment.seq))
        .flatten();

    Ok(Json(TaskCommentListResponse {
        comments,
        total,
        next_cursor,
    }))
}

/// `GET /tasks/{number}/attempts` — the worker runs attempted against a task.
///
/// `tasks.worker_id` names only the run executing right now; this is the
/// history that says what has already been tried and how it ended.
#[utoipa::path(
    get,
    path = "/tasks/{number}/attempts",
    params(("number" = i64, Path, description = "Task number")),
    responses(
        (status = 200, body = TaskAttemptListResponse),
        (status = 404, description = "Task not found", body = TaskErrorBody),
        (status = 503, description = "Task store not initialized", body = TaskErrorBody),
    ),
    tag = "tasks",
)]
pub(super) async fn list_task_attempts(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
) -> Result<Json<TaskAttemptListResponse>, TaskApiError> {
    let store = task_store(&state)?;

    // Distinguish "never attempted" from "no such task".
    store
        .get_by_number(number)
        .await?
        .ok_or_else(|| TaskApiError::not_found(number))?;

    let attempts = store
        .list_task_attempts(number, crate::tasks::MAX_ATTEMPT_PAGE)
        .await?;
    let summary = crate::tasks::render_prior_attempts(&attempts);

    Ok(Json(TaskAttemptListResponse { attempts, summary }))
}

/// `POST /tasks/{number}/comments` — append a comment to a task.
#[utoipa::path(
    post,
    path = "/tasks/{number}/comments",
    params(("number" = i64, Path, description = "Task number")),
    request_body = CreateTaskCommentRequest,
    responses(
        (status = 200, body = TaskCommentResponse),
        (status = 400, description = "Invalid comment", body = TaskErrorBody),
        (status = 404, description = "Task not found", body = TaskErrorBody),
        (status = 503, description = "Task store not initialized", body = TaskErrorBody),
    ),
    tag = "tasks",
)]
pub(super) async fn create_task_comment(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Json(request): Json<CreateTaskCommentRequest>,
) -> Result<Json<TaskCommentResponse>, TaskApiError> {
    let store = task_store(&state)?;

    let author_type = match request.author_type.as_deref() {
        None => TaskAuthorKind::User,
        Some(value) => TaskAuthorKind::parse(value)
            .ok_or_else(|| TaskApiError::bad_request(format!("invalid author_type: {value}")))?,
    };

    let task = store
        .get_by_number(number)
        .await?
        .ok_or_else(|| TaskApiError::not_found(number))?;

    let comment = store
        .add_comment(crate::tasks::CreateTaskCommentInput {
            task_number: number,
            author_type,
            author_id: request.author_id,
            body: request.body,
            worker_id: request.worker_id,
            metadata: request.metadata.unwrap_or_else(|| serde_json::json!({})),
        })
        .await?;

    emit_task_commented(&state, &task, &comment);
    Ok(Json(TaskCommentResponse { comment }))
}

// ---------------------------------------------------------------------------
// Revision history
// ---------------------------------------------------------------------------

/// `GET /tasks/{number}/revisions` — list revision summaries, newest first.
#[utoipa::path(
    get,
    path = "/tasks/{number}/revisions",
    params(
        ("number" = i64, Path, description = "Task number"),
        TaskHistoryQuery,
    ),
    responses(
        (status = 200, body = TaskHistoryResponse),
        (status = 404, description = "Task not found", body = TaskErrorBody),
        (status = 503, description = "Task store not initialized", body = TaskErrorBody),
    ),
    tag = "tasks",
)]
pub(super) async fn list_task_revisions(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Query(query): Query<TaskHistoryQuery>,
) -> Result<Json<TaskHistoryResponse>, TaskApiError> {
    let store = task_store(&state)?;

    let task = store
        .get_by_number(number)
        .await?
        .ok_or_else(|| TaskApiError::not_found(number))?;
    let revisions = store.list_revisions(number, query.limit).await?;

    Ok(Json(TaskHistoryResponse {
        revisions,
        current: task.revision,
    }))
}

/// `GET /tasks/{number}/revisions/diff` — compare two points in a task's
/// history. `to` defaults to the current revision.
#[utoipa::path(
    get,
    path = "/tasks/{number}/revisions/diff",
    params(
        ("number" = i64, Path, description = "Task number"),
        TaskDiffQuery,
    ),
    responses(
        (status = 200, body = crate::tasks::TaskRevisionDiff),
        (status = 404, description = "Task or revision not found", body = TaskErrorBody),
        (status = 503, description = "Task store not initialized", body = TaskErrorBody),
    ),
    tag = "tasks",
)]
pub(super) async fn diff_task_revisions(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Query(query): Query<TaskDiffQuery>,
) -> Result<Json<crate::tasks::TaskRevisionDiff>, TaskApiError> {
    let store = task_store(&state)?;
    let diff = store.diff_revisions(number, query.from, query.to).await?;
    Ok(Json(diff))
}

/// `GET /tasks/{number}/revisions/{revision}` — read one historical revision.
#[utoipa::path(
    get,
    path = "/tasks/{number}/revisions/{revision}",
    params(
        ("number" = i64, Path, description = "Task number"),
        ("revision" = i64, Path, description = "Revision number"),
    ),
    responses(
        (status = 200, body = TaskRevisionResponse),
        (status = 404, description = "Task or revision not found", body = TaskErrorBody),
        (status = 503, description = "Task store not initialized", body = TaskErrorBody),
    ),
    tag = "tasks",
)]
pub(super) async fn get_task_revision(
    State(state): State<Arc<ApiState>>,
    Path((number, revision)): Path<(i64, i64)>,
) -> Result<Json<TaskRevisionResponse>, TaskApiError> {
    let store = task_store(&state)?;
    let revision = store
        .get_revision(number, revision)
        .await?
        .ok_or_else(|| TaskApiError::not_found(number))?;
    Ok(Json(TaskRevisionResponse { revision }))
}

/// `POST /tasks/{number}/revisions/{revision}/restore` — put a task back to a
/// historical revision by appending a new one.
///
/// Nothing is rewound: revision `revision` and everything after it stay exactly
/// as they were, and the restore lands as the new latest version.
#[utoipa::path(
    post,
    path = "/tasks/{number}/revisions/{revision}/restore",
    params(
        ("number" = i64, Path, description = "Task number"),
        ("revision" = i64, Path, description = "Revision to restore"),
    ),
    request_body = RestoreRevisionRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 400, description = "Restore rejected by task rules", body = TaskErrorBody),
        (status = 404, description = "Task or revision not found", body = TaskErrorBody),
        (status = 409, description = "Task changed since the caller read it", body = TaskErrorBody),
        (status = 503, description = "Task store not initialized", body = TaskErrorBody),
    ),
    tag = "tasks",
)]
pub(super) async fn restore_task_revision(
    State(state): State<Arc<ApiState>>,
    Path((number, revision)): Path<(i64, i64)>,
    Json(request): Json<RestoreRevisionRequest>,
) -> Result<Json<TaskResponse>, TaskApiError> {
    let store = task_store(&state)?;

    let mut context = request
        .attribution
        .context()?
        .expecting(Some(request.expected_revision));
    context.source = TaskMutationSource::Restore;
    if context.edit_summary.is_none() {
        context.edit_summary = Some(format!("Restored revision {revision}"));
    }

    let update = store.restore_revision(number, revision, context).await?;

    finish_task_mutation(
        &state,
        &update.task,
        "updated",
        Some(update.previous_status),
    )
    .await;
    emit_task_revised(&state, &update.task, update.new_revision, Some(revision));
    Ok(Json(TaskResponse { task: update.task }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `#[serde(flatten)]` routes through a buffering deserializer, so the
    /// attribution fields and the explicit-null patch fields have to be proven
    /// to survive it rather than assumed to.
    #[test]
    fn update_request_carries_attribution_alongside_patch_fields() {
        let request: UpdateTaskRequest = serde_json::from_value(serde_json::json!({
            "title": "renamed",
            "description": null,
            "project_id": "proj-1",
            "expected_revision": 4,
            "author_type": "user",
            "author_id": "jamie",
            "source": "portal",
            "edit_summary": "Scope tightened",
        }))
        .expect("request should deserialize");

        assert_eq!(request.title.as_deref(), Some("renamed"));
        assert_eq!(request.description, Some(None), "null must clear");
        assert_eq!(request.project_id, Some(Some("proj-1".to_string())));
        assert_eq!(request.repo_id, None, "an omitted field stays unset");
        assert_eq!(request.expected_revision, Some(4));

        let context = request.attribution.context().expect("attribution is valid");
        assert_eq!(context.author_type, TaskAuthorKind::User);
        assert_eq!(context.author_id.as_deref(), Some("jamie"));
        assert_eq!(context.source, TaskMutationSource::Portal);
        assert_eq!(context.edit_summary.as_deref(), Some("Scope tightened"));
    }

    #[test]
    fn an_empty_update_body_defaults_to_a_user_edit_from_the_api() {
        let request: UpdateTaskRequest =
            serde_json::from_value(serde_json::json!({})).expect("request should deserialize");

        assert_eq!(request.description, None);
        assert_eq!(request.assigned_agent_id, None);

        let context = request.attribution.context().expect("attribution is valid");
        assert_eq!(context.author_type, TaskAuthorKind::User);
        assert_eq!(context.source, TaskMutationSource::Api);
    }

    #[test]
    fn create_and_restore_requests_accept_attribution() {
        let create: CreateTaskRequest = serde_json::from_value(serde_json::json!({
            "owner_agent_id": "main",
            "title": "new work",
            "source": "cli",
        }))
        .expect("create request should deserialize");
        assert_eq!(
            create.attribution.context().expect("valid").source,
            TaskMutationSource::Cli
        );

        let restore: RestoreRevisionRequest = serde_json::from_value(serde_json::json!({
            "expected_revision": 7,
            "edit_summary": "The agent overwrote the spec",
        }))
        .expect("restore request should deserialize");
        assert_eq!(restore.expected_revision, 7);
        assert_eq!(
            restore
                .attribution
                .context()
                .expect("valid")
                .edit_summary
                .as_deref(),
            Some("The agent overwrote the spec")
        );
    }

    #[test]
    fn an_unknown_source_is_a_bad_request_not_a_silent_default() {
        let request: UpdateTaskRequest =
            serde_json::from_value(serde_json::json!({ "source": "telepathy" }))
                .expect("request should deserialize");

        let error = request
            .attribution
            .context()
            .expect_err("unknown source should be rejected");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }
}
