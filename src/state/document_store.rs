use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use relm4::gtk::glib;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::sql_file::MAX_SQL_FILE_BYTES;

const MAX_RECORD_BYTES: u64 = (MAX_SQL_FILE_BYTES * 12 + 1024 * 1024) as u64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryRecord {
    pub document_id: String,
    #[serde(default)]
    pub source_tab_id: Option<u64>,
    pub connection_id: String,
    pub database: String,
    pub uri: Option<String>,
    pub text: String,
    pub saved_text: String,
    pub etag: Option<String>,
    pub bom: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryLoad {
    pub records: Vec<RecoveryRecord>,
    pub warnings: Vec<String>,
}

pub fn save(record: &RecoveryRecord) -> Result<(), String> {
    save_in(&directory(), record)
}

pub fn load_for_database(connection_id: &str, database: &str) -> Result<RecoveryLoad, String> {
    load_in(&directory(), connection_id, database)
}

pub fn remove(document_id: &str) -> Result<(), String> {
    remove_in(&directory(), document_id)
}

fn directory() -> PathBuf {
    glib::user_data_dir().join("codd/documents")
}

fn record_path(directory: &Path, document_id: &str) -> Result<PathBuf, String> {
    let id = Uuid::parse_str(document_id).map_err(|error| error.to_string())?;

    Ok(directory.join(format!("{id}.json")))
}

fn validate(record: &RecoveryRecord) -> Result<(), String> {
    Uuid::parse_str(&record.document_id).map_err(|error| error.to_string())?;
    if record.connection_id.is_empty() || record.database.is_empty() {
        return Err("recovery record requires connection and database".into());
    }
    let limit = MAX_SQL_FILE_BYTES - if record.bom { 3 } else { 0 };
    if record.text.len() > limit || record.saved_text.len() > MAX_SQL_FILE_BYTES {
        return Err("recovery SQL exceeds 16 MiB".into());
    }
    if record.text.contains('\0') || record.saved_text.contains('\0') {
        return Err("recovery SQL contains NUL".into());
    }

    Ok(())
}

fn save_in(directory: &Path, record: &RecoveryRecord) -> Result<(), String> {
    validate(record)?;
    let path = record_path(directory, &record.document_id)?;
    let bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err("recovery record exceeds size limit".into());
    }
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(|error| error.to_string())?;
    let metadata = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("recovery directory is not a regular directory".into());
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let temporary = directory.join(format!(".{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;

        File::open(directory)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }

    result.map_err(|error: io::Error| error.to_string())
}

fn load_in(directory: &Path, connection_id: &str, database: &str) -> Result<RecoveryLoad, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RecoveryLoad::default());
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let record = match read_record(directory, &path) {
            Ok(record) => record,
            Err(error) => {
                warnings.push(format!(
                    "Skipping recovery record {}: {error}",
                    path.display()
                ));
                continue;
            }
        };
        if record.connection_id == connection_id && record.database == database {
            records.push(record);
        }
    }
    records.sort_by(|left, right| left.document_id.cmp(&right.document_id));

    Ok(RecoveryLoad { records, warnings })
}

fn read_record(directory: &Path, path: &Path) -> Result<RecoveryRecord, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("recovery record is not a regular file".into());
    }
    if metadata.len() > MAX_RECORD_BYTES {
        return Err("recovery record exceeds size limit".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err("recovery record exceeds size limit".into());
    }
    let record: RecoveryRecord =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    validate(&record)?;
    if record_path(directory, &record.document_id)? != path {
        return Err("recovery record ID does not match filename".into());
    }

    Ok(record)
}

