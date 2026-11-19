//! What the app keeps on disk, under the user's application-support directory:
//!
//! - `session.json`: the login session (a credential, readable by the user only);
//! - `cache/`: copies of what the server sent, so the next launch can draw before the network
//!   answers. Deleting it loses nothing.

use std::{
    fmt,
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The current refresh token. Every refresh replaces it.
    pub login_session_token: String,
    /// The id this installation reports as its device.
    pub device_id: String,
}

/// Leaves the token out, so a log line or a failed assertion cannot carry it.
impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogStatus {
    Clean,
    TornTail,
    Corrupt,
}

impl Store {
    /// The store in the platform's application-support directory.
    pub fn open() -> io::Result<Self> {
        let base =
            dirs::data_dir().ok_or_else(|| io::Error::other("no application-support directory"))?;
        Self::at(base.join("openpickle"))
    }

    pub fn at(root: PathBuf) -> io::Result<Self> {
        // Everything here is private: a credential and copies of the user's conversations.
        for directory in [root.clone(), root.join("cache")] {
            fs::create_dir_all(&directory)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(Self { root })
    }

    /// Claims the store for this process; `None` means another instance holds it. Two
    /// instances would spend the same single-use refresh token, and the slower one would be
    /// signed out. The claim lasts as long as the returned file is kept.
    pub fn claim(&self) -> io::Result<Option<File>> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(self.root.join("lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    /// Missing is normal. A malformed credential is not usable, and other read errors must
    /// not quietly turn into a new login attempt. Never include the credential bytes in errors.
    pub fn session(&self) -> io::Result<Option<Session>> {
        let bytes = match fs::read(self.root.join("session.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid local login session");
        let session: Session = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if session.login_session_token.trim().is_empty() || session.device_id.trim().is_empty() {
            return Err(invalid());
        }
        Ok(Some(session))
    }

    /// A newly issued session may be for a different account. Establish an empty cache before
    /// persisting it, so even a restart cannot open the old account's conversations under it.
    pub fn start_session(&self, session: &Session) -> io::Result<()> {
        self.sign_out()?;
        self.save_session(session)
    }

    /// Persists a replacement credential for the same account, preserving its cache.
    pub fn save_session(&self, session: &Session) -> io::Result<()> {
        write_atomically(
            &self.root.join("session.json"),
            &serde_json::to_vec(session)?,
        )
    }

    /// Forgets the login session and everything cached for it. The next person to sign in on
    /// this computer account must not be shown the previous conversations.
    pub fn sign_out(&self) -> io::Result<()> {
        let credential = self.root.join("session.json");
        remove_if_present(fs::remove_file(&credential))?;
        // A failed atomic write can have left a second copy of the credential.
        remove_if_present(fs::remove_file(credential.with_extension("tmp")))?;
        remove_if_present(fs::remove_dir_all(self.root.join("cache")))?;
        Self::at(self.root.clone()).map(drop)
    }

    /// A cached value, or `None` when it is missing or unreadable.
    pub fn cached<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        self.read_json(&self.cache_path(name))
    }

    pub fn cache<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        write_atomically(&self.cache_path(name), &serde_json::to_vec(value)?)
    }

    /// Visits valid records in order. The visitor's second argument is true after a
    /// malformed, newline-terminated record. A malformed unterminated tail has no later
    /// records to invalidate, but must be removed before appending.
    pub(crate) fn read_log<T: DeserializeOwned>(
        &self,
        name: &str,
        mut visit: impl FnMut(T, bool),
    ) -> io::Result<LogStatus> {
        let file = match File::open(self.cache_path(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(LogStatus::Clean),
            Err(error) => return Err(error),
        };
        scan_log(file, |record, _, after_gap| {
            visit(record, after_gap);
            Ok(())
        })
    }

    /// Removes unreadable lines and records rejected by `keep`. Retained lines are copied
    /// as-is so repair does not strip fields unknown to this version of the app.
    pub(crate) fn repair_log<T: DeserializeOwned>(
        &self,
        name: &str,
        mut keep: impl FnMut(&T, bool) -> bool,
    ) -> io::Result<()> {
        let path = self.cache_path(name);
        let source = File::open(&path)?;
        let temporary = path.with_extension("repair");
        let mut repaired = BufWriter::new(
            private(OpenOptions::new().write(true).create(true).truncate(true)).open(&temporary)?,
        );
        let result = (|| {
            scan_log(source, |record, line, after_gap| {
                if keep(&record, after_gap) {
                    repaired.write_all(line)?;
                }
                Ok(())
            })?;
            repaired.flush()?;
            repaired.get_ref().sync_all()
        })();
        drop(repaired);
        let result = result.and_then(|()| fs::rename(&temporary, &path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    #[cfg(test)]
    pub fn log<T: DeserializeOwned>(&self, name: &str) -> Vec<T> {
        let mut records = Vec::new();
        self.read_log(name, |record, _| records.push(record))
            .unwrap();
        records
    }

    pub fn append_log<T: Serialize>(&self, name: &str, records: &[T]) -> io::Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let mut file = private(OpenOptions::new().create(true).read(true).append(true))
            .open(self.cache_path(name))?;
        let mut lines = Vec::new();
        // A crash can leave the last line unfinished. Starting on a fresh line keeps that
        // fragment from swallowing the first new record.
        if file.metadata()?.len() > 0 {
            let mut last = [0];
            file.seek(SeekFrom::End(-1))?;
            file.read_exact(&mut last)?;
            if last != *b"\n" {
                lines.push(b'\n');
            }
        }
        for record in records {
            serde_json::to_writer(&mut lines, record)?;
            lines.push(b'\n');
        }
        file.write_all(&lines)
    }

    /// Writes a cached file as it is, such as a downloaded picture.
    pub fn cache_bytes(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        write_atomically(&self.cache_path(name), bytes)
    }

    /// Where a cached file (an avatar image, say) lives.
    pub fn cache_path(&self, name: &str) -> PathBuf {
        self.root.join("cache").join(name)
    }

    fn read_json<T: DeserializeOwned>(&self, path: &Path) -> Option<T> {
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }
}

fn scan_log<T: DeserializeOwned>(
    file: File,
    mut visit: impl FnMut(T, &[u8], bool) -> io::Result<()>,
) -> io::Result<LogStatus> {
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut status = LogStatus::Clean;
    // Read bytes, not text: invalid UTF-8 in one row must not hide later rows.
    while reader.read_until(b'\n', &mut line)? != 0 {
        match serde_json::from_slice(&line) {
            Ok(record) => visit(record, &line, status == LogStatus::Corrupt)?,
            Err(_) if line.ends_with(b"\n") => status = LogStatus::Corrupt,
            Err(_) if status == LogStatus::Clean => status = LogStatus::TornTail,
            Err(_) => {}
        }
        line.clear();
    }
    Ok(status)
}

fn remove_if_present(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Files are readable by their owner only.
fn private(options: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Writes through a temporary file and a rename, so a crash leaves the old or the new content,
/// never half of one. That matters most for the session: a lost refresh token ends the login.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file =
        private(OpenOptions::new().write(true).create(true).truncate(true)).open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Store {
        let root =
            std::env::temp_dir().join(format!("openpickle-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        Store::at(root).unwrap()
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn session_round_trips() {
        let store = store("session");
        assert_eq!(store.session().unwrap(), None);
        let session = Session {
            login_session_token: "ls_test".into(),
            device_id: "device".into(),
        };
        store.save_session(&session).unwrap();
        assert_eq!(store.session().unwrap(), Some(session.clone()));
        let replacement = Session {
            login_session_token: "ls_next".into(),
            device_id: "device".into(),
        };
        store.save_session(&replacement).unwrap();
        assert_eq!(store.session().unwrap(), Some(replacement));
        assert!(!format!("{session:?}").contains("ls_test"));
    }

    #[test]
    fn local_login_distinguishes_missing_malformed_and_read_errors() {
        let store = store("session-errors");
        let path = store.root.join("session.json");
        assert_eq!(store.session().unwrap(), None);
        for bytes in [
            b"synthetic-malformed-credential".as_slice(),
            br#"{"login_session_token":" ","device_id":"fixture"}"#,
        ] {
            fs::write(&path, bytes).unwrap();
            let error = store.session().unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(error.to_string(), "invalid local login session");
        }
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let error = store.session().unwrap_err();
        assert_ne!(error.kind(), io::ErrorKind::NotFound);
        assert_ne!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn fresh_login_is_not_saved_when_the_old_cache_cannot_be_cleared() {
        let store = store("blocked-cache-clear");
        let path = store.root.join("cache");
        fs::remove_dir(&path).unwrap();
        fs::write(&path, b"not a directory").unwrap();
        let session = Session {
            login_session_token: "fixture-new-user".into(),
            device_id: "fixture-device".into(),
        };
        assert!(store.start_session(&session).is_err());
        assert_eq!(store.session().unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_readable_by_other_users() {
        let store = store("private");
        store
            .save_session(&Session {
                login_session_token: "ls_test".into(),
                device_id: "device".into(),
            })
            .unwrap();
        store.cache("room.json", &"room").unwrap();
        store.append_log("room.jsonl", &[1]).unwrap();
        assert_eq!(mode(&store.root), 0o700);
        assert_eq!(mode(&store.root.join("cache")), 0o700);
        for file in [
            store.root.join("session.json"),
            store.cache_path("room.json"),
            store.cache_path("room.jsonl"),
        ] {
            assert_eq!(mode(&file), 0o600, "{}", file.display());
        }
    }

    #[test]
    fn a_line_that_is_not_text_does_not_hide_the_records_after_it() {
        let store = store("log-bytes");
        store.append_log("room.jsonl", &[1, 2]).unwrap();
        // A crash in the middle of a character, or a damaged block, leaves bytes that are
        // not UTF-8.
        let path = store.cache_path("room.jsonl");
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"\xff\xfe{\"cut\n");
        fs::write(&path, bytes).unwrap();
        store.append_log("room.jsonl", &[3]).unwrap();
        let mut records = Vec::new();
        let status = store
            .read_log::<i32>("room.jsonl", |record, after_gap| {
                records.push((record, after_gap));
            })
            .unwrap();
        assert_eq!(status, LogStatus::Corrupt);
        assert_eq!(records, [(1, false), (2, false), (3, true)]);
    }

    #[test]
    fn signing_out_forgets_the_session_and_the_cache() {
        let store = store("sign-out");
        store
            .save_session(&Session {
                login_session_token: "ls_test".into(),
                device_id: "device".into(),
            })
            .unwrap();
        store.cache("room.json", &"room").unwrap();
        store.append_log("room.jsonl", &[1]).unwrap();
        let scratch = store.root.join("session.tmp");
        fs::write(&scratch, b"fixture refresh scratch").unwrap();
        store.sign_out().unwrap();
        assert_eq!(store.session().unwrap(), None);
        assert!(!scratch.exists());
        assert_eq!(store.cached::<String>("room.json"), None);
        assert!(store.log::<u32>("room.jsonl").is_empty());
        // A repeated cleanup also works after a partial removal.
        fs::remove_dir(store.root.join("cache")).unwrap();
        store.sign_out().unwrap();
        // The store is still usable for the next sign-in.
        store.cache("room.json", &"next").unwrap();
    }

    #[test]
    fn only_one_claim_at_a_time() {
        let store = store("claim");
        let first = store.claim().unwrap();
        assert!(first.is_some());
        assert!(store.claim().unwrap().is_none());
        drop(first);
        assert!(store.claim().unwrap().is_some());
    }

    #[test]
    fn a_torn_last_line_costs_only_itself() {
        let store = store("log");
        store.append_log("room.jsonl", &[1, 2]).unwrap();
        store.append_log("room.jsonl", &[3]).unwrap();
        // A crash in the middle of a write leaves an unfinished line.
        fs::OpenOptions::new()
            .append(true)
            .open(store.cache_path("room.jsonl"))
            .unwrap()
            .write_all(b"{\"cut")
            .unwrap();
        assert_eq!(store.log::<u32>("room.jsonl"), vec![1, 2, 3]);
        // What is appended next must not be joined to the fragment and lost with it.
        store.append_log("room.jsonl", &[4, 5]).unwrap();
        assert_eq!(store.log::<u32>("room.jsonl"), vec![1, 2, 3, 4, 5]);
    }
}
