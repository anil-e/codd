use std::fmt;

use gio::prelude::*;
use relm4::gtk::{gio, glib};
use uuid::Uuid;

pub const MAX_SQL_FILE_BYTES: usize = 16 * 1024 * 1024;
const ATTRIBUTES: &str = "standard::type,standard::is-symlink,standard::size,etag::value,unix::mode,unix::uid,unix::gid,xattr-sys::system.posix_acl_access";
const POSIX_ACL: &str = "xattr-sys::system.posix_acl_access";

#[derive(Debug)]
pub struct LoadedSqlFile {
    pub text: String,
    pub etag: Option<String>,
    pub bom: bool,
}

#[derive(Debug)]
pub enum SaveTarget {
    Existing { etag: Option<String> },
    New,
    Overwrite { etag: Option<String> },
}

#[derive(Debug)]
pub enum FileError {
    Conflict,
    Deleted,
    Unverified,
    TooLarge,
    InvalidEncoding,
    Unsupported,
    Io(glib::Error),
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict => f.write_str("file version changed"),
            Self::Deleted => f.write_str("file no longer exists"),
            Self::Unverified => f.write_str("file version cannot be verified"),
            Self::TooLarge => f.write_str("SQL file exceeds 16 MiB"),
            Self::InvalidEncoding => f.write_str("SQL must be UTF-8 without NUL bytes"),
            Self::Unsupported => {
                f.write_str("SQL file target or permission metadata is not supported")
            }
            Self::Io(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<glib::Error> for FileError {
    fn from(error: glib::Error) -> Self {
        if error.matches(gio::IOErrorEnum::WrongEtag) || error.matches(gio::IOErrorEnum::Exists) {
            Self::Conflict
        } else if error.matches(gio::IOErrorEnum::NotFound) {
            Self::Deleted
        } else {
            Self::Io(error)
        }
    }
}

async fn file_info(file: &gio::File) -> Result<gio::FileInfo, FileError> {
    if file.path().is_none() {
        return Err(FileError::Unsupported);
    }
    let info = file
        .query_info_future(
            ATTRIBUTES,
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            glib::Priority::DEFAULT,
        )
        .await?;
    if info.is_symlink() || info.file_type() != gio::FileType::Regular {
        return Err(FileError::Unsupported);
    }

    Ok(info)
}

fn snapshot_etag(infos: &[&gio::FileInfo]) -> Result<Option<String>, FileError> {
    let mut etag: Option<String> = None;
    let mut verified = true;
    for info in infos {
        match info
            .attribute_string("etag::value")
            .filter(|value| !value.is_empty())
        {
            Some(value) => {
                if etag.as_deref().is_some_and(|etag| etag != value.as_str()) {
                    return Err(FileError::Conflict);
                }
                etag = Some(value.into());
            }
            None => verified = false,
        }
    }

    Ok(etag.filter(|_| verified))
}

pub async fn load(file: &gio::File) -> Result<LoadedSqlFile, FileError> {
    let path_info = file_info(file).await?;
    let stream = file.read_future(glib::Priority::DEFAULT).await?;
    let result = async {
        let before = stream
            .query_info_future(ATTRIBUTES, glib::Priority::DEFAULT)
            .await?;
        if before.size() > MAX_SQL_FILE_BYTES as i64 {
            return Err(FileError::TooLarge);
        }
        let mut bytes = Vec::new();
        loop {
            let count = (MAX_SQL_FILE_BYTES + 1 - bytes.len()).min(64 * 1024);
            let chunk = stream
                .read_bytes_future(count, glib::Priority::DEFAULT)
                .await?;
            if chunk.is_empty() {
                break;
            }
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_SQL_FILE_BYTES {
                return Err(FileError::TooLarge);
            }
        }
        let after = stream
            .query_info_future(ATTRIBUTES, glib::Priority::DEFAULT)
            .await?;
        let current = file_info(file).await?;
        let etag = snapshot_etag(&[&path_info, &before, &after, &current])?;
        if after.size() != before.size()
            || current.size() != before.size()
            || bytes.len() as i64 != before.size()
        {
            return Err(FileError::Conflict);
        }
        let bom = bytes.starts_with(b"\xef\xbb\xbf");
        let text = String::from_utf8(if bom { bytes.split_off(3) } else { bytes })
            .map_err(|_| FileError::InvalidEncoding)?;
        if text.contains('\0') {
            return Err(FileError::InvalidEncoding);
        }

        Ok(LoadedSqlFile { text, etag, bom })
    }
    .await;
    let closed = stream.close_future(glib::Priority::DEFAULT).await;
    let loaded = result?;
    closed?;

    Ok(loaded)
}

pub async fn save(
    file: &gio::File,
    text: String,
    bom: bool,
    target: SaveTarget,
) -> Result<Option<String>, FileError> {
    if text.len() > MAX_SQL_FILE_BYTES - if bom { 3 } else { 0 } {
        return Err(FileError::TooLarge);
    }
    if text.contains('\0') {
        return Err(FileError::InvalidEncoding);
    }
    if file.path().is_none() {
        return Err(FileError::Unsupported);
    }
    let bytes = if bom {
        let mut bytes = Vec::with_capacity(text.len() + 3);
        bytes.extend_from_slice(b"\xef\xbb\xbf");
        bytes.extend_from_slice(text.as_bytes());
        bytes
    } else {
        text.into_bytes()
    };
    let existing = match target {
        SaveTarget::New => None,
        SaveTarget::Existing { etag } | SaveTarget::Overwrite { etag } => {
            let etag = etag
                .filter(|value| !value.is_empty())
                .ok_or(FileError::Unverified)?;
            let info = file_info(file).await?;
            let actual_etag = info
                .attribute_string("etag::value")
                .filter(|value| !value.is_empty())
                .ok_or(FileError::Unverified)?;
            if actual_etag.as_str() != etag {
                return Err(FileError::Conflict);
            }
            Some((etag, info))
        }
    };
    let parent = file.parent().ok_or(FileError::Unsupported)?;
    let temporary = parent.child(format!(".codd-{}.tmp", Uuid::new_v4()));
    let stream = temporary
        .create_future(gio::FileCreateFlags::PRIVATE, glib::Priority::DEFAULT)
        .await?;
    let result = async {
        let written = stream
            .write_all_future(bytes, glib::Priority::DEFAULT)
            .await;
        let closed = stream.close_future(glib::Priority::DEFAULT).await;
        let (_, _, error) = written.map_err(|(_, error)| FileError::from(error))?;
        if let Some(error) = error {
            return Err(error.into());
        }
        closed?;
        let saved_etag = stream.etag().map(Into::into);

        let sync_path = temporary.path().ok_or(FileError::Unsupported)?;
        gio::spawn_blocking(move || std::fs::File::open(sync_path)?.sync_all())
            .await
            .map_err(|_| {
                FileError::Io(glib::Error::new(
                    gio::IOErrorEnum::Failed,
                    "file synchronization worker panicked",
                ))
            })?
            .map_err(|error| {
                FileError::Io(glib::Error::new(
                    gio::IOErrorEnum::Failed,
                    &error.to_string(),
                ))
            })?;

        let mut flags = gio::FileCopyFlags::NO_FALLBACK_FOR_MOVE;
        if let Some((etag, info)) = existing {
            let permissions = gio::FileInfo::new();
            for attribute in ["unix::uid", "unix::gid", "unix::mode"] {
                if info.has_attribute(attribute) {
                    permissions.set_attribute_uint32(attribute, info.attribute_uint32(attribute));
                }
            }
            if let Some(acl) = info.attribute_string(POSIX_ACL) {
                permissions.set_attribute_string(POSIX_ACL, &acl);
            }
            temporary
                .set_attributes_future(
                    &permissions,
                    gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                    glib::Priority::DEFAULT,
                )
                .await?;
            let staged = file_info(&temporary).await?;
            for attribute in ["unix::uid", "unix::gid", "unix::mode"] {
                if info.has_attribute(attribute)
                    && (!staged.has_attribute(attribute)
                        || staged.attribute_uint32(attribute) != info.attribute_uint32(attribute))
                {
                    return Err(FileError::Unsupported);
                }
            }
            if staged.attribute_string(POSIX_ACL) != info.attribute_string(POSIX_ACL) {
                return Err(FileError::Unsupported);
            }

            let current = file_info(file).await?;
            let current_etag = current
                .attribute_string("etag::value")
                .filter(|value| !value.is_empty())
                .ok_or(FileError::Unverified)?;
            if current_etag.as_str() != etag {
                return Err(FileError::Conflict);
            }
            for attribute in ["unix::uid", "unix::gid", "unix::mode"] {
                if current.has_attribute(attribute) != info.has_attribute(attribute)
                    || (info.has_attribute(attribute)
                        && current.attribute_uint32(attribute) != info.attribute_uint32(attribute))
                {
                    return Err(FileError::Conflict);
                }
            }
            if current.attribute_string(POSIX_ACL) != info.attribute_string(POSIX_ACL) {
                return Err(FileError::Conflict);
            }
            flags |= gio::FileCopyFlags::OVERWRITE;
        }
        let (moved, _) = temporary.move_future(file, flags, glib::Priority::DEFAULT);
        moved.await?;

        Ok(saved_etag)
    }
    .await;
    if result.is_err() {
        let _ = temporary.delete_future(glib::Priority::DEFAULT).await;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::PathBuf;
    use std::process::Command;
    use uuid::Uuid;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("codd-sql-{}", Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn file(&self) -> gio::File {
            gio::File::for_path(self.0.join("query.sql"))
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn roundtrip_and_conflicts() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let directory = Directory::new();
                    let file = directory.file();
                    let text = "  select 'é';\r\n\r\n \n";
                    let etag = save(&file, text.into(), true, SaveTarget::New)
                        .await
                        .unwrap();
                    assert!(etag.is_some());
                    let loaded = load(&file).await.unwrap();
                    assert_eq!(loaded.text, text);
                    assert!(loaded.bom);
                    assert_eq!(loaded.etag, etag);
                    assert!(matches!(
                        save(&file, "other".into(), false, SaveTarget::New).await,
                        Err(FileError::Conflict)
                    ));
                    assert_eq!(load(&file).await.unwrap().text, text);
                    assert_eq!(
                        fs::metadata(file.path().unwrap()).unwrap().mode() & 0o777,
                        0o600
                    );
                    assert!(matches!(
                        save(
                            &file,
                            "other".into(),
                            false,
                            SaveTarget::Existing { etag: None }
                        )
                        .await,
                        Err(FileError::Unverified)
                    ));
                    assert!(matches!(
                        save(
                            &file,
                            "other".into(),
                            false,
                            SaveTarget::Overwrite {
                                etag: Some("stale".into())
                            }
                        )
                        .await,
                        Err(FileError::Conflict)
                    ));
                    let etag = save(
                        &file,
                        "select 2;".into(),
                        false,
                        SaveTarget::Existing { etag },
                    )
                    .await
                    .unwrap();
                    assert_eq!(fs::read(file.path().unwrap()).unwrap(), b"select 2;");
                    fs::remove_file(file.path().unwrap()).unwrap();
                    assert!(matches!(
                        save(&file, "other".into(), false, SaveTarget::Existing { etag }).await,
                        Err(FileError::Deleted)
                    ));
                    assert!(!file.path().unwrap().exists());
                })
            })
            .unwrap();
    }

