//! The page commands and the tasks that read, receive, pair, and answer.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use crate::{
    outbox::{self, Delivery},
    readers::{self, PendingPair, Readers},
    storage::{load_or_create_secret, Dir, RelayStore},
};
use iroh::{endpoint::Connection, Endpoint, EndpointId};
use n0_future::task::JoinHandle;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, Semaphore};
use wasm_bindgen::prelude::*;
use yappr::{
    choices::{self, QuestionState},
    client::{Denied, Event, Follower},
    control::ControlClient,
    deliver,
    entry::Entry,
    health::{self, Health},
    net::{self, RelaySettings},
    pairing::{self, Invitation},
    proto::{Frame, Request},
    EndpointTicket,
};

/// How many deliveries the page takes at the same time.
const MAX_DELIVERIES: usize = 8;

#[derive(Debug, Clone, Serialize)]
struct WriterView {
    id: String,
    name: String,
    ticket: String,
    log: Option<String>,
    entries: Vec<Entry>,
    questions: Vec<QuestionState>,
    enabled: bool,
    /// The machine has a delivery connection open to this page.
    delivering: bool,
    health: Health,
}
#[derive(Debug, Clone, Serialize)]
struct View {
    device_id: String,
    writers: Vec<WriterView>,
    selected: Option<String>,
    deliveries: Vec<Delivery>,
    pairing: bool,
    pairing_code: Option<String>,
    pairing_name: Option<String>,
    network: RelaySettings,
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UiEvent {
    View {
        view: View,
    },
    Health {
        writer_id: String,
        health: Health,
    },
    Status {
        text: String,
    },
    PairFailed {
        message: String,
    },
    /// The pairing request: "sending", "waiting" (the writer has it), or
    /// "retrying" (the writer is not reachable yet).
    PairState {
        state: String,
    },
}
#[derive(Clone)]
struct Runtime {
    endpoint: Endpoint,
    client: ControlClient,
}
#[derive(Default)]
struct AppState {
    runtime: Mutex<Option<Runtime>>,
    /// Takes the deliveries to the runtime's endpoint.
    receiver: Mutex<Option<JoinHandle<()>>>,
    book: Mutex<Option<Readers>>,
    tasks: Mutex<BTreeMap<String, JoinHandle<()>>>,
    pair_task: Mutex<Option<JoinHandle<()>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    health: Mutex<BTreeMap<String, Health>>,
    /// The open delivery connection of each machine.
    delivering: Mutex<BTreeMap<String, Connection>>,
    lifecycle: Mutex<()>,
    wake: Notify,
}
/// The page callback for events.
#[derive(Clone)]
struct AppHandle {
    on_event: js_sys::Function,
}
impl AppHandle {
    /// Send an event to the page as a JSON string.
    fn emit(&self, event: UiEvent) {
        if let Ok(json) = serde_json::to_string(&event) {
            self.on_event
                .call1(&JsValue::NULL, &JsValue::from_str(&json))
                .ok();
        }
    }
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
async fn book(state: &AppState) -> Result<Readers, String> {
    let mut slot = state.book.lock().await;
    if slot.is_none() {
        *slot = Some(Readers::load(&Dir).map_err(err)?);
    }
    Ok(slot.as_ref().expect("book exists").clone())
}
async fn update<T>(
    state: &AppState,
    apply: impl FnOnce(&mut Readers) -> Result<T, String>,
) -> Result<T, String> {
    book(state).await?;
    let mut slot = state.book.lock().await;
    let mut next = slot.as_ref().expect("book exists").clone();
    let result = apply(&mut next)?;
    next.save(&Dir).map_err(err)?;
    *slot = Some(next);
    Ok(result)
}
async fn runtime(app: &AppHandle, state: &Arc<AppState>) -> Result<Runtime, String> {
    let mut slot = state.runtime.lock().await;
    if let Some(runtime) = slot.as_ref() {
        return Ok(runtime.clone());
    }
    let secret = load_or_create_secret(&Dir).map_err(err)?;
    let relays = RelaySettings::load(&Dir).map_err(err)?;
    let endpoint = net::bind(secret, &relays).await.map_err(err)?;
    endpoint.set_alpns(vec![deliver::ALPN.to_vec()]);
    let receiving = n0_future::task::spawn(receive(app.clone(), state.clone(), endpoint.clone()));
    if let Some(old) = state.receiver.lock().await.replace(receiving) {
        old.abort();
    }
    let runtime = Runtime {
        client: ControlClient::new(endpoint.clone()),
        endpoint,
    };
    Ok(slot.insert(runtime).clone())
}
async fn view(state: &AppState) -> Result<View, String> {
    let book = book(state).await?;
    let health = state.health.lock().await;
    let delivering = state.delivering.lock().await;
    let writers = book
        .writers
        .into_iter()
        .map(|(id, writer)| WriterView {
            health: health.get(&id).cloned().unwrap_or_else(|| {
                Health::state(if writer.enabled {
                    "connecting"
                } else {
                    "paused"
                })
            }),
            delivering: delivering.contains_key(&id),
            questions: choices::list(&writer.entries),
            id,
            name: writer.name,
            ticket: writer.ticket,
            log: writer.log,
            entries: writer.entries,
            enabled: writer.enabled,
        })
        .collect();
    Ok(View {
        device_id: load_or_create_secret(&Dir)
            .map_err(err)?
            .public()
            .to_string(),
        writers,
        selected: book.selected,
        deliveries: outbox::list(&Dir).map_err(err)?,
        pairing: book.invitation.is_some(),
        pairing_code: book
            .invitation
            .as_ref()
            .map(|p| p.invitation.encode())
            .transpose()
            .map_err(err)?,
        pairing_name: book.invitation.map(|p| p.name),
        network: RelaySettings::load(&Dir).map_err(err)?,
    })
}
async fn emit_view(app: &AppHandle, state: &AppState) {
    match view(state).await {
        Ok(view) => app.emit(UiEvent::View { view }),
        Err(error) => status(app, &error),
    }
}
fn pair_state(app: &AppHandle, state: &str) {
    app.emit(UiEvent::PairState {
        state: state.into(),
    });
}
fn status(app: &AppHandle, text: &str) {
    app.emit(UiEvent::Status { text: text.into() });
}
async fn set_health(app: &AppHandle, state: &AppState, id: &str, health: Health) {
    let mut all = state.health.lock().await;
    if all.get(id) == Some(&health) {
        return;
    }
    all.insert(id.into(), health.clone());
    app.emit(UiEvent::Health {
        writer_id: id.into(),
        health,
    });
}
/// The health of a machine from its feed connection. An open delivery
/// connection shows the machine as connected, so it takes precedence.
async fn set_follow_health(app: &AppHandle, state: &AppState, id: &str, health: Health) {
    if !state.delivering.lock().await.contains_key(id) {
        set_health(app, state, id, health).await;
    }
}
async fn resume(app: AppHandle, state: Arc<AppState>) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    resume_all(app.clone(), state.clone()).await?;
    view(&state).await
}
async fn resume_all(app: AppHandle, state: Arc<AppState>) -> Result<(), String> {
    let book = book(&state).await?;
    if book.writers.values().any(|w| w.enabled)
        || book.invitation.is_some()
        || outbox::list(&Dir)
            .map_err(err)?
            .iter()
            .any(|d| d.status == "queued")
    {
        runtime(&app, &state).await?;
        start_worker(app.clone(), state.clone()).await;
    }
    for (id, writer) in book.writers.into_iter().filter(|(_, w)| w.enabled) {
        let active = state
            .tasks
            .lock()
            .await
            .get(&id)
            .is_some_and(|t| !t.is_finished());
        if !active {
            start_follow(
                app.clone(),
                state.clone(),
                writer.ticket.parse().map_err(err)?,
            )
            .await;
        }
    }
    if let Some(pair) = book.invitation {
        let active = state
            .pair_task
            .lock()
            .await
            .as_ref()
            .is_some_and(|t| !t.is_finished());
        if !active {
            start_pair(app, state, pair).await;
        }
    }
    Ok(())
}
async fn follow(
    app: AppHandle,
    state: Arc<AppState>,
    ticket: String,
    name: Option<String>,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    let ticket: EndpointTicket = ticket.trim().parse().map_err(err)?;
    update(&state, |book| book.add(&ticket, name).map_err(err)).await?;
    start_follow(app.clone(), state.clone(), ticket).await;
    start_worker(app.clone(), state.clone()).await;
    let result = view(&state).await?;
    app.emit(UiEvent::View {
        view: result.clone(),
    });
    Ok(result)
}
async fn abort(task: Option<JoinHandle<()>>) {
    if let Some(task) = task {
        task.abort();
        task.await.ok();
    }
}
async fn start_follow(app: AppHandle, state: Arc<AppState>, ticket: EndpointTicket) {
    let id = ticket.endpoint_addr().id.to_string();
    abort(state.tasks.lock().await.remove(&id)).await;
    set_follow_health(&app, &state, &id, Health::default()).await;
    let shared = state.clone();
    let key = id.clone();
    let task = n0_future::task::spawn(async move {
        if let Err(error) = run_follow(&app, &shared, ticket).await {
            set_follow_health(&app, &shared, &key, Health::failure("stopped", error, None)).await;
        }
    });
    state.tasks.lock().await.insert(id, task);
}
// The observer never reads or cancels a framed feed read.
struct Observer(JoinHandle<()>);
impl Drop for Observer {
    fn drop(&mut self) {
        self.0.abort();
    }
}
/// Save an entry from a machine with the name of its log.
async fn save_entry(
    state: &AppState,
    id: &str,
    log: Option<&str>,
    entry: Entry,
) -> Result<(), String> {
    update(state, |book| {
        book.set_log(id, log.map(String::from)).map_err(err)?;
        book.push(id, entry).map_err(err)
    })
    .await
}
async fn run_follow(
    app: &AppHandle,
    state: &Arc<AppState>,
    ticket: EndpointTicket,
) -> Result<(), String> {
    let endpoint = runtime(app, state).await?.endpoint;
    let id = ticket.endpoint_addr().id.to_string();
    loop {
        let saved = book(state)
            .await?
            .writers
            .get(&id)
            .cloned()
            .ok_or("The machine does not exist")?;
        let mut observer = None;
        let mut follower = Follower::new(
            endpoint.clone(),
            ticket.endpoint_addr().clone(),
            Request {
                since: saved.last_id(),
                log: saved.log,
                ..Default::default()
            },
        );
        loop {
            match follower.next().await {
                Ok(Some(Event::Entry(entry))) => {
                    save_entry(state, &id, follower.log(), entry).await?;
                    emit_view(app, state).await;
                }
                Ok(Some(Event::CaughtUp { .. })) => {
                    if let Some(conn) = follower.connection() {
                        set_follow_health(app, state, &id, health::snapshot(&conn)).await;
                        let (app, shared, id) = (app.clone(), state.clone(), id.clone());
                        observer = Some(Observer(n0_future::task::spawn(async move {
                            loop {
                                n0_future::time::sleep(Duration::from_secs(2)).await;
                                let health = health::snapshot(&conn);
                                let closed = conn.close_reason().is_some();
                                set_follow_health(&app, &shared, &id, health).await;
                                if closed {
                                    break;
                                }
                            }
                        })));
                    }
                }
                Ok(Some(Event::Retrying { delay, error })) => {
                    observer.take();
                    set_follow_health(
                        app,
                        state,
                        &id,
                        Health::failure("reconnecting", error, Some(delay.as_millis() as u64)),
                    )
                    .await;
                }
                Ok(None) => return Ok(()),
                Err(error) if error.downcast_ref::<Denied>().is_some() => {
                    observer.take();
                    set_follow_health(
                        app,
                        state,
                        &id,
                        Health::failure("denied", error.to_string(), Some(3000)),
                    )
                    .await;
                    n0_future::time::sleep(Duration::from_secs(3)).await;
                    break;
                }
                Err(error) => return Err(format!("{error:#}")),
            }
        }
    }
}
/// Take the deliveries that machines send to `endpoint`.
async fn receive(app: AppHandle, state: Arc<AppState>, endpoint: Endpoint) {
    let permits = Arc::new(Semaphore::new(MAX_DELIVERIES));
    while let Some(incoming) = endpoint.accept().await {
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            incoming.refuse();
            continue;
        };
        let (app, state, me) = (app.clone(), state.clone(), endpoint.id());
        n0_future::task::spawn(async move {
            let _permit = permit;
            let Ok(conn) = incoming.await else { return };
            if let Err(error) = take_delivery(&app, &state, conn, me).await {
                web_sys::console::warn_1(&format!("delivery: {error}").into());
            }
        });
    }
}
/// Save the entries of one delivery from an enabled machine, and tell the
/// machine each saved entry.
async fn take_delivery(
    app: &AppHandle,
    state: &Arc<AppState>,
    conn: Connection,
    me: EndpointId,
) -> Result<(), String> {
    let incoming = deliver::accept(conn, me).await.map_err(err)?;
    let id = incoming.writer().to_string();
    let Some(saved) = book(state)
        .await?
        .writers
        .get(&id)
        .filter(|w| w.enabled)
        .cloned()
    else {
        incoming.deny();
        return Ok(());
    };
    let req = Request {
        since: saved.last_id(),
        log: saved.log,
        ..Default::default()
    };
    let mut sub = incoming.subscribe(&req).await.map_err(err)?;
    let conn = sub.connection().clone();
    state
        .delivering
        .lock()
        .await
        .insert(id.clone(), conn.clone());
    set_health(app, state, &id, health::snapshot(&conn)).await;
    emit_view(app, state).await;
    // Queued answers for this machine can go on this connection now.
    state.wake.notify_one();
    let result = async {
        loop {
            match sub.next().await.map_err(err)? {
                Some(Frame::Entry { entry }) => {
                    let saved = entry.id;
                    save_entry(state, &id, sub.log(), entry).await?;
                    emit_view(app, state).await;
                    sub.stored(saved).await.map_err(err)?;
                }
                Some(Frame::CaughtUp { head }) => sub.stored(head).await.map_err(err)?,
                Some(_) => {}
                None => return Ok(()),
            }
        }
    }
    .await;
    let mut delivering = state.delivering.lock().await;
    if delivering
        .get(&id)
        .is_some_and(|c| c.stable_id() == conn.stable_id())
    {
        delivering.remove(&id);
    }
    drop(delivering);
    state.health.lock().await.remove(&id);
    emit_view(app, state).await;
    // The connection ends when the machine finishes its delivery.
    if conn.close_reason().is_some() {
        return Ok(());
    }
    result
}
async fn select_writer(state: Arc<AppState>, writer_id: Option<String>) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    update(&state, |book| {
        book.selected = writer_id;
        Ok(())
    })
    .await?;
    view(&state).await
}
async fn set_writer_enabled(
    app: AppHandle,
    state: Arc<AppState>,
    writer_id: String,
    enabled: bool,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    let ticket = update(&state, |book| {
        let writer = book
            .writers
            .get_mut(&writer_id)
            .ok_or("The machine does not exist")?;
        writer.enabled = enabled;
        Ok(writer.ticket.clone())
    })
    .await?;
    if enabled {
        start_follow(app.clone(), state.clone(), ticket.parse().map_err(err)?).await;
        start_worker(app.clone(), state.clone()).await;
        state.wake.notify_one();
    } else {
        abort(state.tasks.lock().await.remove(&writer_id)).await;
        if let Some(conn) = state.delivering.lock().await.remove(&writer_id) {
            conn.close(yappr::proto::DENIED_CODE.into(), b"paused");
        }
        set_health(&app, &state, &writer_id, Health::state("paused")).await;
    }
    view(&state).await
}
async fn rename_writer(
    state: Arc<AppState>,
    writer_id: String,
    name: String,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    readers::validate_name(&name).map_err(err)?;
    update(&state, |book| {
        book.writers
            .get_mut(&writer_id)
            .ok_or("The machine does not exist")?
            .name = name;
        Ok(())
    })
    .await?;
    view(&state).await
}
async fn choose(
    app: AppHandle,
    state: Arc<AppState>,
    writer_id: String,
    question_id: String,
    value: String,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    let book = book(&state).await?;
    let writer = book
        .writers
        .get(&writer_id)
        .ok_or("The machine does not exist")?;
    if !writer.enabled {
        return Err("Resume this machine before answering.".into());
    }
    let question = choices::get(&writer.entries, &question_id).map_err(err)?;
    if question
        .question
        .expires_at
        .is_some_and(|expires| expires <= jiff::Timestamp::now())
        && question.status == "pending"
    {
        return Err("The question expired.".into());
    }
    if question.status != "pending" {
        return Err(format!("The question is {}.", question.status));
    }
    let local = load_or_create_secret(&Dir).map_err(err)?.public();
    if question.question.responder != local {
        return Err("This question is for another device.".into());
    }
    if !question.question.options.iter().any(|o| o.value == value) {
        return Err("The option does not exist.".into());
    }
    let ticket: EndpointTicket = writer.ticket.parse().map_err(err)?;
    outbox::queue(&Dir, &ticket, question_id, value).map_err(err)?;
    start_worker(app.clone(), state.clone()).await;
    state.wake.notify_one();
    view(&state).await
}
/// Send the queued answers again and again, one task for each machine.
async fn start_worker(app: AppHandle, state: Arc<AppState>) {
    let mut slot = state.worker.lock().await;
    if slot.as_ref().is_some_and(|t| !t.is_finished()) {
        return;
    }
    let shared = state.clone();
    *slot = Some(n0_future::task::spawn(async move {
        loop {
            if let (Ok(runtime), Ok(readers), Ok(deliveries)) = (
                runtime(&app, &shared).await,
                book(&shared).await,
                outbox::list(&Dir),
            ) {
                let mut by_writer: BTreeMap<String, Vec<Delivery>> = BTreeMap::new();
                for mut delivery in deliveries.into_iter().filter(|d| d.status == "queued") {
                    let Some(writer) = readers.writers.get(&delivery.writer) else {
                        continue;
                    };
                    if !writer.enabled {
                        continue;
                    }
                    delivery.ticket = writer.ticket.clone();
                    by_writer
                        .entry(delivery.writer.clone())
                        .or_default()
                        .push(delivery);
                }
                let mut tasks = n0_future::task::JoinSet::new();
                for (writer, deliveries) in by_writer {
                    let (client, app, shared) =
                        (runtime.client.clone(), app.clone(), shared.clone());
                    tasks.spawn(async move {
                        for delivery in deliveries {
                            // Check the pause state again before each answer.
                            let enabled = book(&shared)
                                .await
                                .ok()
                                .and_then(|b| b.writers.get(&writer).map(|w| w.enabled))
                                .unwrap_or(false);
                            if !enabled {
                                break;
                            }
                            let delivering = shared.delivering.lock().await.get(&writer).cloned();
                            outbox::deliver_with(&Dir, &client, &delivery, delivering.as_ref())
                                .await
                                .ok();
                            emit_view(&app, &shared).await;
                        }
                    });
                }
                while tasks.join_next().await.is_some() {}
            }
            tokio::select! {
                _ = n0_future::time::sleep(Duration::from_secs(3)) => {},
                _ = shared.wake.notified() => {},
            }
        }
    }));
}

