//! The machines that this page reads, each with its own saved history.
use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use yappr::{entry::Entry, pairing::Invitation, EndpointTicket};

fn enabled() -> bool {
    true
}

/// The most machines that one page reads.
const MAX_WRITERS: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reader {
    pub ticket: String,
    pub name: String,
    pub entries: Vec<Entry>,
    /// The name of the machine's log, see `yappr::store`. Entries from
    /// before log names have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    /// A paused machine is not read, and its deliveries are refused.
    #[serde(default = "enabled")]
    pub enabled: bool,
}

impl Reader {
    /// The id of the last saved entry, or 0.
    pub fn last_id(&self) -> u64 {
        self.entries.last().map_or(0, |e| e.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPair {
    pub invitation: Invitation,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Readers {
    pub writers: BTreeMap<String, Reader>,
    pub selected: Option<String>,
    pub invitation: Option<PendingPair>,
}

impl Readers {
    pub fn validate(&self) -> Result<()> {
        if self.writers.len() > MAX_WRITERS {
            bail!("read at most {MAX_WRITERS} machines");
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !self.writers.contains_key(id))
        {
            bail!("the selected machine does not exist");
        }
        for (id, writer) in &self.writers {
            let ticket: EndpointTicket = writer.ticket.parse()?;
            if ticket.endpoint_addr().id.to_string() != *id {
                bail!("the machine ticket belongs to another identity");
            }
            let mut last = 0;
            for entry in &writer.entries {
                entry.validate()?;
                if entry.id <= last {
                    bail!("the saved feed is not in entry order");
                }
                last = entry.id;
            }
            validate_name(&writer.name)?;
        }
        Ok(())
    }

    /// Add a machine, or update its address without a change to its saved
    /// history.
    pub fn add(&mut self, ticket: &EndpointTicket, name: Option<String>) -> Result<String> {
        let id = ticket.endpoint_addr().id.to_string();
        if !self.writers.contains_key(&id) && self.writers.len() >= MAX_WRITERS {
            bail!("read at most {MAX_WRITERS} machines");
        }
        if let Some(name) = &name {
            validate_name(name)?;
        }
        let writer = self.writers.entry(id.clone()).or_insert_with(|| Reader {
            ticket: ticket.to_string(),
            name: format!("Machine {}", &id[..8]),
            entries: Vec::new(),
            log: None,
            enabled: true,
        });
        writer.ticket = ticket.to_string();
        writer.enabled = true;
        if let Some(name) = name {
            writer.name = name;
        }
        self.selected = Some(id.clone());
        Ok(id)
    }

    /// Save the name of the machine's log. A new log replaces the saved
    /// history, because its entry ids start again at 1.
    pub fn set_log(&mut self, writer: &str, log: Option<String>) -> Result<()> {
        let reader = self
            .writers
            .get_mut(writer)
            .context("the machine does not exist")?;
        if log.is_none() || reader.log == log {
            return Ok(());
        }
        if reader.log.is_some() {
            reader.entries.clear();
        }
        reader.log = log;
        Ok(())
    }

    /// Save an entry. An entry that is saved already changes nothing, so a
    /// machine can send it on two connections.
    pub fn push(&mut self, writer: &str, entry: Entry) -> Result<()> {
        entry.validate()?;
        let entries = &mut self
            .writers
            .get_mut(writer)
            .context("the machine does not exist")?
            .entries;
        if let Some(old) = entries.iter().rev().find(|e| e.id == entry.id) {
            if old != &entry {
                bail!("the machine changed a saved entry");
            }
            return Ok(());
        }
        if entries.last().is_some_and(|old| old.id >= entry.id) {
            bail!("the machine sent entries out of order");
        }
        entries.push(entry);
        Ok(())
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
        bail!("use a machine name of 1 to 80 bytes without control characters");
    }
    Ok(())
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use iroh::{EndpointAddr, SecretKey};

    fn ticket() -> EndpointTicket {
        EndpointTicket::new(EndpointAddr::new(SecretKey::generate().public()))
    }

    fn entry(id: u64, body: &str) -> Entry {
        serde_json::from_value(serde_json::json!({
            "v": 1, "id": id, "time": "2026-11-01T10:00:00Z", "source": "test",
            "kind": "event", "body": body,
        }))
        .unwrap()
    }

    #[test]
    fn an_entry_from_two_connections_is_saved_once() {
        let mut readers = Readers::default();
        let id = readers.add(&ticket(), Some("Laptop".into())).unwrap();
        readers.push(&id, entry(1, "one")).unwrap();
        readers.push(&id, entry(1, "one")).unwrap();
        readers.push(&id, entry(2, "two")).unwrap();
        assert!(readers.push(&id, entry(2, "changed")).is_err());
        assert!(readers.push(&id, entry(1, "one")).is_ok());
        assert_eq!(readers.writers[&id].last_id(), 2);
        readers.validate().unwrap();
    }

    #[test]
    fn a_new_log_replaces_the_saved_history() {
        let mut readers = Readers::default();
        let id = readers.add(&ticket(), None).unwrap();
        readers.set_log(&id, Some("a".into())).unwrap();
        readers.push(&id, entry(1, "old")).unwrap();
        readers.set_log(&id, None).unwrap();
        readers.set_log(&id, Some("a".into())).unwrap();
        assert_eq!(readers.writers[&id].entries.len(), 1);
        readers.set_log(&id, Some("b".into())).unwrap();
        assert!(readers.writers[&id].entries.is_empty());
        readers.push(&id, entry(1, "new")).unwrap();
        assert_eq!(readers.writers[&id].log.as_deref(), Some("b"));
    }
}
