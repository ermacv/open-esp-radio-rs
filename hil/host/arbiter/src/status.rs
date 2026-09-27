//! A snapshot of the stand for `cargo hil queue`.

use std::{fmt::Write as _, time::Duration};

use serde::Serialize;

use crate::{
    Arbiter, BoardEvent, BudgetSource, LeaseRecord,
    board::{device_label, latest},
    budget::format_duration,
    queue,
};

const RECENT_LEASES: usize = 5;

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub holder: Option<HolderStatus>,
    /// In expected grant order.
    pub queue: Vec<QueuedStatus>,
    /// Registered, attached or journaled boards.
    pub devices: Vec<DeviceStatus>,
    /// The newest event of every startup-artifact host file.
    pub startup_artifacts: Vec<BoardEvent>,
    pub recent: Vec<LeaseRecord>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceStatus {
    /// `None` for journal records that predate device identities.
    pub mac: Option<String>,
    pub label: String,
    /// The port it is attached at now.
    pub port: Option<String>,
    pub firmware: Option<BoardEvent>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HolderStatus {
    pub id: u64,
    pub owner: String,
    pub work: String,
    pub pid: u32,
    pub elapsed_secs: u64,
    pub budget_secs: u64,
    pub budget_source: BudgetSource,
    pub over_budget: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct QueuedStatus {
    pub id: u64,
    pub owner: String,
    pub work: String,
    pub pid: u32,
    pub short: bool,
    pub waiting_secs: u64,
    pub budget_secs: u64,
    pub budget_source: BudgetSource,
    pub expected_start_secs: u64,
}

impl Arbiter {
    pub fn status(&self) -> crate::Result<Status> {
        let now = crate::unix_now();
        let state = self.transaction(|state| Ok(state.clone()))?;
        let starts = queue::expected_starts(&state, now);
        let queue = starts
            .iter()
            .filter_map(|(id, start)| {
                let ticket = state.queue.iter().find(|ticket| ticket.id == *id)?;
                Some(QueuedStatus {
                    id: ticket.id,
                    owner: ticket.owner.clone(),
                    work: ticket.work.clone(),
                    pid: ticket.process.pid,
                    short: ticket.short,
                    waiting_secs: now.saturating_sub(ticket.enqueued_unix),
                    budget_secs: ticket.budget_secs,
                    budget_source: ticket.budget_source,
                    expected_start_secs: *start,
                })
            })
            .collect();
        let events = self.board_events()?;
        let (flashes, startup_artifacts) = latest(&events);
        let registered = self.devices()?;
        let attached = crate::attached_ports();
        let mut macs: Vec<Option<String>> = registered
            .iter()
            .map(|device| Some(device.mac.clone()))
            .chain(
                attached
                    .iter()
                    .filter_map(|port| port.mac.clone().map(Some)),
            )
            .chain(flashes.iter().map(|flash| flash.device.clone()))
            .collect();
        let mut seen = Vec::new();
        macs.retain(|mac| {
            let new = !seen.contains(mac);
            seen.push(mac.clone());
            new
        });
        let devices = macs
            .into_iter()
            .map(|mac| DeviceStatus {
                label: device_label(mac.as_deref(), &registered),
                port: attached
                    .iter()
                    .find(|port| port.mac.is_some() && port.mac == mac)
                    .map(|port| port.port.clone()),
                firmware: flashes
                    .iter()
                    .find(|flash| flash.device == mac)
                    .map(|flash| (*flash).clone()),
                mac,
            })
            .collect();
        let history = self.history()?;
        Ok(Status {
            holder: state.holder.map(|holder| HolderStatus {
                id: holder.ticket.id,
                owner: holder.ticket.owner,
                work: holder.ticket.work,
                pid: holder.ticket.process.pid,
                elapsed_secs: now.saturating_sub(holder.granted_unix),
                budget_secs: holder.ticket.budget_secs,
                budget_source: holder.ticket.budget_source,
                over_budget: holder.over_budget,
            }),
            queue,
            devices,
            startup_artifacts: startup_artifacts.into_iter().cloned().collect(),
            recent: history.iter().rev().take(RECENT_LEASES).cloned().collect(),
        })
    }
}

fn duration(seconds: u64) -> String {
    format_duration(Duration::from_secs(seconds))
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut text = String::new();
        match &self.holder {
            Some(holder) => writeln!(
                text,
                "held:    #{} {} `{}` pid {}, {} of budget {}{}",
                holder.id,
                holder.owner,
                holder.work,
                holder.pid,
                duration(holder.elapsed_secs),
                duration(holder.budget_secs),
                if holder.over_budget {
                    " (over budget)"
                } else {
                    ""
                }
            ),
            None => writeln!(text, "held:    free"),
        }?;
        if self.queue.is_empty() {
            writeln!(text, "queue:   empty")?;
        }
        for (position, entry) in self.queue.iter().enumerate() {
            writeln!(
                text,
                "queue {}: #{} {} `{}`{} pid {}, waiting {}, budget {} ({}), start in ~{}",
                position + 1,
                entry.id,
                entry.owner,
                entry.work,
                if entry.short { " [short]" } else { "" },
                entry.pid,
                duration(entry.waiting_secs),
                duration(entry.budget_secs),
                entry.budget_source,
                duration(entry.expected_start_secs)
            )?;
        }
        let describe = |event: &Option<BoardEvent>| {
            event
                .as_ref()
                .map_or_else(|| String::from("unknown"), ToString::to_string)
        };
        if self.devices.is_empty() {
            writeln!(text, "boards:  none known")?;
        }
        for device in &self.devices {
            writeln!(
                text,
                "board {} [{}]: {}",
                device.label,
                device.port.as_deref().unwrap_or("not attached"),
                describe(&device.firmware)
            )?;
        }
        for artifact in &self.startup_artifacts {
            writeln!(text, "startup artifact (host file): {artifact}")?;
        }
        if !self.recent.is_empty() {
            writeln!(text, "recent leases:")?;
        }
        for record in &self.recent {
            writeln!(
                text,
                "  #{} {} `{}` {} of {} {:?}",
                record.id,
                record.owner,
                record.work,
                duration(record.duration_secs()),
                duration(record.budget_secs),
                record.outcome
            )?;
        }
        f.write_str(text.trim_end())
    }
}
