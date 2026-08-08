//! Background ownership for reload operations.

use std::{
    io,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use crate::{WorkbenchApplication, WorkspaceSnapshot};

enum Command {
    Reload,
    Compare(String),
    Shutdown,
}

pub(super) enum Event {
    Snapshot(Box<WorkspaceSnapshot>),
    Comparison {
        name: String,
        report: Box<crate::ExecutionComparisonReport>,
    },
    Error(String),
}

pub(super) struct Worker {
    commands: Sender<Command>,
    events: Receiver<Event>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Worker {
    pub(super) fn start(mut application: WorkbenchApplication) -> io::Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("vendor-workbench-tui".to_owned())
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

    pub(super) fn poll(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
