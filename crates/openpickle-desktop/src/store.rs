//! What the app keeps on disk, under the user's application-support directory:
//!
//! - `session.json`: the login session (a credential, readable by the user only);
//! - `cache/`: copies of what the server sent, so the next launch can draw before the network
//!   answers. Deleting it loses nothing.

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The current refresh token. Every refresh replaces it.
    pub login_session_token: String,
    /// The id this installation reports as its device.
    pub device_id: String,
}

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The store in the platform's application-support directory.
    pub fn open() -> io::Result<Self> {
        let base =
            dirs::data_dir().ok_or_else(|| io::Error::other("no application-support directory"))?;
        Self::at(base.join("openpickle"))
    }

    pub fn at(root: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(root.join("cache"))?;
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

    pub fn session(&self) -> Option<Session> {
        self.read_json(&self.root.join("session.json"))
    }

    pub fn save_session(&self, session: &Session) -> io::Result<()> {
        let path = self.root.join("session.json");
        write_atomically(&path, &serde_json::to_vec(session)?, true)
    }

    /// Forgets the login session and everything cached for it. The next person to sign in on
    /// this computer account must not be shown the previous conversations.
    pub fn sign_out(&self) -> io::Result<()> {
        match fs::remove_file(self.root.join("session.json")) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        fs::remove_dir_all(self.root.join("cache"))?;
        Self::at(self.root.clone()).map(drop)
    }

    /// A cached value, or `None` when it is missing or unreadable.
    pub fn cached<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        self.read_json(&self.cache_path(name))
    }

    pub fn cache<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        write_atomically(&self.cache_path(name), &serde_json::to_vec(value)?, false)
    }

    /// Every record of an append-only log, oldest first. A line cut short by a crash is skipped.
    pub fn log<T: DeserializeOwned>(&self, name: &str) -> Vec<T> {
        let Ok(file) = File::open(self.cache_path(name)) else {
            return Vec::new();
        };
        BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str(&line).ok())
            .collect()
    }

    pub fn append_log<T: Serialize>(&self, name: &str, records: &[T]) -> io::Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
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

    /// Where a cached file (an avatar image, say) lives.
    pub fn cache_path(&self, name: &str) -> PathBuf {
        self.root.join("cache").join(name)
    }

    fn read_json<T: DeserializeOwned>(&self, path: &Path) -> Option<T> {
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }
}

/// Writes through a temporary file and a rename, so a crash leaves the old or the new content,
/// never half of one. That matters most for the session: a lost refresh token ends the login.
fn write_atomically(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
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

    #[test]
    fn session_round_trips_and_is_private() {
        let store = store("session");
        assert_eq!(store.session(), None);
        let session = Session {
            login_session_token: "ls_test".into(),
            device_id: "device".into(),
        };
        store.save_session(&session).unwrap();
        assert_eq!(store.session(), Some(session));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.root.join("session.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
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
        store.sign_out().unwrap();
        assert_eq!(store.session(), None);
        assert_eq!(store.cached::<String>("room.json"), None);
        assert!(store.log::<u32>("room.jsonl").is_empty());
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
