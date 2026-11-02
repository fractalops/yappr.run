//! The machines that this page reads. Their entries are in IndexedDB, see
//! `entries.rs`.
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
    /// The id of the last saved entry, or 0.
    #[serde(default)]
    pub last_id: u64,
    /// Entries that a page saved here before it used IndexedDB. The page
    /// moves them to IndexedDB once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<Entry>,
    /// The name of the machine's log, see `yappr::store`. Entries from
    /// before log names have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    /// A paused machine is not read, and its deliveries are refused.
    #[serde(default = "enabled")]
    pub enabled: bool,
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
            last_id: 0,
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

    /// Save the name of the machine's log. Returns true when a new log
    /// replaces the saved history, because its entry ids start again at 1.
    pub fn set_log(&mut self, writer: &str, log: Option<String>) -> Result<bool> {
        let reader = self
            .writers
            .get_mut(writer)
            .context("the machine does not exist")?;
        if log.is_none() || reader.log == log {
            return Ok(false);
        }
        let reset = reader.log.is_some();
        if reset {
            reader.last_id = 0;
        }
        reader.log = log;
        Ok(reset)
    }

    /// True if `entry` is new: its id is after the last saved id. A machine
    /// can send an entry on two connections, and the page saves it once.
    pub fn is_new(&self, writer: &str, entry: &Entry) -> Result<bool> {
        let reader = self
            .writers
            .get(writer)
            .context("the machine does not exist")?;
        entry.validate()?;
        Ok(entry.id > reader.last_id)
    }

    /// Move the last saved id forward to `id`.
    pub fn saved(&mut self, writer: &str, id: u64) -> Result<()> {
        let reader = self
            .writers
            .get_mut(writer)
            .context("the machine does not exist")?;
        reader.last_id = reader.last_id.max(id);
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
    fn an_entry_after_the_last_saved_id_is_new() {
        let mut readers = Readers::default();
        let id = readers.add(&ticket(), Some("Laptop".into())).unwrap();
        assert!(readers.is_new(&id, &entry(1, "one")).unwrap());
        readers.saved(&id, 2).unwrap();
        assert!(!readers.is_new(&id, &entry(2, "two")).unwrap());
        assert!(readers.is_new(&id, &entry(3, "three")).unwrap());
        readers.saved(&id, 1).unwrap();
        assert_eq!(readers.writers[&id].last_id, 2);
        readers.validate().unwrap();
    }

    #[test]
    fn a_new_log_starts_the_history_again() {
        let mut readers = Readers::default();
        let id = readers.add(&ticket(), None).unwrap();
        assert!(!readers.set_log(&id, Some("a".into())).unwrap());
        readers.saved(&id, 5).unwrap();
        assert!(!readers.set_log(&id, None).unwrap());
        assert!(!readers.set_log(&id, Some("a".into())).unwrap());
        assert_eq!(readers.writers[&id].last_id, 5);
        assert!(readers.set_log(&id, Some("b".into())).unwrap());
        assert_eq!(readers.writers[&id].last_id, 0);
        assert!(readers.is_new(&id, &entry(1, "new")).unwrap());
    }
}
