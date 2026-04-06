use std::collections::HashMap;

use anyhow::{Result, anyhow};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use crate::internal::ai::tools::context::{
    UserInputAnswer, UserInputOption, UserInputQuestion, UserInputRequest, UserInputResponse,
};

const PLAN_ACTION_QUESTION_ID: &str = "plan_action";
const REFINEMENT_NOTE_QUESTION_ID: &str = "refinement_note";
const APPROVE_LABEL: &str = "Approve";
const REFINE_LABEL: &str = "Refine";
const CANCEL_LABEL: &str = "Cancel";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanCheckpointDecision {
    Approve,
    Refine { note: String },
    Cancel,
}

pub(crate) async fn prompt_for_plan_checkpoint_decision(
    user_input_tx: &UnboundedSender<UserInputRequest>,
    turn_id: u64,
    checkpoint_index: usize,
) -> Result<PlanCheckpointDecision> {
    let response = request_user_input(
        user_input_tx,
        format!("claudecode-plan-checkpoint-{turn_id}-{checkpoint_index}"),
        plan_checkpoint_question(),
    )
    .await?;
    let answer = response.answers.get(PLAN_ACTION_QUESTION_ID);
    let (selected, note) = selected_option_and_note(answer);

    match selected.as_deref() {
        Some(APPROVE_LABEL) => Ok(PlanCheckpointDecision::Approve),
        Some(REFINE_LABEL) => {
            let note = if let Some(note) = note {
                Some(note)
            } else {
                request_refinement_note(user_input_tx, turn_id, checkpoint_index).await?
            };
            match note {
                Some(note) => Ok(PlanCheckpointDecision::Refine { note }),
                None => Ok(PlanCheckpointDecision::Cancel),
            }
        }
        Some(CANCEL_LABEL) | None => Ok(PlanCheckpointDecision::Cancel),
        Some(other) => Err(anyhow!(
            "unexpected Claude plan checkpoint choice '{other}'"
        )),
    }
}

fn plan_checkpoint_question() -> UserInputQuestion {
    UserInputQuestion {
        id: PLAN_ACTION_QUESTION_ID.to_string(),
        header: "Plan".to_string(),
        question: "Claude finished planning. Approve execution, refine the plan, or cancel?"
            .to_string(),
        is_other: false,
        is_secret: false,
        options: Some(vec![
            UserInputOption {
                label: APPROVE_LABEL.to_string(),
                description: "Exit plan mode and start implementation.".to_string(),
            },
            UserInputOption {
                label: REFINE_LABEL.to_string(),
                description: "Keep planning and revise the structured plan.".to_string(),
            },
            UserInputOption {
                label: CANCEL_LABEL.to_string(),
                description: "Keep the plan, but do not start execution.".to_string(),
            },
        ]),
    }
}

fn refinement_note_question() -> UserInputQuestion {
    UserInputQuestion {
        id: REFINEMENT_NOTE_QUESTION_ID.to_string(),
        header: "Refine".to_string(),
        question: "What should Claude change about the current plan?".to_string(),
        is_other: false,
        is_secret: false,
        options: None,
    }
}

async fn request_refinement_note(
    user_input_tx: &UnboundedSender<UserInputRequest>,
    turn_id: u64,
    checkpoint_index: usize,
) -> Result<Option<String>> {
    let response = request_user_input(
        user_input_tx,
        format!("claudecode-plan-refine-note-{turn_id}-{checkpoint_index}"),
        refinement_note_question(),
    )
    .await?;
    Ok(response
        .answers
        .get(REFINEMENT_NOTE_QUESTION_ID)
        .and_then(extract_freeform_text))
}

async fn request_user_input(
    user_input_tx: &UnboundedSender<UserInputRequest>,
    call_id: String,
    question: UserInputQuestion,
) -> Result<UserInputResponse> {
    let (response_tx, response_rx) = oneshot::channel();
    user_input_tx
        .send(UserInputRequest {
            call_id,
            questions: vec![question],
            response_tx,
        })
        .map_err(|_| anyhow!("TUI user input channel is unavailable"))?;

    match response_rx.await {
        Ok(response) => Ok(response),
        Err(_) => Ok(UserInputResponse {
            answers: HashMap::new(),
        }),
    }
}

