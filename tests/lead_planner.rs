pub use ai_team::{api, context, domain, model, openai};
#[path = "../src/agent/lead.rs"]
mod lead;

use ai_team::{
    context::discovery::RepositoryMap,
    domain::{
        project::{PlanStatus, ProjectRun, RunStatus},
        task::{NonEmptyString, PositiveLimit},
    },
    model::{FinishReason, ModelError, ModelLimits, ModelRequest, ModelResponse, Usage},
};
use serde_json::{Value, json};

struct FakeModel {
    body: String,
    request: Option<ModelRequest>,
}

impl lead::LeadModel for FakeModel {
    async fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.request = Some(request.clone());
        Ok(ModelResponse {
            content: Some(self.body.clone()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: 0,
                total_tokens: 2,
                estimated: false,
            },
            latency_ms: 1,
        })
    }
}

fn text(value: &str) -> NonEmptyString {
    NonEmptyString::parse("value", value).unwrap()
}

fn run() -> ProjectRun {
    ProjectRun {
        id: text("run"),
        project_id: text("project"),
        objective: text("Ship changes"),
        acceptance_criteria: vec![text("Tests pass")],
        token_budget: PositiveLimit::new("token_budget", 5000).unwrap(),
        status: RunStatus::Planning,
    }
}

fn discovery() -> RepositoryMap {
    RepositoryMap {
        files: vec!["src/a.rs".into(), "src/b.rs".into()],
        test_commands: vec!["cargo test".into()],
        ..RepositoryMap::default()
    }
}

fn task(id: &str, path: &str) -> Value {
    json!({
        "id": id, "project_id": "project", "project_run_id": "run",
        "title": "Change code", "role": "worker", "objective": "Ship feature",
        "depends_on": [], "allowed_paths": [path], "context_refs": [],
        "acceptance_criteria": ["Tests pass"], "verification_commands": ["cargo test"],
        "limits": {"max_input_tokens": 1000, "max_output_tokens": 500,
            "max_tool_calls": 10, "max_attempts": 1, "timeout_seconds": 600}
    })
}

fn plan() -> Value {
    json!({"id":"plan", "project_run_id":"run", "version":1,
        "risk_flags":[], "tasks":[task("a", "src/a.rs"), task("b", "src/b.rs")]})
}

async fn check(value: Value) -> Result<domain::project::ProposedPlan, lead::LeadError> {
    let mut model = FakeModel {
        body: value.to_string(),
        request: None,
    };
    let result = lead::propose(
        &mut model,
        &run(),
        &discovery(),
        "lead-run",
        ModelLimits {
            max_input_tokens: 12_000,
            max_output_tokens: 8_000,
        },
    )
    .await;
    let request = model.request.unwrap();
    assert_eq!(request.model_class, "reasoning");
    assert!(request.tools.is_empty());
    assert!(request.messages[1].content.contains("src/a.rs"));
    result
}

#[tokio::test]
async fn valid_plan_is_proposed_only() {
    let plan = check(plan()).await.unwrap();
    assert_eq!(plan.status, PlanStatus::Proposed);
    assert_eq!(plan.tasks.len(), 2);
}

/// Regresi (benchmark M5-010): model nyata mengisi `context_refs` dengan PATH FILE, padahal ContextBuilder hanya menerima
/// `artifact://<id>`; plan itu lolos validasi lalu setiap dispatch gagal `orchestrator.context`. Referensi yang bukan
/// artifact dibuang saat plan diparse (akses file tetap hanya lewat `allowed_paths`), referensi artifact dipertahankan.
#[tokio::test]
async fn non_artifact_context_refs_are_dropped_but_artifact_refs_survive() {
    let mut value = plan();
    value["tasks"][0]["context_refs"] = json!([
        "src/a.rs",
        "artifact://0b3f9c2e-1111-4222-8333-444455556666",
        "test/a.test.js"
    ]);
    value["tasks"][1]["context_refs"] = json!(["src/b.rs"]);
    let plan = check(value).await.unwrap();
    let refs = |i: usize| {
        plan.tasks[i]
            .context_refs
            .iter()
            .map(|r| r.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(refs(0), ["artifact://0b3f9c2e-1111-4222-8333-444455556666"]);
    assert!(refs(1).is_empty());
}

#[tokio::test]
async fn malformed_or_unverified_plan_is_rejected() {
    let mut missing = plan();
    missing["tasks"][0]["verification_commands"] = json!([]);
    let mut unknown = plan();
    unknown["tasks"][0]["verification_commands"] = json!(["rm -rf ."]);
    let mut forged = plan();
    forged["status"] = json!("APPROVED");
    let mut mismatched = plan();
    mismatched["tasks"][0]["project_id"] = json!("other");
    for (value, expected) in [
        (missing, "lead plan is invalid"),
        (
            unknown,
            "lead plan contains unapproved verification command",
        ),
        (forged, "lead response is invalid"),
        (mismatched, "lead plan project mismatch"),
    ] {
        assert_eq!(check(value).await.unwrap_err().to_string(), expected);
    }
}

#[tokio::test]
async fn cyclic_dependencies_are_rejected() {
    let mut value = plan();
    value["tasks"][0]["depends_on"] = json!(["b"]);
    value["tasks"][1]["depends_on"] = json!(["a"]);
    assert!(matches!(
        check(value).await,
        Err(lead::LeadError::InvalidPlan(_))
    ));
}

#[tokio::test]
async fn overlapping_and_unsafe_scopes_are_rejected() {
    for (first, second, expected) in [
        ("src/**", "src/b.rs", lead::LeadError::OverlappingScope),
        ("src/*", "src/b.rs", lead::LeadError::OverlappingScope),
        ("src/**", "src/*", lead::LeadError::OverlappingScope),
        ("src/*.rs", "src/b.rs", lead::LeadError::UnsafeScope),
    ] {
        let mut value = plan();
        value["tasks"][0]["allowed_paths"] = json!([first]);
        value["tasks"][1]["allowed_paths"] = json!([second]);
        assert_eq!(check(value).await.unwrap_err(), expected);
    }
}

#[tokio::test]
async fn over_budget_including_attempts_is_rejected() {
    let mut value = plan();
    value["tasks"][0]["limits"]["max_attempts"] = json!(3);
    assert_eq!(
        check(value).await.unwrap_err(),
        lead::LeadError::BudgetExceeded
    );
}
