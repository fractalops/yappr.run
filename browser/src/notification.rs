//! Send a saved notification answer from the page or service worker.
use std::{collections::BTreeMap, time::Duration};

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use yappr::{
    choices::{self, ChoiceError, Submission},
    control::ControlClient,
    net::{self, RelaySettings},
    EndpointTicket,
};

use crate::{
    readers::Readers,
    storage::{load_or_create_secret, Dir, RelayStore},
};

#[derive(Deserialize, Serialize)]
pub struct Snapshot {
    secret: String,
    tickets: BTreeMap<String, String>,
    relays: RelaySettings,
}

pub fn snapshot(dir: &Dir, book: Readers) -> anyhow::Result<Snapshot> {
    Ok(Snapshot {
        secret: data_encoding::HEXLOWER.encode(&load_or_create_secret(dir)?.to_bytes()),
        tickets: book
            .writers
            .into_iter()
            .filter(|(_, writer)| writer.enabled && !writer.delivers_only)
            .map(|(id, writer)| (id, writer.ticket))
            .collect(),
        relays: <RelaySettings as RelayStore>::load(dir)?,
    })
}

#[derive(Deserialize)]
pub struct Answer {
    pub writer: String,
    pub submission: Submission,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultKind {
    Sent,
    Retry,
    Closed,
}

pub async fn send(client: &ControlClient, ticket: &str, answer: Answer) -> ResultKind {
    let Ok(ticket) = ticket.parse::<EndpointTicket>() else {
        return ResultKind::Closed;
    };
    let addr = ticket.endpoint_addr();
    let submission = answer.submission;
    if addr.id.to_string() != answer.writer
        || !choices::valid_id(&submission.id)
        || !choices::valid_id(&submission.question_id)
        || !choices::valid_value(&submission.value)
    {
        return ResultKind::Closed;
    }
    let result = n0_future::time::timeout(
        Duration::from_secs(7),
        client.submit(net::relay_addr(addr.clone()), submission.clone()),
    )
    .await;
    match result {
        Ok(Ok(decision))
            if decision.question_id == submission.question_id
                && decision.submission_id == submission.id
                && decision.value == submission.value
                && decision.responder == client.endpoint().id() =>
        {
            ResultKind::Sent
        }
        Ok(Err(error))
            if error
                .downcast_ref::<ChoiceError>()
                .is_some_and(|e| e.code != "storage") =>
        {
            ResultKind::Closed
        }
        _ => ResultKind::Retry,
    }
}

async fn send_from_snapshot(snapshot: Snapshot, answer: Answer) -> ResultKind {
    let Some(ticket) = snapshot.tickets.get(&answer.writer) else {
        return ResultKind::Closed;
    };
    let Ok(secret) = snapshot.secret.parse::<iroh::SecretKey>() else {
        return ResultKind::Retry;
    };
    let Ok(Ok(endpoint)) =
        n0_future::time::timeout(Duration::from_secs(3), net::bind(secret, &snapshot.relays)).await
    else {
        return ResultKind::Retry;
    };
    let client = ControlClient::new(endpoint.clone());
    let result = send(&client, ticket, answer).await;
    client.close().await;
    endpoint.close().await;
    result
}

#[wasm_bindgen]
pub async fn notification_reply(snapshot: String, answer: String) -> Result<String, JsValue> {
    let error = |e: serde_json::Error| JsValue::from_str(&e.to_string());
    let snapshot = serde_json::from_str(&snapshot).map_err(error)?;
    let answer = serde_json::from_str(&answer).map_err(error)?;
    serde_json::to_string(&send_from_snapshot(snapshot, answer).await).map_err(error)
}
