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
    request_details(&mut state, &worker);
    ratatui::run(|terminal| event_loop(terminal, &mut state, &worker))?;
    Ok(true)
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut BrowserState,
    worker: &Worker,
) -> io::Result<()> {
    let mut dirty = true;
    loop {
        while let Some(event) = worker.poll() {
            dirty = true;
            match event {
                worker::Event::Snapshot(snapshot) => state.replace_snapshot(*snapshot),
                worker::Event::Comparison { name, report } => {
                    state.comparison_finished(name, *report);
                }
                worker::Event::FunctionDetail { identity, detail } => {
                    state.function_detail_finished(identity, detail.map(|detail| *detail));
                }
                worker::Event::RegisterDetail { address, detail } => {
                    state.register_detail_finished(address, detail.map(|detail| *detail));
                }
                worker::Event::Error(message) => state.operation_failed(message),
            }
        }
        request_details(state, worker);
        if dirty {
            terminal.draw(|frame| super::view::render(frame, state))?;
            dirty = false;
        }
        if event::poll(Duration::from_millis(75))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    dirty = true;
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
                    request_details(state, worker);
                }
                Event::Resize(_, _) => {
                    dirty = true;
                }
                _ => {}
            }
        }
    }
}

fn request_details(state: &mut BrowserState, worker: &Worker) {
    if let Some(identity) = state.request_function_detail()
        && let Err(error) = worker.function_detail(identity)
    {
        state.operation_failed(error.to_string());
    }
    if let Some(address) = state.request_register_detail()
        && let Err(error) = worker.register_detail(address)
    {
        state.operation_failed(error.to_string());
    }
}

fn handle_key(state: &mut BrowserState, key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }
    if state.search_editing {
        return match key.code {
            KeyCode::Enter => {
                state.finish_search();
                Action::Continue
            }
            KeyCode::Esc => {
                state.clear_search();
                Action::Continue
            }
            KeyCode::Backspace => {
                state.pop_search();
                Action::Continue
            }
            KeyCode::Char(character) => {
                state.push_search(character);
                Action::Continue
            }
            _ => Action::Continue,
        };
    }
    match key.code {
        KeyCode::Esc if !state.search_query.is_empty() => {
            state.clear_search();
            Action::Continue
        }
        KeyCode::Esc | KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('/') => {
            state.begin_search();
            Action::Continue
        }
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
        KeyCode::PageDown | KeyCode::Char('d') => {
            state.scroll_detail_down(8);
            Action::Continue
        }
        KeyCode::PageUp | KeyCode::Char('u') => {
            state.scroll_detail_up(8);
            Action::Continue
        }
        KeyCode::Char('r') => state.begin_reload(),
        KeyCode::Enter => state.activate(),
        KeyCode::Char('c') => state.begin_compare(),
        _ => Action::Continue,
    }
}