fn selected_option_and_note(answer: Option<&UserInputAnswer>) -> (Option<String>, Option<String>) {
    let Some(answer) = answer else {
        return (None, None);
    };

    let mut selected = None;
    let mut note = None;
    for value in &answer.answers {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("user_note: ") {
            let note_text = rest.trim();
            if !note_text.is_empty() {
                note = Some(note_text.to_string());
            }
            continue;
        }
        if selected.is_none() {
            selected = Some(trimmed.to_string());
        }
    }
    (selected, note)
}

fn extract_freeform_text(answer: &UserInputAnswer) -> Option<String> {
    for value in &answer.answers {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("user_note: ") {
            let note_text = rest.trim();
            if !note_text.is_empty() {
                return Some(note_text.to_string());
            }
            continue;
        }
        return Some(trimmed.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_input_answer(values: &[&str]) -> UserInputAnswer {
        UserInputAnswer {
            answers: values.iter().map(|value| (*value).to_string()).collect(),
        }
    }

    fn single_answer_response(question_id: &str, answer: UserInputAnswer) -> UserInputResponse {
        UserInputResponse {
            answers: HashMap::from([(question_id.to_string(), answer)]),
        }
    }

    #[tokio::test]
    async fn approve_selection_returns_approve() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UserInputRequest>();
        tokio::spawn(async move {
            let request = rx.recv().await.expect("request");
            assert_eq!(request.questions.len(), 1);
            assert_eq!(request.questions[0].id, PLAN_ACTION_QUESTION_ID);
            let _ = request.response_tx.send(single_answer_response(
                PLAN_ACTION_QUESTION_ID,
                user_input_answer(&[APPROVE_LABEL]),
            ));
        });

        let decision = prompt_for_plan_checkpoint_decision(&tx, 7, 0)
            .await
            .expect("decision");
        assert_eq!(decision, PlanCheckpointDecision::Approve);
    }

    #[tokio::test]
    async fn refine_selection_uses_inline_note_when_present() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UserInputRequest>();
        tokio::spawn(async move {
            let request = rx.recv().await.expect("request");
            let _ = request.response_tx.send(single_answer_response(
                PLAN_ACTION_QUESTION_ID,
                user_input_answer(&[REFINE_LABEL, "user_note: tighten the rollout step"]),
            ));
        });

        let decision = prompt_for_plan_checkpoint_decision(&tx, 9, 0)
            .await
            .expect("decision");
        assert_eq!(
            decision,
            PlanCheckpointDecision::Refine {
                note: "tighten the rollout step".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn refine_selection_requests_followup_note_when_inline_note_is_missing() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UserInputRequest>();
        tokio::spawn(async move {
            let request = rx.recv().await.expect("first request");
            assert_eq!(request.questions[0].id, PLAN_ACTION_QUESTION_ID);
            let _ = request.response_tx.send(single_answer_response(
                PLAN_ACTION_QUESTION_ID,
                user_input_answer(&[REFINE_LABEL]),
            ));

            let request = rx.recv().await.expect("second request");
            assert_eq!(request.questions[0].id, REFINEMENT_NOTE_QUESTION_ID);
            let _ = request.response_tx.send(single_answer_response(
                REFINEMENT_NOTE_QUESTION_ID,
                user_input_answer(&["narrow the first step and mention smoke coverage"]),
            ));
        });

        let decision = prompt_for_plan_checkpoint_decision(&tx, 12, 1)
            .await
            .expect("decision");
        assert_eq!(
            decision,
            PlanCheckpointDecision::Refine {
                note: "narrow the first step and mention smoke coverage".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn cancelled_refinement_followup_defaults_to_cancel() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UserInputRequest>();
        tokio::spawn(async move {
            let request = rx.recv().await.expect("first request");
            let _ = request.response_tx.send(single_answer_response(
                PLAN_ACTION_QUESTION_ID,
                user_input_answer(&[REFINE_LABEL]),
            ));

            let request = rx.recv().await.expect("second request");
            drop(request.response_tx);
        });

        let decision = prompt_for_plan_checkpoint_decision(&tx, 15, 2)
            .await
            .expect("decision");
        assert_eq!(decision, PlanCheckpointDecision::Cancel);
    }
}