    #[test]
    fn validates_encoding_limits_and_symlinks() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let directory = Directory::new();
                    let file = directory.file();
                    let path = file.path().unwrap();
                    for bytes in [&b"\xff"[..], &b"select\0"[..]] {
                        fs::write(&path, bytes).unwrap();
                        assert!(matches!(load(&file).await, Err(FileError::InvalidEncoding)));
                    }
                    fs::write(&path, vec![b' '; MAX_SQL_FILE_BYTES]).unwrap();
                    assert_eq!(load(&file).await.unwrap().text.len(), MAX_SQL_FILE_BYTES);
                    fs::write(&path, vec![b' '; MAX_SQL_FILE_BYTES + 1]).unwrap();
                    assert!(matches!(load(&file).await, Err(FileError::TooLarge)));
                    fs::remove_file(&path).unwrap();
                    assert!(matches!(
                        save(&file, " ".repeat(MAX_SQL_FILE_BYTES), true, SaveTarget::New).await,
                        Err(FileError::TooLarge)
                    ));
                    assert!(!path.exists());
                    let actual = directory.0.join("actual.sql");
                    fs::write(&actual, "select 1;").unwrap();
                    std::os::unix::fs::symlink(&actual, &path).unwrap();
                    assert!(matches!(load(&file).await, Err(FileError::Unsupported)));
                    assert!(matches!(
                        save(
                            &file,
                            "other".into(),
                            false,
                            SaveTarget::Overwrite {
                                etag: Some("tag".into())
                            }
                        )
                        .await,
                        Err(FileError::Unsupported)
                    ));
                    assert_eq!(fs::read_to_string(actual).unwrap(), "select 1;");
                })
            })
            .unwrap();
    }

    #[test]
    fn approved_overwrite_and_external_changes() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let directory = Directory::new();
                    let file = directory.file();
                    let path = file.path().unwrap();
                    fs::write(&path, b"original").unwrap();
                    let loaded = load(&file).await.unwrap();
                    assert!(!loaded.bom);
                    fs::write(&path, b"external edit with a different length").unwrap();
                    assert!(matches!(
                        save(
                            &file,
                            "replacement".into(),
                            false,
                            SaveTarget::Existing { etag: loaded.etag }
                        )
                        .await,
                        Err(FileError::Conflict)
                    ));
                    assert_eq!(
                        fs::read(&path).unwrap(),
                        b"external edit with a different length"
                    );
                    let loaded = load(&file).await.unwrap();
                    save(
                        &file,
                        String::new(),
                        true,
                        SaveTarget::Overwrite { etag: loaded.etag },
                    )
                    .await
                    .unwrap();
                    assert_eq!(fs::read(&path).unwrap(), b"\xef\xbb\xbf");
                    let loaded = load(&file).await.unwrap();
                    assert!(loaded.text.is_empty());
                    assert!(loaded.bom);
                    assert!(matches!(
                        save(
                            &file,
                            "\0".into(),
                            false,
                            SaveTarget::Existing { etag: loaded.etag }
                        )
                        .await,
                        Err(FileError::InvalidEncoding)
                    ));
                    assert_eq!(fs::read(&path).unwrap(), b"\xef\xbb\xbf");
                })
            })
            .unwrap();
    }

    #[test]
    fn replacement_preserves_permissions_ownership_and_newlines() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let directory = Directory::new();
                    let file = directory.file();
                    let path = file.path().unwrap();
                    for mode in [0o600, 0o640, 0o644] {
                        fs::write(&path, b"original\r\n").unwrap();
                        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
                        let before = fs::metadata(&path).unwrap();
                        let loaded = load(&file).await.unwrap();
                        let text = "\r\nselect 1;\rselect 2;\n\n  \r\n";
                        let etag = save(
                            &file,
                            text.into(),
                            false,
                            SaveTarget::Existing { etag: loaded.etag },
                        )
                        .await
                        .unwrap();
                        assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
                        assert_eq!(load(&file).await.unwrap().etag, etag);
                        let after = fs::metadata(&path).unwrap();
                        assert_eq!(after.mode() & 0o7777, mode);
                        assert_eq!(after.uid(), before.uid());
                        assert_eq!(after.gid(), before.gid());
                        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
                    }
                })
            })
            .unwrap();
    }

    #[test]
    fn native_atomic_replace_write_failure_preserves_original() {
        const CHILD_PATH: &str = "CODD_TEST_ATOMIC_SQL_PATH";
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            let context = glib::MainContext::new();
            context
                .with_thread_default(|| {
                    context.block_on(async {
                        let file = gio::File::for_path(&path);
                        let loaded = load(&file).await.unwrap();
                        let result = save(
                            &file,
                            "x".repeat(1024 * 1024),
                            false,
                            SaveTarget::Existing { etag: loaded.etag },
                        )
                        .await;
                        assert!(matches!(result, Err(FileError::Io(_))), "{result:?}");
                        assert_eq!(fs::read(path).unwrap(), b"last good SQL\r\n");
                    })
                })
                .unwrap();
            return;
        }
        let directory = Directory::new();
        let path = directory.file().path().unwrap();
        fs::write(&path, b"last good SQL\r\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let before = fs::metadata(&path).unwrap();
        let status = Command::new("sh")
            .args([
                "-c",
                "trap '' XFSZ; ulimit -f 1; exec \"$@\"",
                "codd-file-test",
            ])
            .arg(std::env::current_exe().unwrap())
            .args([
                "native_atomic_replace_write_failure_preserves_original",
                "--test-threads=1",
            ])
            .env(CHILD_PATH, &path)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(fs::read(&path).unwrap(), b"last good SQL\r\n");
        let after = fs::metadata(&path).unwrap();
        assert_eq!(after.ino(), before.ino());
        assert_eq!(after.mode(), before.mode());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn new_write_failure_never_publishes_partial_file() {
        const CHILD_PATH: &str = "CODD_TEST_ATOMIC_NEW_SQL_PATH";
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            let context = glib::MainContext::new();
            context
                .with_thread_default(|| {
                    context.block_on(async {
                        let file = gio::File::for_path(&path);
                        let result =
                            save(&file, "x".repeat(1024 * 1024), false, SaveTarget::New).await;
                        assert!(matches!(result, Err(FileError::Io(_))), "{result:?}");
                        assert!(!file.path().unwrap().exists());
                    })
                })
                .unwrap();
            assert!(!PathBuf::from(path).exists());
            return;
        }
        let directory = Directory::new();
        let path = directory.file().path().unwrap();
        let status = Command::new("sh")
            .args([
                "-c",
                "trap '' XFSZ; ulimit -f 1; exec \"$@\"",
                "codd-file-test",
            ])
            .arg(std::env::current_exe().unwrap())
            .args([
                "new_write_failure_never_publishes_partial_file",
                "--test-threads=1",
            ])
            .env(CHILD_PATH, &path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    #[test]
    fn unwritable_directory_save_preserves_existing_file() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let directory = Directory::new();
                    let file = directory.file();
                    let path = file.path().unwrap();
                    fs::write(&path, b"original").unwrap();
                    let etag = load(&file).await.unwrap().etag;
                    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o500)).unwrap();
                    let result = save(
                        &file,
                        "replacement".into(),
                        false,
                        SaveTarget::Existing { etag },
                    )
                    .await;
                    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700)).unwrap();
                    if fs::metadata(&directory.0).unwrap().uid() == 0 {
                        result.unwrap();
                        assert_eq!(fs::read(path).unwrap(), b"replacement");
                    } else {
                        assert!(matches!(result, Err(FileError::Io(_))), "{result:?}");
                        assert_eq!(fs::read(path).unwrap(), b"original");
                    }
                })
            })
            .unwrap();
    }

    #[test]
    fn atomic_save_leaves_hard_linked_alias_unchanged() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let directory = Directory::new();
                    let file = directory.file();
                    let path = file.path().unwrap();
                    let alias = directory.0.join("alias.sql");
                    fs::write(&path, b"original").unwrap();
                    fs::hard_link(&path, &alias).unwrap();
                    let etag = load(&file).await.unwrap().etag;
                    save(
                        &file,
                        "replacement".into(),
                        false,
                        SaveTarget::Existing { etag },
                    )
                    .await
                    .unwrap();
                    assert_eq!(fs::read(path).unwrap(), b"replacement");
                    assert_eq!(fs::read(alias).unwrap(), b"original");
                })
            })
            .unwrap();
    }

    #[test]
    fn missing_etags_allow_unverified_reads_but_known_changes_conflict() {
        let first = gio::FileInfo::new();
        let second = gio::FileInfo::new();
        assert_eq!(snapshot_etag(&[&first, &second]).unwrap(), None);
        first.set_attribute_string("etag::value", "version");
        assert_eq!(snapshot_etag(&[&first, &second]).unwrap(), None);
        second.set_attribute_string("etag::value", "version");
        assert_eq!(
            snapshot_etag(&[&first, &second]).unwrap().as_deref(),
            Some("version")
        );
        second.set_attribute_string("etag::value", "changed");
        assert!(matches!(
            snapshot_etag(&[&first, &second]),
            Err(FileError::Conflict)
        ));
    }

    #[test]
    fn symlinked_parent_directory_is_allowed() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let directory = Directory::new();
                    let actual = directory.0.join("actual");
                    fs::create_dir(&actual).unwrap();
                    let alias = directory.0.join("alias");
                    std::os::unix::fs::symlink(&actual, &alias).unwrap();
                    let file = gio::File::for_path(alias.join("query.sql"));
                    let etag = save(&file, "original".into(), false, SaveTarget::New)
                        .await
                        .unwrap();
                    assert_eq!(load(&file).await.unwrap().text, "original");
                    save(
                        &file,
                        "replacement".into(),
                        false,
                        SaveTarget::Existing { etag },
                    )
                    .await
                    .unwrap();
                    assert_eq!(fs::read(actual.join("query.sql")).unwrap(), b"replacement");
                })
            })
            .unwrap();
    }

    #[test]
    fn access_acl_is_preserved_and_inherited_acl_cannot_widen_access() {
        fn acl(attribute: &str) -> gio::FileInfo {
            let mut bytes = 2_u32.to_le_bytes().to_vec();
            for (tag, permissions, id) in [
                (1_u16, 6_u16, u32::MAX),
                (2, 4, 65534),
                (4, 0, u32::MAX),
                (16, 4, u32::MAX),
                (32, 0, u32::MAX),
            ] {
                bytes.extend_from_slice(&tag.to_le_bytes());
                bytes.extend_from_slice(&permissions.to_le_bytes());
                bytes.extend_from_slice(&id.to_le_bytes());
            }
            let encoded = bytes
                .iter()
                .map(|byte| format!("\\x{byte:02x}"))
                .collect::<String>();
            let info = gio::FileInfo::new();
            info.set_attribute_string(attribute, &encoded);

            info
        }

        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let directory = Directory::new();
                    let file = directory.file();
                    fs::write(file.path().unwrap(), b"original").unwrap();
                    file.set_attributes_future(
                        &acl(POSIX_ACL),
                        gio::FileQueryInfoFlags::NONE,
                        glib::Priority::DEFAULT,
                    )
                    .await
                    .unwrap();
                    let before = file_info(&file).await.unwrap();
                    assert!(before.attribute_string(POSIX_ACL).is_some());
                    let etag = load(&file).await.unwrap().etag;
                    save(
                        &file,
                        "replacement".into(),
                        false,
                        SaveTarget::Existing { etag },
                    )
                    .await
                    .unwrap();
                    let after = file_info(&file).await.unwrap();
                    assert_eq!(
                        after.attribute_string(POSIX_ACL),
                        before.attribute_string(POSIX_ACL)
                    );
                    assert_eq!(
                        after.attribute_uint32("unix::mode"),
                        before.attribute_uint32("unix::mode")
                    );

                    let plain_path = directory.0.join("plain.sql");
                    fs::write(&plain_path, b"unchanged").unwrap();
                    let plain = gio::File::for_path(&plain_path);
                    let etag = load(&plain).await.unwrap().etag;
                    let parent = gio::File::for_path(&directory.0);
                    parent
                        .set_attributes_future(
                            &acl("xattr-sys::system.posix_acl_default"),
                            gio::FileQueryInfoFlags::NONE,
                            glib::Priority::DEFAULT,
                        )
                        .await
                        .unwrap();
                    let result = save(
                        &plain,
                        "replacement".into(),
                        false,
                        SaveTarget::Existing { etag },
                    )
                    .await;
                    assert!(matches!(result, Err(FileError::Unsupported)), "{result:?}");
                    assert_eq!(fs::read(plain_path).unwrap(), b"unchanged");
                    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 2);
                })
            })
            .unwrap();
    }
}