fn remove_in(directory: &Path, document_id: &str) -> Result<(), String> {
    match fs::remove_file(record_path(directory, document_id)?) {
        Ok(()) => File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("codd-recovery-{}", Uuid::new_v4())))
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn record() -> RecoveryRecord {
        RecoveryRecord {
            document_id: Uuid::new_v4().to_string(),
            source_tab_id: Some(42),
            connection_id: "connection".into(),
            database: "database".into(),
            uri: Some("file:///tmp/query.sql".into()),
            text: "  select 'é';\r\n\n ".into(),
            saved_text: "\r\n".into(),
            etag: Some("version".into()),
            bom: true,
        }
    }

    #[test]
    fn roundtrip_independent_records_and_permissions() {
        let directory = Directory::new();
        let first = record();
        let mut second = record();
        second.database = "other".into();
        save_in(&directory.0, &first).unwrap();
        save_in(&directory.0, &second).unwrap();
        assert_eq!(
            load_in(&directory.0, "connection", "database")
                .unwrap()
                .records,
            vec![first.clone()]
        );
        let path = record_path(&directory.0, &first.document_id).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
        remove_in(&directory.0, &first.document_id).unwrap();
        remove_in(&directory.0, &first.document_id).unwrap();
        assert!(
            load_in(&directory.0, "connection", "database")
                .unwrap()
                .records
                .is_empty()
        );
        assert_eq!(
            load_in(&directory.0, "connection", "other")
                .unwrap()
                .records,
            vec![second]
        );
    }

    #[test]
    fn invalid_updates_preserve_last_good_record() {
        let directory = Directory::new();
        let good = record();
        save_in(&directory.0, &good).unwrap();
        let mut bad = good.clone();
        bad.text = " ".repeat(MAX_SQL_FILE_BYTES);
        assert!(save_in(&directory.0, &bad).is_err());
        bad.text = "\0".into();
        assert!(save_in(&directory.0, &bad).is_err());
        assert_eq!(
            load_in(&directory.0, "connection", "database")
                .unwrap()
                .records,
            vec![good]
        );
        assert!(remove_in(&directory.0, "../../escape").is_err());
        bad.document_id = "../../escape".into();
        assert!(save_in(&directory.0, &bad).is_err());
    }

    #[test]
    fn corrupt_record_does_not_block_other_documents() {
        let directory = Directory::new();
        let good = record();
        save_in(&directory.0, &good).unwrap();
        let path = record_path(&directory.0, &good.document_id).unwrap();
        fs::write(&path, b"broken").unwrap();
        let intact = record();
        save_in(&directory.0, &intact).unwrap();
        let intact_path = record_path(&directory.0, &intact.document_id).unwrap();
        let intact_bytes = fs::read(&intact_path).unwrap();
        let loaded = load_in(&directory.0, "connection", "database").unwrap();
        assert_eq!(loaded.records, vec![intact]);
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains(&path.display().to_string()));
        assert_eq!(fs::read(path).unwrap(), b"broken");
        assert_eq!(fs::read(intact_path).unwrap(), intact_bytes);
    }

    #[test]
    fn preserves_empty_and_full_size_documents() {
        let directory = Directory::new();
        assert!(
            load_in(&directory.0, "connection", "database")
                .unwrap()
                .records
                .is_empty()
        );
        let mut record = record();
        record.bom = false;
        record.text = "\t".repeat(MAX_SQL_FILE_BYTES);
        record.saved_text.clear();
        save_in(&directory.0, &record).unwrap();
        assert_eq!(
            load_in(&directory.0, "connection", "database")
                .unwrap()
                .records,
            vec![record.clone()]
        );
        record.text.clear();
        save_in(&directory.0, &record).unwrap();
        assert_eq!(
            load_in(&directory.0, "connection", "database")
                .unwrap()
                .records,
            vec![record]
        );
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn both_full_size_snapshots_fit_even_with_maximum_json_escaping() {
        let directory = Directory::new();
        let mut record = record();
        record.text = "\u{1}".repeat(MAX_SQL_FILE_BYTES - 3);
        record.saved_text = "\u{1}".repeat(MAX_SQL_FILE_BYTES);
        save_in(&directory.0, &record).unwrap();
        let path = record_path(&directory.0, &record.document_id).unwrap();
        assert!(fs::metadata(path).unwrap().len() <= MAX_RECORD_BYTES);
        let loaded = load_in(&directory.0, "connection", "database").unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.records[0], record);
    }

    #[test]
    fn oversized_and_mismatched_records_are_skipped_untouched() {
        let directory = Directory::new();
        let good = record();
        save_in(&directory.0, &good).unwrap();
        let oversized = directory.0.join(format!("{}.json", Uuid::new_v4()));
        File::create(&oversized)
            .unwrap()
            .set_len(MAX_RECORD_BYTES + 1)
            .unwrap();
        let mismatch = directory.0.join(format!("{}.json", Uuid::new_v4()));
        fs::write(&mismatch, serde_json::to_vec(&good).unwrap()).unwrap();
        let link = directory.0.join(format!("{}.json", Uuid::new_v4()));
        std::os::unix::fs::symlink(&mismatch, &link).unwrap();
        let loaded = load_in(&directory.0, "connection", "database").unwrap();
        assert_eq!(loaded.records, vec![good]);
        assert_eq!(loaded.warnings.len(), 3);
        assert_eq!(fs::metadata(oversized).unwrap().len(), MAX_RECORD_BYTES + 1);
        assert!(mismatch.exists());
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    }

    #[test]
    fn atomic_recovery_write_failure_preserves_last_good_record() {
        const CHILD_DIRECTORY: &str = "CODD_TEST_ATOMIC_RECOVERY_DIRECTORY";
        if let Some(directory) = std::env::var_os(CHILD_DIRECTORY) {
            let directory = PathBuf::from(directory);
            let records = load_in(&directory, "connection", "database").unwrap();
            let good = &records.records[0];
            let mut update = good.clone();
            update.text = "x".repeat(64 * 1024);
            assert!(save_in(&directory, &update).is_err());
            assert_eq!(
                load_in(&directory, "connection", "database").unwrap(),
                records
            );
            return;
        }
        let directory = Directory::new();
        let good = record();
        save_in(&directory.0, &good).unwrap();
        let status = std::process::Command::new("sh")
            .args([
                "-c",
                "trap '' XFSZ; ulimit -f 1; exec \"$@\"",
                "codd-recovery-test",
            ])
            .arg(std::env::current_exe().unwrap())
            .args([
                "atomic_recovery_write_failure_preserves_last_good_record",
                "--test-threads=1",
            ])
            .env(CHILD_DIRECTORY, &directory.0)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            load_in(&directory.0, "connection", "database")
                .unwrap()
                .records,
            vec![good]
        );
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn legacy_records_default_source_tab_id_to_none() {
        let directory = Directory::new();
        let mut expected = record();
        expected.source_tab_id = None;
        save_in(&directory.0, &expected).unwrap();
        let path = record_path(&directory.0, &expected.document_id).unwrap();
        let mut legacy = serde_json::to_value(&expected).unwrap();
        legacy.as_object_mut().unwrap().remove("source_tab_id");
        fs::write(path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let loaded = load_in(&directory.0, "connection", "database").unwrap();
        assert_eq!(loaded.records, vec![expected]);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn directory_errors_are_fatal_and_absent_directory_is_empty() {
        let directory = Directory::new();
        assert_eq!(
            load_in(&directory.0, "connection", "database").unwrap(),
            RecoveryLoad::default()
        );
        fs::write(&directory.0, b"not a directory").unwrap();
        assert!(load_in(&directory.0, "connection", "database").is_err());
        fs::remove_file(&directory.0).unwrap();
    }
}
