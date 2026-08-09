use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result};

use crate::models::AppSettings;

pub fn load(path: &Path) -> AppSettings {
    if !path.exists() {
        return AppSettings::default();
    }

    match fs::read_to_string(path)
        .context("reading settings")
        .and_then(|json| serde_json::from_str::<AppSettings>(&json).context("parsing settings"))
    {
        Ok(mut settings) => {
            let stored_schema_version = settings.schema_version;
            settings.normalize();
            if settings.schema_version != stored_schema_version
                && let Err(error) = save(path, &settings)
            {
                eprintln!("AI Dock could not persist migrated settings: {error:#}");
            }
            settings
        }
        Err(error) => {
            eprintln!("AI Dock could not load settings: {error:#}");
            let backup = path.with_extension(format!(
                "corrupt-{}.json",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0)
            ));
            let _ = fs::copy(path, backup);
            AppSettings::default()
        }
    }
}

pub fn save(path: &Path, settings: &AppSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("creating settings directory")?;
    }

    let json = serde_json::to_vec_pretty(settings).context("serializing settings")?;
    let directory = path.parent().context("settings path has no directory")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(directory).context("creating temporary settings file")?;
    temporary.write_all(&json).context("writing settings")?;
    temporary.flush().context("flushing settings")?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .context("replacing settings file")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_replace_existing_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        let mut expected = AppSettings::default();

        save(&path, &expected).expect("first save");
        expected.popup_height = 777.0;
        save(&path, &expected).expect("replacement save");
        let actual = load(&path);

        assert_eq!(actual.popup_height, 777.0);
        assert_eq!(actual.sessions[0].id, expected.sessions[0].id);
    }

    #[test]
    fn corrupt_settings_fall_back_without_deleting_source() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        fs::write(&path, "{ definitely not json").expect("write corrupt settings");

        let loaded = load(&path);

        assert_eq!(loaded.schema_version, crate::models::CURRENT_SCHEMA_VERSION);
        assert!(path.exists());
        assert!(
            directory
                .path()
                .read_dir()
                .expect("list backups")
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("settings.corrupt-"))
        );
    }

    #[test]
    fn legacy_sessions_gain_independent_window_groups() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        let mut legacy = serde_json::to_value(AppSettings::default()).expect("serialize settings");
        legacy["schemaVersion"] = serde_json::json!(5);
        legacy["sessions"][0]
            .as_object_mut()
            .expect("session object")
            .remove("groupId");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy settings"),
        )
        .expect("write legacy settings");

        let loaded = load(&path);

        assert_eq!(loaded.schema_version, crate::models::CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.sessions[0].group_id, loaded.sessions[0].id);
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read migrated settings"))
                .expect("parse migrated settings");
        assert_eq!(
            persisted["schemaVersion"],
            serde_json::json!(crate::models::CURRENT_SCHEMA_VERSION)
        );
        assert_eq!(
            persisted["sessions"][0]["groupId"],
            serde_json::json!(loaded.sessions[0].id)
        );
    }
}