#[derive(Serialize)]
struct PairPreview {
    name: String,
    writer_id: String,
    expires_at: String,
}
fn preview_invitation(invitation: String) -> Result<PairPreview, String> {
    let invitation = Invitation::decode(&invitation).map_err(err)?;
    let ticket: EndpointTicket = invitation.ticket.parse().map_err(err)?;
    Ok(PairPreview {
        name: invitation.name,
        writer_id: ticket.endpoint_addr().id.to_string(),
        expires_at: invitation.expires_at.to_string(),
    })
}
async fn pair_device(
    app: AppHandle,
    state: Arc<AppState>,
    invitation: String,
    name: String,
) -> Result<(), String> {
    let _guard = state.lifecycle.lock().await;
    let invitation = Invitation::decode(&invitation).map_err(err)?;
    readers::validate_name(&name).map_err(err)?;
    let pair = PendingPair { invitation, name };
    abort(state.pair_task.lock().await.take()).await;
    update(&state, |book| {
        book.invitation = Some(pair.clone());
        Ok(())
    })
    .await?;
    start_pair(app, state.clone(), pair).await;
    Ok(())
}
/// The address of this page, after the endpoint has a relay. The writer
/// delivers its feed to this address.
async fn own_ticket(endpoint: &Endpoint) -> Result<EndpointTicket, String> {
    n0_future::time::timeout(Duration::from_secs(15), endpoint.online())
        .await
        .map_err(|_| "The relay did not connect. Check your connection.".to_string())?;
    Ok(EndpointTicket::new(net::relay_addr(endpoint.addr())))
}
async fn start_pair(app: AppHandle, state: Arc<AppState>, pair: PendingPair) {
    let shared = state.clone();
    let task = n0_future::task::spawn(async move {
        let result = async {
            let runtime = runtime(&app, &shared).await?;
            let invitation = pair.invitation;
            let ticket: EndpointTicket = invitation.ticket.parse().map_err(err)?;
            pair_state(&app, "sending");
            let own = own_ticket(&runtime.endpoint).await?;
            // The writer has the request after its first reply.
            let mut delivered = false;
            loop {
                // The writer answers a claimed invitation for a short time after
                // its deadline, so keep asking until that window ends.
                let last_ask = invitation
                    .expires_at
                    .checked_add(pairing::RESULT_WINDOW)
                    .unwrap_or(invitation.expires_at);
                if last_ask <= jiff::Timestamp::now() {
                    return Err("The invitation expired. Create a new code on the computer.".into());
                }
                let reply: Result<pairing::PairReply, _> = runtime
                    .client
                    .exchange(
                        ticket.endpoint_addr().clone(),
                        pairing::ALPN,
                        &pairing::Join {
                            token: invitation.token.clone(),
                            name: pair.name.clone(),
                            ticket: Some(own.to_string()),
                        },
                    )
                    .await;
                match reply {
                    Ok(reply) if reply.status == "accepted" => {
                        let _guard = shared.lifecycle.lock().await;
                        update(&shared, |book| {
                            book.add(&ticket, Some(invitation.name.clone()))
                                .map_err(err)?;
                            book.invitation = None;
                            Ok(())
                        })
                        .await?;
                        emit_view(&app, &shared).await;
                        start_follow(app.clone(), shared.clone(), ticket).await;
                        start_worker(app.clone(), shared.clone()).await;
                        return Ok(());
                    }
                    Ok(reply) if reply.status == "rejected" => return Err(reply.message),
                    Err(error) => {
                        let text = format!("Pairing request failed: {error:#}");
                        web_sys::console::warn_1(&text.as_str().into());
                        if !delivered {
                            pair_state(&app, "retrying");
                        }
                    }
                    Ok(_) => {
                        if !delivered {
                            delivered = true;
                            pair_state(&app, "waiting");
                        }
                    }
                }
                n0_future::time::sleep(Duration::from_secs(1)).await;
            }
        }
        .await;
        if let Err(message) = result {
            update(&shared, |book| {
                book.invitation = None;
                Ok(())
            })
            .await
            .ok();
            emit_view(&app, &shared).await;
            app.emit(UiEvent::PairFailed { message });
        }
    });
    *state.pair_task.lock().await = Some(task);
}
async fn stop(app: AppHandle, state: Arc<AppState>) -> Result<(), String> {
    let _guard = state.lifecycle.lock().await;
    abort(state.pair_task.lock().await.take()).await;
    update(&state, |book| {
        book.invitation = None;
        Ok(())
    })
    .await?;
    emit_view(&app, &state).await;
    Ok(())
}
async fn configure_network(
    app: AppHandle,
    state: Arc<AppState>,
    settings: RelaySettings,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    settings.validate().map_err(err)?;
    if book(&state).await?.invitation.is_some() {
        return Err("Finish or cancel pairing before changing relays.".into());
    }
    settings.save(&Dir).map_err(err)?;
    let tasks = std::mem::take(&mut *state.tasks.lock().await);
    for task in tasks.into_values() {
        abort(Some(task)).await;
    }
    abort(state.worker.lock().await.take()).await;
    abort(state.receiver.lock().await.take()).await;
    state.delivering.lock().await.clear();
    if let Some(runtime) = state.runtime.lock().await.take() {
        runtime.client.close().await;
        runtime.endpoint.close().await;
    }
    state.health.lock().await.clear();
    resume_all(app.clone(), state.clone()).await?;
    view(&state).await
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FollowArgs {
    ticket: String,
    name: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectArgs {
    writer_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnableArgs {
    writer_id: String,
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameArgs {
    writer_id: String,
    name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChooseArgs {
    writer_id: String,
    question_id: String,
    value: String,
}
#[derive(Deserialize)]
struct InvitationArgs {
    invitation: String,
}
#[derive(Deserialize)]
struct PairArgs {
    invitation: String,
    name: String,
}
#[derive(Deserialize)]
struct NetworkArgs {
    settings: RelaySettings,
}

fn args<T: DeserializeOwned>(value: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(err)
}
fn json<T: Serialize>(result: Result<T, String>) -> Result<serde_json::Value, String> {
    result.and_then(|value| serde_json::to_value(value).map_err(err))
}

/// Run one page command.
async fn call(
    app: AppHandle,
    state: Arc<AppState>,
    command: &str,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    match command {
        "snapshot" => json(view(&state).await),
        "resume" => json(resume(app, state).await),
        "preview_feed" => {
            let a: FollowArgs = args(value)?;
            let ticket: EndpointTicket = a.ticket.parse().map_err(err)?;
            Ok(serde_json::json!({"writer_id":ticket.endpoint_addr().id.to_string()}))
        }
        "follow" => {
            let a: FollowArgs = args(value)?;
            json(follow(app, state, a.ticket, a.name).await)
        }
        "select_writer" => {
            let a: SelectArgs = args(value)?;
            json(select_writer(state, a.writer_id).await)
        }
        "set_writer_enabled" => {
            let a: EnableArgs = args(value)?;
            json(set_writer_enabled(app, state, a.writer_id, a.enabled).await)
        }
        "rename_writer" => {
            let a: RenameArgs = args(value)?;
            json(rename_writer(state, a.writer_id, a.name).await)
        }
        "choose" => {
            let a: ChooseArgs = args(value)?;
            json(choose(app, state, a.writer_id, a.question_id, a.value).await)
        }
        "preview_invitation" => {
            let a: InvitationArgs = args(value)?;
            json(preview_invitation(a.invitation))
        }
        "pair_device" => {
            let a: PairArgs = args(value)?;
            json(pair_device(app, state, a.invitation, a.name).await)
        }
        "stop" => json(stop(app, state).await),
        "configure_network" => {
            let a: NetworkArgs = args(value)?;
            json(configure_network(app, state, a.settings).await)
        }
        _ => Err(format!("unknown command {command:?}")),
    }
}

/// The reader of this page. The page creates one and keeps it.
#[wasm_bindgen]
pub struct App {
    app: AppHandle,
    state: Arc<AppState>,
}

#[wasm_bindgen]
impl App {
    /// `on_event` receives each event as a JSON string.
    #[wasm_bindgen(constructor)]
    pub fn new(on_event: js_sys::Function) -> App {
        console_error_panic_hook::set_once();
        App {
            app: AppHandle { on_event },
            state: Arc::new(AppState::default()),
        }
    }

    /// Run an app command. `args` and the result are JSON strings. A failed
    /// command rejects with its message.
    pub fn invoke(&self, command: String, args: String) -> js_sys::Promise {
        let (app, state) = (self.app.clone(), self.state.clone());
        wasm_bindgen_futures::future_to_promise(async move {
            let value = if args.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&args).map_err(|e| JsValue::from_str(&e.to_string()))?
            };
            let result = call(app, state, &command, value)
                .await
                .map_err(|e| JsValue::from_str(&e))?;
            Ok(JsValue::from_str(&result.to_string()))
        })
    }
}
