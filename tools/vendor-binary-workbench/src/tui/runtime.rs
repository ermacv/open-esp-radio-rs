//! Terminal lifecycle and input loop.

use std::{
    io::{self, IsTerminal},
    path::Path,
    time::Duration,
};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{
    state::{Action, BrowserState},
    worker::{self, Worker},
};
use crate::{Result, WorkbenchApplication};

pub(crate) fn run(manifest: &Path) -> Result<bool> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return Err(crate::Error::invalid(
            "project browse requires an interactive stdin and stdout terminal",
        ));
    }
    let mut application =
        WorkbenchApplication::open(manifest).map_err(|error| error.into_inner())?;
    let snapshot = application.snapshot().map_err(|error| error.into_inner())?;
    let mut state = BrowserState::new(snapshot);
    let worker = Worker::start(application)?;
    ratatui::run(|terminal| event_loop(terminal, &mut state, &worker))?;
    Ok(true)
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut BrowserState,
    worker: &Worker,
) -> io::Result<()> {
    loop {
        while let Some(event) = worker.poll() {
            match event {
                worker::Event::Snapshot(snapshot) => state.replace_snapshot(*snapshot),
                worker::Event::Comparison { name, report } => {
                    state.comparison_finished(name, *report);
                }
                worker::Event::Error(message) => state.operation_failed(message),
            }
        }
        terminal.draw(|frame| super::view::render(frame, state))?;
        if event::poll(Duration::from_millis(75))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match handle_key(state, key) {
                Action::Continue => {}
                Action::Reload => {
                    if let Err(error) = worker.reload() {
                        state.operation_failed(error.to_string());
                    }
                }
                Action::Compare(name) => {
                    if let Err(error) = worker.compare(name) {
                        state.operation_failed(error.to_string());
                    }
                }
                Action::Quit => return Ok(()),
            }
        }
    }
}

fn handle_key(state: &mut BrowserState, key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Action::Quit,
        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
            state.select_next_section();
            Action::Continue
        }
        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
            state.select_previous_section();
            Action::Continue
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.select_next();
            Action::Continue
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.select_previous();
            Action::Continue
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.select_first();
            Action::Continue
        }
        KeyCode::End | KeyCode::Char('G') => {
            state.select_last();
            Action::Continue
        }
        KeyCode::Char('r') => state.begin_reload(),
        KeyCode::Enter | KeyCode::Char('c') => state.begin_compare(),
        _ => Action::Continue,
    }
}
