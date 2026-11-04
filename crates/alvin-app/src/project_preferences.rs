//! Local project presentation settings, separate from omp's files.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use alvin_conversation_ui::{ProjectPreference, ProjectPreferences};
use serde_json::{Value, json};

use crate::{Error, Result};

pub(crate) struct ProjectPreferencesFile {
  path: PathBuf,
}

impl ProjectPreferencesFile {
  pub(crate) fn new(path: PathBuf) -> Self {
    Self { path }
  }

  pub(crate) fn load(&self) -> Result<ProjectPreferences> {
    let bytes = match std::fs::read(&self.path) {
      Ok(bytes) => bytes,
      Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(ProjectPreferences::new()),
      Err(source) => {
        return Err(Error::Read {
          path: self.path.clone(),
          source,
        });
      }
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|source| Error::Parse {
      path: self.path.clone(),
      source,
    })?;
    let invalid = || Error::Invalid { path: self.path.clone() };
    let projects = value.as_object().ok_or_else(invalid)?;
    let mut preferences = ProjectPreferences::new();
    for (key, value) in projects {
      let object = value.as_object().ok_or_else(invalid)?;
      let alias = match object.get("alias").ok_or_else(invalid)? {
        Value::Null => None,
        Value::String(alias) => Some(alias.clone()),
        _ => return Err(invalid()),
      };
      let pinned = object.get("pinned").and_then(Value::as_bool).ok_or_else(invalid)?;
      let hidden = object.get("hidden").and_then(Value::as_bool).ok_or_else(invalid)?;
      let sessions = |name: &str| -> Result<_> {
        object.get(name).map_or_else(
          || Ok(Default::default()),
          |value| {
            value
              .as_array()
              .ok_or_else(invalid)?
              .iter()
              .map(|value| value.as_str().map(str::to_owned).ok_or_else(invalid))
              .collect()
          },
        )
      };
      let pinned_sessions = sessions("pinned_sessions")?;
      let hidden_sessions = sessions("hidden_sessions")?;
      preferences.insert(
        key.clone(),
        ProjectPreference {
          alias,
          pinned,
          hidden,
          pinned_sessions,
          hidden_sessions,
        },
      );
    }
    Ok(preferences)
  }

  pub(crate) fn save(&self, preferences: &ProjectPreferences) -> Result<()> {
    let projects: serde_json::Map<String, Value> = preferences
      .iter()
      .map(|(key, preference)| {
        (
          key.clone(),
          json!({
            "alias": preference.alias, "pinned": preference.pinned, "hidden": preference.hidden,
            "pinned_sessions": preference.pinned_sessions, "hidden_sessions": preference.hidden_sessions,
          }),
        )
      })
      .collect();
    let parent = self
      .path
      .parent()
      .filter(|path| !path.as_os_str().is_empty())
      .unwrap_or_else(|| Path::new("."));
    let write = || -> std::io::Result<()> {
      std::fs::create_dir_all(parent)?;
      let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
      let temporary = parent.join(format!(".alvin-projects-{}-{nonce}.json", std::process::id()));
      let mut options = std::fs::OpenOptions::new();
      options.write(true).create_new(true);
      #[cfg(unix)]
      {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
      }
      let mut file = options.open(&temporary)?;
      let result = (|| {
        serde_json::to_writer_pretty(&mut file, &projects).map_err(std::io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &self.path)
      })();
      if result.is_err() && temporary.exists() {
        std::fs::remove_file(&temporary)?;
      }
      result
    };
    write().map_err(|source| Error::Write {
      path: self.path.clone(),
      source,
    })
  }
}

#[cfg(test)]
mod tests {
  use alvin_conversation_ui::{ProjectPreference, ProjectPreferences};
  use alvin_protocol::EventId;

  use super::ProjectPreferencesFile;
  use crate::Error;

  #[test]
  fn preferences_round_trip_and_report_bad_files() {
    let dir = std::env::temp_dir().join(format!("alvin-preferences-{}", EventId::new().as_uuid()));
    let store = ProjectPreferencesFile::new(dir.join("projects.json"));
    assert!(store.load().unwrap().is_empty());
    let preferences = ProjectPreferences::from([(
      "project".to_owned(),
      ProjectPreference {
        alias: Some("Work".to_owned()),
        pinned: true,
        hidden: true,
        pinned_sessions: ["session-a".to_owned()].into_iter().collect(),
        hidden_sessions: ["session-b".to_owned()].into_iter().collect(),
      },
    )]);
    store.save(&preferences).unwrap();
    let loaded = store.load().unwrap();
    let entry = loaded.get("project").unwrap();
    assert_eq!(entry.alias.as_deref(), Some("Work"));
    assert!(entry.pinned && entry.hidden);
    assert_eq!(entry.pinned_sessions, ["session-a".to_owned()].into_iter().collect());
    assert_eq!(entry.hidden_sessions, ["session-b".to_owned()].into_iter().collect());
    std::fs::write(
      &store.path,
      "{\"project\": {\"alias\": null, \"pinned\": false, \"hidden\": false}}",
    )
    .unwrap();
    let legacy = store.load().unwrap();
    assert!(legacy["project"].pinned_sessions.is_empty());
    assert!(legacy["project"].hidden_sessions.is_empty());
    std::fs::write(
      &store.path,
      "{\"project\": {\"alias\": null, \"pinned\": false, \"hidden\": false, \"pinned_sessions\": [1]}}",
    )
    .unwrap();
    assert!(matches!(store.load(), Err(Error::Invalid { .. })));
    std::fs::write(&store.path, "{").unwrap();
    assert!(matches!(store.load(), Err(Error::Parse { .. })));
    std::fs::write(&store.path, "{\"project\": {\"alias\": null, \"pinned\": 1, \"hidden\": false}}").unwrap();
    assert!(matches!(store.load(), Err(Error::Invalid { .. })));
    std::fs::remove_file(&store.path).unwrap();
    std::fs::create_dir(&store.path).unwrap();
    assert!(matches!(store.load(), Err(Error::Read { .. })));
    assert!(matches!(store.save(&preferences), Err(Error::Write { .. })));
    std::fs::remove_dir_all(dir).unwrap();
  }
}
