//! Background ownership for reload operations.

use std::{
    io,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use crate::{BlobrayApplication, FunctionDetailSummary, RegisterDetailSummary, WorkspaceSnapshot};

enum Command {
    Reload,
    Compare(String),
    FunctionDetail(String),
    RegisterDetail(u32),
    Shutdown,
}

pub(super) enum Event {
    Snapshot(Box<WorkspaceSnapshot>),
    Comparison {
        name: String,
        report: Box<crate::ExecutionComparisonReport>,
    },
    FunctionDetail {
        identity: String,
        detail: Option<Box<FunctionDetailSummary>>,
    },
    RegisterDetail {
        address: u32,
        detail: Option<Box<RegisterDetailSummary>>,
    },
    Error(String),
}

pub(super) struct Worker {
    commands: Sender<Command>,
    events: Receiver<Event>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Worker {
    pub(super) fn start(mut application: BlobrayApplication) -> io::Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("blobray-tui".to_owned())
            .spawn(move || {
                while let Ok(command) = command_rx.recv() {
                    match command {
                        Command::Reload => {
                            let event = match application.reload() {
                                Ok(snapshot) => Event::Snapshot(Box::new(snapshot)),
                                Err(error) => Event::Error(error.to_string()),
                            };
                            if event_tx.send(event).is_err() {
                                break;
                            }
                        }
                        Command::Compare(name) => {
                            let event = match application.compare_profile(&name) {
                                Ok(report) => Event::Comparison {
                                    name,
                                    report: Box::new(report),
                                },
                                Err(error) => Event::Error(error.to_string()),
                            };
                            if event_tx.send(event).is_err() {
                                break;
                            }
                        }
                        Command::FunctionDetail(identity) => {
                            let event = match application.function_detail(&identity) {
                                Ok(detail) => Event::FunctionDetail {
                                    identity,
                                    detail: detail.map(Box::new),
                                },
                                Err(error) => Event::Error(error.to_string()),
                            };
                            if event_tx.send(event).is_err() {
                                break;
                            }
                        }
                        Command::RegisterDetail(address) => {
                            let event = match application.register_detail(address) {
                                Ok(detail) => Event::RegisterDetail {
                                    address,
                                    detail: detail.map(Box::new),
                                },
                                Err(error) => Event::Error(error.to_string()),
                            };
                            if event_tx.send(event).is_err() {
                                break;
                            }
                        }
                        Command::Shutdown => break,
                    }
                }
            })?;
        Ok(Self {
            commands: command_tx,
            events: event_rx,
            thread: Some(thread),
        })
    }

    pub(super) fn reload(&self) -> io::Result<()> {
        self.commands
            .send(Command::Reload)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "TUI worker stopped"))
    }

    pub(super) fn compare(&self, name: String) -> io::Result<()> {
        self.commands
            .send(Command::Compare(name))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "TUI worker stopped"))
    }

    pub(super) fn function_detail(&self, identity: String) -> io::Result<()> {
        self.commands
            .send(Command::FunctionDetail(identity))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "TUI worker stopped"))
    }

    pub(super) fn register_detail(&self, address: u32) -> io::Result<()> {
        self.commands
            .send(Command::RegisterDetail(address))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "TUI worker stopped"))
    }

    pub(super) fn poll(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        // Dropping a JoinHandle detaches the worker. A comparison may be in a
        // long-running backend operation that cannot be cancelled safely; TUI
        // teardown must never wait for it while the terminal is being restored.
        let _ = self.thread.take();
    }
}
