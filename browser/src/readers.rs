//! Saved machines and their delivery cursors. Entries use platform storage.
use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use yappr::{entry::Entry, pairing::Invitation, EndpointTicket};

fn enabled() -> bool {
    true
}

/// The most machines that one page reads.
const MAX_WRITERS: usize = 32;
/// The most entries that the page keeps for one machine, in all its logs. A
/// new entry removes the oldest entries above this number.
pub const LIMIT: u64 = 5000;

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
    /// The name of the machine's current log, see `yappr::store`. Entries
    /// from before log names have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    /// The earlier logs of the machine that the page keeps, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub old_logs: Vec<OldLog>,
    /// The page does not read the feed of this machine. It takes only the
    /// deliveries of the machine, such as a CI job that it allowed by key.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delivers_only: bool,
    /// A paused machine is not read, and its deliveries are refused.
    #[serde(default = "enabled")]
    pub enabled: bool,
}

/// An earlier log of a machine, and the id of its last saved entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OldLog {
    pub log: String,
    pub last_id: u64,
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
            old_logs: Vec::new(),
            delivers_only: false,
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

    /// Add a machine that the page allows by its key, such as a CI job. The
    /// page takes its deliveries and does not read its feed.
    pub fn allow_machine(&mut self, id: iroh::EndpointId, name: String) -> Result<String> {
        validate_name(&name)?;
        let ticket = EndpointTicket::new(iroh::EndpointAddr::new(id));
        let key = self.add(&ticket, Some(name))?;
        self.writers
            .get_mut(&key)
            .expect("added machine")
            .delivers_only = true;
        Ok(key)
    }

    /// Save the name of the machine's current log. A new log starts at entry
    /// 1, and the page keeps the entries of the earlier log.
    pub fn set_log(&mut self, writer: &str, log: Option<String>) -> Result<()> {
        let reader = self
            .writers
            .get_mut(writer)
            .context("the machine does not exist")?;
        if log.is_none() || reader.log == log {
            return Ok(());
        }
        if let Some(old) = reader.log.take() {
            reader.old_logs.push(OldLog {
                log: old,
                last_id: reader.last_id,
            });
            reader.last_id = 0;
        }
        reader.log = log;
        Ok(())
    }

    /// The earlier logs to remove, oldest first, so that the machine keeps
    /// at most [`LIMIT`] entries. They leave the book.
    pub fn trim(&mut self, writer: &str) -> Vec<String> {
        let Some(reader) = self.writers.get_mut(writer) else {
            return Vec::new();
        };
        let mut total =
            reader.last_id.min(LIMIT) + reader.old_logs.iter().map(|l| l.last_id).sum::<u64>();
        let mut drop = Vec::new();
        while total > LIMIT && !reader.old_logs.is_empty() {
            let old = reader.old_logs.remove(0);
            total -= old.last_id;
            drop.push(old.log);
        }
        drop
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
    fn a_new_log_keeps_the_earlier_logs_until_the_limit() {
        let mut readers = Readers::default();
        let id = readers.add(&ticket(), None).unwrap();
        readers.set_log(&id, Some("a".into())).unwrap();
        readers.saved(&id, 5).unwrap();
        readers.set_log(&id, None).unwrap();
        readers.set_log(&id, Some("a".into())).unwrap();
        assert_eq!(readers.writers[&id].last_id, 5);
        readers.set_log(&id, Some("b".into())).unwrap();
        assert_eq!(readers.writers[&id].last_id, 0);
        assert_eq!(
            readers.writers[&id].old_logs,
            [OldLog {
                log: "a".into(),
                last_id: 5
            }]
        );
        assert!(readers.is_new(&id, &entry(1, "new")).unwrap());
        assert!(readers.trim(&id).is_empty());
        readers.saved(&id, LIMIT).unwrap();
        assert_eq!(readers.trim(&id), ["a"]);
        assert!(readers.writers[&id].old_logs.is_empty());
    }

    #[test]
    fn a_machine_allowed_by_key_only_delivers() {
        let mut readers = Readers::default();
        let key = iroh::SecretKey::generate().public();
        let id = readers.allow_machine(key, "CI deploys".into()).unwrap();
        assert_eq!(id, key.to_string());
        assert!(readers.writers[&id].delivers_only);
        assert_eq!(readers.writers[&id].name, "CI deploys");
        readers.validate().unwrap();
        assert!(readers.allow_machine(key, "\u{1b}".into()).is_err());
    }
}
