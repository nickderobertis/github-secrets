use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A named secret tracked locally. `created` and `updated` are only used to
/// decide which secrets need re-syncing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Secret {
    pub name: String,
    pub value: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

impl Secret {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            name: name.into(),
            value: value.into(),
            created: now,
            updated: now,
        }
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.updated = Utc::now();
    }
}

/// Outcome of an upsert: was this newly created, or did we update an existing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upsert {
    Created,
    Updated,
}

/// Secrets that apply to every included repository in a profile.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalSecrets {
    #[serde(default)]
    pub secrets: Vec<Secret>,
}

impl GlobalSecrets {
    pub fn upsert(&mut self, name: &str, value: &str) -> Upsert {
        for s in &mut self.secrets {
            if s.name == name {
                s.set_value(value);
                return Upsert::Updated;
            }
        }
        self.secrets.push(Secret::new(name, value));
        Upsert::Created
    }

    pub fn get(&self, name: &str) -> Option<&Secret> {
        self.secrets.iter().find(|s| s.name == name)
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.secrets.len();
        self.secrets.retain(|s| s.name != name);
        before != self.secrets.len()
    }
}

/// Secrets scoped to a specific repository (overrides the global value when both
/// exist).
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySecrets {
    #[serde(default)]
    pub by_repo: BTreeMap<String, Vec<Secret>>,
}

impl RepositorySecrets {
    pub fn upsert(&mut self, repository: &str, name: &str, value: &str) -> Upsert {
        let entries = self.by_repo.entry(repository.to_string()).or_default();
        for s in entries.iter_mut() {
            if s.name == name {
                s.set_value(value);
                return Upsert::Updated;
            }
        }
        entries.push(Secret::new(name, value));
        Upsert::Created
    }

    pub fn get(&self, repository: &str, name: &str) -> Option<&Secret> {
        self.by_repo
            .get(repository)
            .and_then(|v| v.iter().find(|s| s.name == name))
    }

    pub fn remove(&mut self, repository: &str, name: &str) -> bool {
        let Some(entries) = self.by_repo.get_mut(repository) else {
            return false;
        };
        let before = entries.len();
        entries.retain(|s| s.name != name);
        before != entries.len()
    }

    pub fn names_for(&self, repository: &str) -> Vec<&str> {
        self.by_repo
            .get(repository)
            .map(|v| v.iter().map(|s| s.name.as_str()).collect())
            .unwrap_or_default()
    }
}

/// One record of a successful push of a named secret to a specific repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncRecord {
    pub secret_name: String,
    pub last_synced: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_upsert_creates_then_updates() {
        let mut g = GlobalSecrets::default();
        assert_eq!(g.upsert("A", "1"), Upsert::Created);
        assert_eq!(g.upsert("A", "2"), Upsert::Updated);
        assert_eq!(g.get("A").unwrap().value, "2");
    }

    #[test]
    fn repository_upsert_isolates_per_repo() {
        let mut r = RepositorySecrets::default();
        r.upsert("o/a", "K", "1");
        r.upsert("o/b", "K", "2");
        assert_eq!(r.get("o/a", "K").unwrap().value, "1");
        assert_eq!(r.get("o/b", "K").unwrap().value, "2");
    }

    #[test]
    fn remove_returns_whether_existed() {
        let mut g = GlobalSecrets::default();
        g.upsert("A", "1");
        assert!(g.remove("A"));
        assert!(!g.remove("A"));
    }
}
