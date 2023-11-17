use std::{
    net::{Shutdown, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use websocket::{
    server::upgrade::WsUpgrade,
    sync::{server::upgrade::Buffer, Server, Writer},
    OwnedMessage,
};

use crate::{
    store::SpanId,
    store_container::StoreContainer,
    viewer::{ExpandedState, ViewLineUpdate, Viewer},
};

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
pub enum ServerToClientMessage {
    ViewLine {
        #[serde(flatten)]
        update: ViewLineUpdate,
    },
    ViewLinesCount {
        count: usize,
    },
    #[serde(rename_all = "camelCase")]
    QueryResult {
        id: SpanId,
        is_graph: bool,
        start: u64,
        args: Vec<(String, String)>,
        path: Vec<String>,
    },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
pub enum ClientToServerMessage {
    #[serde(rename_all = "camelCase")]
    ViewRect {
        view_rect: ViewRect,
    },
    Expand {
        id: SpanId,
    },
    ExpandAll {
        id: SpanId,
    },
    Collapse {
        id: SpanId,
    },
    ResetExpand {
        id: SpanId,
    },
    Query {
        id: SpanId,
    },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpanViewEvent {
    pub start: u64,
    pub duration: u64,
    pub name: String,
    pub id: Option<SpanId>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ViewRect {
    pub x: u64,
    pub y: u64,
    pub width: u64,
    pub height: u64,
    pub horizontal_pixels: u64,
}

struct ConnectionState {
    writer: Writer<TcpStream>,
    store: Arc<StoreContainer>,
    viewer: Viewer,
    view_rect: ViewRect,
    last_update_generation: usize,
}

pub fn serve(store: Arc<StoreContainer>) -> Result<()> {
    let mut server = Server::bind("127.0.0.1:57475")?;
    loop {
        let Ok(connection) = server.accept() else {
            continue;
        };
        let store = store.clone();
        thread::spawn(move || {
            fn handle_connection(
                connection: WsUpgrade<TcpStream, Option<Buffer>>,
                store: Arc<StoreContainer>,
            ) -> Result<()> {
                let connection = match connection.accept() {
                    Ok(connection) => connection,
                    Err((connection, error)) => {
                        connection.shutdown(Shutdown::Both)?;
                        return Err(error.into());
                    }
                };
                println!("client connected");
                let (mut reader, writer) = connection.split()?;
                let state = Arc::new(Mutex::new(ConnectionState {
                    writer,
                    store,
                    viewer: Viewer::new(),
                    view_rect: ViewRect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                        horizontal_pixels: 1,
                    },
                    last_update_generation: 0,
                }));
                let should_shutdown = Arc::new(AtomicBool::new(false));
                fn send_update(state: &mut ConnectionState, force_send: bool) -> Result<()> {
                    let store = state.store.read();
                    if !force_send && state.last_update_generation == store.generation() {
                        return Ok(());
                    }
                    state.last_update_generation = store.generation();
                    let updates = state.viewer.compute_update(&*store, &state.view_rect);
                    let count = updates.len();
                    for update in updates {
                        let message = ServerToClientMessage::ViewLine { update };
                        let message = serde_json::to_string(&message).unwrap();
                        state.writer.send_message(&OwnedMessage::Text(message))?;
                    }
                    let message = ServerToClientMessage::ViewLinesCount { count };
                    let message = serde_json::to_string(&message).unwrap();
                    state.writer.send_message(&OwnedMessage::Text(message))?;
                    Ok(())
                }
                let inner_thread = {
                    let should_shutdown = should_shutdown.clone();
                    let state = state.clone();
                    thread::spawn(move || loop {
                        if should_shutdown.load(Ordering::SeqCst) {
                            return;
                        }
                        if send_update(&mut *state.lock().unwrap(), false).is_err() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(500));
                    })
                };
                loop {
                    match reader.recv_message()? {
                        OwnedMessage::Text(text) => {
                            let message: ClientToServerMessage = serde_json::from_str(&text)?;
                            let mut state = state.lock().unwrap();
                            match message {
                                ClientToServerMessage::ViewRect { view_rect } => {
                                    state.view_rect = view_rect;
                                }
                                ClientToServerMessage::Expand { id } => {
                                    state
                                        .viewer
                                        .set_expanded_state(id, Some(ExpandedState::Expanded));
                                }
                                ClientToServerMessage::ExpandAll { id } => {
                                    state
                                        .viewer
                                        .set_expanded_state(id, Some(ExpandedState::AllExpanded));
                                }
                                ClientToServerMessage::Collapse { id } => {
                                    state
                                        .viewer
                                        .set_expanded_state(id, Some(ExpandedState::Collapsed));
                                }
                                ClientToServerMessage::ResetExpand { id } => {
                                    state.viewer.set_expanded_state(id, None);
                                }
                                ClientToServerMessage::Query { id } => {
                                    let message = if let Some((span, is_graph)) =
                                        state.store.read().span(id)
                                    {
                                        let span_start = span.start();
                                        let args = span
                                            .args()
                                            .map(|(k, v)| (k.to_string(), v.to_string()))
                                            .collect();
                                        let mut path = Vec::new();
                                        let mut current = span;
                                        while let Some(parent) = current.parent() {
                                            path.push(parent.nice_name().1.to_string());
                                            current = parent;
                                        }
                                        path.reverse();
                                        ServerToClientMessage::QueryResult {
                                            id,
                                            is_graph,
                                            start: span_start,
                                            args,
                                            path,
                                        }
                                    } else {
                                        ServerToClientMessage::QueryResult {
                                            id,
                                            is_graph: false,
                                            start: 0,
                                            args: Vec::new(),
                                            path: Vec::new(),
                                        }
                                    };
                                    let message = serde_json::to_string(&message).unwrap();
                                    state.writer.send_message(&OwnedMessage::Text(message))?;
                                    continue;
                                }
                            }
                            send_update(&mut *state, true)?;
                        }
                        OwnedMessage::Binary(_) => {
                            // This doesn't happen
                        }
                        OwnedMessage::Close(_) => {
                            reader.shutdown_all()?;
                            should_shutdown.store(true, Ordering::SeqCst);
                            inner_thread.join().unwrap();
                            return Ok(());
                        }
                        OwnedMessage::Ping(d) => {
                            state
                                .lock()
                                .unwrap()
                                .writer
                                .send_message(&OwnedMessage::Pong(d))?;
                        }
                        OwnedMessage::Pong(_) => {
                            // thanks for the fish
                        }
                    }
                }
            }
            if let Err(err) = handle_connection(connection, store) {
                eprintln!("Error: {:?}", err);
            }
        });
    }
}
