//! Answers that wait in browser storage until the writer saves them.
use anyhow::{bail, Context, Result};
use iroh::endpoint::Connection;
use serde::{Deserialize, Serialize};
use yappr::{
    choices::{self, ChoiceError, Decision, Submission},
    control::{self, ControlClient},
    proto, EndpointTicket,
};

use crate::storage::{self, Dir};

const KEY: &str = "yappr.outbox";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delivery {
    pub writer: String,
    pub ticket: String,
    pub submission: Submission,
    pub status: String,
    pub decision: Option<Decision>,
    pub error: Option<String>,
}

pub fn list(dir: &Dir) -> Result<Vec<Delivery>> {
    storage::read(dir, KEY)
}

pub fn queue(
    dir: &Dir,
    ticket: &EndpointTicket,
    question_id: String,
    value: String,
) -> Result<Delivery> {
    if !choices::valid_id(&question_id) {
        bail!("the question ID is not valid");
    }
    let _guard = storage::lock_outbox(dir)?;
    let mut deliveries = list(dir)?;
    let writer = ticket.endpoint_addr().id.to_string();
    if let Some(delivery) = deliveries.iter().find(|d| {
        d.writer == writer && d.submission.question_id == question_id && d.status != "rejected"
    }) {
        if delivery.submission.value != value {
            bail!("this device already selected a different option for this question");
        }
        return Ok(delivery.clone());
    }
    let delivery = Delivery {
        writer,
        ticket: ticket.to_string(),
        submission: Submission {
            id: choices::random_id(),
            question_id,
            value,
        },
        status: "queued".into(),
        decision: None,
        error: None,
    };
    deliveries.push(delivery.clone());
    storage::write(dir, KEY, &deliveries)?;
    Ok(delivery)
}

/// Send the answer to the writer. With `delivering`, the answer goes on the
/// connection that the writer opened to deliver its feed. Without it, the
/// page connects to the writer.
pub async fn deliver_with(
    dir: &Dir,
    client: &ControlClient,
    delivery: &Delivery,
    delivering: Option<&Connection>,
) -> Result<Decision> {
    if let Some(decision) = &delivery.decision {
        return Ok(decision.clone());
    }
    let ticket: EndpointTicket = delivery.ticket.parse()?;
    if ticket.endpoint_addr().id.to_string() != delivery.writer {
        bail!("the delivery ticket belongs to another writer");
    }
    let result = match delivering {
        Some(conn) => submit_on(conn, delivery.submission.clone()).await,
        None => {
            client
                .submit(ticket.endpoint_addr().clone(), delivery.submission.clone())
                .await
        }
    };
    // Read again after the request, because another task can change the list.
    let _guard = storage::lock_outbox(dir)?;
    let mut deliveries = list(dir)?;
    let saved = deliveries
        .iter_mut()
        .find(|d| d.submission.id == delivery.submission.id)
        .context("the pending submission does not exist")?;
    match result {
        Ok(decision) => {
            if decision.question_id != delivery.submission.question_id
                || decision.submission_id != delivery.submission.id
                || decision.value != delivery.submission.value
                || decision.responder != client.endpoint().id()
            {
                bail!("the writer returned a decision for a different submission");
            }
            saved.status = "delivered".into();
            saved.decision = Some(decision.clone());
            saved.error = None;
            storage::write(dir, KEY, &deliveries)?;
            Ok(decision)
        }
        Err(error) => {
            if error
                .downcast_ref::<ChoiceError>()
                .is_some_and(|e| e.code != "storage")
            {
                saved.status = "rejected".into();
            }
            saved.error = Some(format!("{error:#}"));
            storage::write(dir, KEY, &deliveries)?;
            Err(error)
        }
    }
}

/// Submit an answer on a connection that the writer opened.
async fn submit_on(conn: &Connection, submission: Submission) -> Result<Decision> {
    match control::exchange_on(conn, &control::Request::Submit { submission }).await? {
        control::Reply::Accepted { decision } => Ok(decision),
        // The text comes from the writer.
        control::Reply::Rejected { error } => Err(ChoiceError::new(
            &proto::printable(&error.code),
            &proto::printable(&error.message),
        )
        .into()),
        _ => bail!("the writer sent an unexpected response"),
    }
}
