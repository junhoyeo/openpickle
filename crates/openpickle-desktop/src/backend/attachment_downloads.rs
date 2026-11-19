//! Original downloads run beside the conversation and publish only complete files.

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures::channel::mpsc::UnboundedSender as Updates;
use openpickle_api::PickleClient;
use tokio::{
    sync::{Semaphore, mpsc, watch},
    task::JoinHandle,
    time::Instant,
};

use crate::protocol::Update;

const ACTIVE_LIMIT: usize = 2;
// Includes active jobs; the rest wait for a permit without opening connections or files.
const JOB_LIMIT: usize = 10;
// Limit decoded bytes written to disk, including when the server does not send a length.
const SIZE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const SIZE_ERROR: &str = "The file exceeds the 2 GiB download limit.";
const CANCELED: &str = "Download canceled.";

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct DownloadKey {
    pub room_id: String,
    pub path: String,
}

impl DownloadKey {
    pub(super) fn finished(self, updates: &Updates<Update>, result: Result<PathBuf, String>) {
        updates
            .unbounded_send(Update::AttachmentDownloaded {
                room_id: self.room_id,
                path: self.path,
                result,
            })
            .ok();
    }

    fn progress(&self, updates: &Updates<Update>, (received, total): Progress) {
        updates
            .unbounded_send(Update::AttachmentDownloadProgress {
                room_id: self.room_id.clone(),
                path: self.path.clone(),
                received,
                total,
            })
            .ok();
    }
}

type Progress = (u64, Option<u64>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Pending,
    Canceled,
    Publishing,
}

/// This transition decides whether cancellation or publication wins. Neither holds the lock
/// across I/O; account teardown also joins the jobs before letting the next account start.
fn begin(phase: &watch::Sender<Phase>, next: Phase) -> bool {
    phase.send_if_modified(|phase| {
        if *phase != Phase::Pending {
            return false;
        }
        *phase = next;
        true
    })
}

struct Job {
    phase: watch::Sender<Phase>,
    progress: watch::Receiver<Progress>,
    task: JoinHandle<()>,
}

pub(super) struct OriginalDownloads {
    folder: PathBuf,
    account_active: Arc<AtomicBool>,
    slots: Arc<Semaphore>,
    jobs: HashMap<DownloadKey, Job>,
}

impl OriginalDownloads {
    pub(super) fn new(folder: PathBuf, account_active: Arc<AtomicBool>) -> Self {
        Self {
            folder,
            account_active,
            slots: Arc::new(Semaphore::new(ACTIVE_LIMIT)),
            jobs: HashMap::new(),
        }
    }

    pub(super) fn start(
        &mut self,
        key: DownloadKey,
        client: PickleClient,
        name: String,
        updates: Updates<Update>,
    ) {
        self.jobs.retain(|_, job| !job.task.is_finished());
        if let Some(job) = self.jobs.get(&key) {
            // Reopening a preview can attach to this transfer rather than creating a new file.
            let progress = *job.progress.borrow();
            key.progress(&updates, progress);
            return;
        }
        if self.jobs.len() >= JOB_LIMIT {
            key.finished(
                &updates,
                Err("Too many downloads are waiting. Try again shortly.".into()),
            );
            return;
        }
        let (phase, mut stopped) = watch::channel(Phase::Pending);
        let (latest, progress) = watch::channel((0, None));
        let reporter = Reporter {
            key: key.clone(),
            updates,
            active: self.account_active.clone(),
            latest,
        };
        reporter.progress(0, None);
        let (folder, slots, writer_phase) =
            (self.folder.clone(), self.slots.clone(), phase.clone());
        let task = tokio::spawn(async move {
            let result = match tokio::select! {
                biased;
                _ = canceled(&mut stopped) => None,
                slot = slots.acquire() => slot.ok(),
            } {
                Some(_slot) => {
                    save_download(&client, folder, name, writer_phase, stopped, &reporter).await
                }
                None => Err(CANCELED.into()),
            };
            if reporter.active.load(Ordering::Acquire) {
                reporter.key.finished(&reporter.updates, result);
            }
        });
        self.jobs.insert(
            key,
            Job {
                phase,
                progress,
                task,
            },
        );
    }

    pub(super) fn cancel(&self, key: &DownloadKey) {
        if let Some(job) = self.jobs.get(key) {
            begin(&job.phase, Phase::Canceled);
        }
    }

    fn cancel_all(&self) {
        for job in self.jobs.values() {
            begin(&job.phase, Phase::Canceled);
        }
    }

    pub(super) async fn shutdown(&mut self) {
        self.cancel_all();
        for (_, job) in self.jobs.drain() {
            job.task.await.ok();
        }
    }
}

impl Drop for OriginalDownloads {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

struct Reporter {
    key: DownloadKey,
    updates: Updates<Update>,
    active: Arc<AtomicBool>,
    latest: watch::Sender<Progress>,
}

impl Reporter {
    fn progress(&self, received: u64, total: Option<u64>) {
        if self.active.load(Ordering::Acquire) {
            self.latest.send_replace((received, total));
            self.key.progress(&self.updates, (received, total));
        }
    }
}

async fn canceled(phase: &mut watch::Receiver<Phase>) {
    phase.wait_for(|phase| *phase == Phase::Canceled).await.ok();
}

async fn save_download(
    client: &PickleClient,
    folder: PathBuf,
    name: String,
    phase: watch::Sender<Phase>,
    mut stopped: watch::Receiver<Phase>,
    reporter: &Reporter,
) -> Result<PathBuf, String> {
    let mut response = tokio::select! {
        biased;
        _ = canceled(&mut stopped) => return Err(CANCELED.into()),
        response = client.get_attachment_response(&reporter.key.path) => response.map_err(|error| super::explain(&error))?,
    };
    // The desktop requested the whole file, not a byte range. Do not publish a partial reply.
    if response.status() != reqwest::StatusCode::OK
        || response
            .headers()
            .contains_key(reqwest::header::CONTENT_RANGE)
    {
        return Err("The server did not return the entire attachment.".into());
    }
    let total = response.content_length();
    if total.is_some_and(|total| total > SIZE_LIMIT) {
        return Err(SIZE_ERROR.into());
    }
    reporter.progress(0, total);
    // Keep only a few chunks in flight; disk backpressure stops reading the network body.
    let (chunks, pending) = mpsc::channel(2);
    let mut writer =
        tokio::task::spawn_blocking(move || write_download(&folder, &name, pending, &phase));
    let receive = async {
        let mut received = 0;
        let mut last_report: Option<Instant> = None;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| super::explain(&error.into()))?
        {
            received += chunk.len() as u64;
            if received > SIZE_LIMIT {
                return Err(SIZE_ERROR.to_owned());
            }
            chunks
                .send(Some(chunk))
                .await
                .map_err(|_| "Could not write the download.".to_owned())?;
            if last_report.is_none_or(|at| at.elapsed() >= Duration::from_millis(250)) {
                reporter.progress(received, total);
                last_report = Some(Instant::now());
            }
        }
        if total.is_some_and(|total| received != total) {
            return Err("The attachment ended before all bytes were received.".into());
        }
        reporter.progress(received, total);
        chunks
            .send(None)
            .await
            .map_err(|_| "Could not write the download.".to_owned())
    };
    let (received, saved) = tokio::select! {
        biased;
        _ = canceled(&mut stopped) => (Err(CANCELED.into()), None),
        result = &mut writer => (Ok(()), Some(result)),
        result = receive => (result, None),
    };
    drop(response);
    drop(chunks);
    // Do not report failure/cancellation until the writer has removed the partial. The writer
    // may also fail early (for example on permissions), without waiting for another body chunk.
    let saved = match saved {
        Some(result) => result,
        None => writer.await,
    }
    .map_err(|error| format!("Could not finish the download: {error}"))?
    .map_err(|error| format!("Could not save the file: {error}"))?;
    match (saved, received) {
        (Some(file), _) => Ok(file),
        (None, Err(reason)) => Err(reason),
        (None, Ok(())) => Err(CANCELED.into()),
    }
}

struct PartialFile(PathBuf);

impl Drop for PartialFile {
    fn drop(&mut self) {
        fs::remove_file(&self.0).ok();
    }
}

fn write_download<B: AsRef<[u8]>>(
    folder: &Path,
    name: &str,
    mut chunks: mpsc::Receiver<Option<B>>,
    phase: &watch::Sender<Phase>,
) -> io::Result<Option<PathBuf>> {
    if *phase.borrow() == Phase::Canceled {
        return Ok(None);
    }
    fs::create_dir_all(folder)?;
    let temporary = folder.join(format!(".openpickle-{}.part", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(&temporary)?;
    let partial = PartialFile(temporary);
    // Move the file in so it is closed before the cleanup guard runs, even after write errors.
    let write = move || -> io::Result<bool> {
        let mut file = file;
        while let Some(next) = chunks.blocking_recv() {
            if *phase.borrow() == Phase::Canceled {
                return Ok(false);
            }
            match next {
                Some(bytes) => file.write_all(bytes.as_ref())?,
                // Only a verified end of body sends this marker. A closed channel is not EOF.
                None => {
                    file.sync_all()?;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    };
    if !write()? || !begin(phase, Phase::Publishing) {
        return Ok(None);
    }
    publish(folder, name, &partial.0).map(Some)
}

/// A hard link in the same directory exposes the complete file and fails atomically if the
/// chosen name already exists; existence checks followed by rename could overwrite another file.
fn publish(folder: &Path, name: &str, temporary: &Path) -> io::Result<PathBuf> {
    let name: String = Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment")
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name.as_str(), ""),
    };
    for count in 0.. {
        let file = if count == 0 {
            folder.join(&name)
        } else {
            folder.join(format!("{stem} ({count}){extension}"))
        };
        match fs::hard_link(temporary, &file) {
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    unreachable!("the count has no end")
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;
    use openpickle_api::{ClientOptions, Credentials};
    use std::{
        io::{BufRead as _, BufReader},
        net::{SocketAddr, TcpListener, TcpStream},
        sync::mpsc as gate,
        thread,
    };

    const BODY_BYTES: usize = 2 * 1024 * 1024;
    const CHUNK_BYTES: usize = 64 * 1024;
    type Received = futures::channel::mpsc::UnboundedReceiver<Update>;

    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("openpickle-download-test-{}", uuid::Uuid::new_v4())),
            )
        }

        fn contents(&self) -> Vec<PathBuf> {
            fs::read_dir(&self.0)
                .into_iter()
                .flatten()
                .map(|entry| entry.unwrap().path())
                .collect()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    struct Server {
        requests: mpsc::UnboundedReceiver<gate::Sender<()>>,
        address: SocketAddr,
        active: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Server {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let active = Arc::new(AtomicBool::new(true));
            let serving = active.clone();
            let (send, requests) = mpsc::unbounded_channel();
            let join = thread::spawn(move || {
                for incoming in listener.incoming() {
                    if !serving.load(Ordering::Acquire) {
                        break;
                    }
                    let send = send.clone();
                    let stream = incoming.unwrap();
                    thread::spawn(move || respond(stream, send).ok());
                }
            });
            Self {
                requests,
                address,
                active,
                thread: Some(join),
            }
        }

        fn client(&self) -> PickleClient {
            PickleClient::new(ClientOptions {
                base_url: Some(format!("http://{}", self.address)),
                credentials: Credentials::AccessToken("fixture".into()),
                ..Default::default()
            })
            .unwrap()
        }

        async fn request(&mut self) -> gate::Sender<()> {
            tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
                .await
                .unwrap()
                .unwrap()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.active.store(false, Ordering::Release);
            TcpStream::connect(self.address).ok();
            self.thread.take().unwrap().join().unwrap();
        }
    }

    fn respond(
        mut stream: TcpStream,
        requests: mpsc::UnboundedSender<gate::Sender<()>>,
    ) -> io::Result<()> {
        let target = {
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line)?;
            let target = line.split_whitespace().nth(1).unwrap().to_owned();
            line.clear();
            while reader.read_line(&mut line)? != 0 && line != "\r\n" {
                line.clear();
            }
            target
        };
        let (release, wait) = gate::channel();
        requests.send(release).ok();
        if target.ends_with("/oversize") {
            return write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                SIZE_LIMIT + 1
            );
        }
        if target.ends_with("/range") {
            return write!(
                stream,
                "HTTP/1.1 206 Partial Content\r\nContent-Length: 1\r\nContent-Range: bytes 0-0/9\r\nConnection: close\r\n\r\na"
            );
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {BODY_BYTES}\r\nConnection: close\r\n\r\n"
        )?;
        let chunk = [42; CHUNK_BYTES];
        stream.write_all(&chunk)?;
        if wait.recv_timeout(Duration::from_secs(5)).is_ok() && !target.ends_with("/truncated") {
            for _ in 1..(BODY_BYTES / CHUNK_BYTES) {
                stream.write_all(&chunk)?;
            }
        }
        Ok(())
    }

    fn key(file: &str) -> DownloadKey {
        DownloadKey {
            room_id: "fixture".into(),
            path: format!("attachments/fixture/{file}"),
        }
    }

    async fn next(received: &mut Received) -> Update {
        tokio::time::timeout(Duration::from_secs(5), received.next())
            .await
            .unwrap()
            .unwrap()
    }

    async fn started(received: &mut Received, count: usize) {
        let mut paths = std::collections::HashSet::new();
        while paths.len() < count {
            match next(received).await {
                Update::AttachmentDownloadProgress {
                    path,
                    received,
                    total,
                    ..
                } if received > 0 => {
                    assert_eq!(total, Some(BODY_BYTES as u64));
                    paths.insert(path);
                }
                Update::AttachmentDownloaded { result, .. } => {
                    panic!("ended while waiting: {result:?}")
                }
                _ => {}
            }
        }
    }

    async fn finished(received: &mut Received) -> (String, Result<PathBuf, String>) {
        loop {
            if let Update::AttachmentDownloaded { path, result, .. } = next(received).await {
                return (path, result);
            }
        }
    }

    #[test]
    fn publish_preserves_the_safe_basename_and_never_replaces_an_existing_path() {
        let folder = Folder::new();
        fs::create_dir_all(&folder.0).unwrap();
        let temporary = folder.0.join("staged");
        fs::write(&temporary, b"complete").unwrap();
        let existing = folder.0.join("report.pdf");
        fs::write(&existing, b"original").unwrap();
        let published = publish(&folder.0, "../../report.pdf", &temporary).unwrap();
        assert_eq!(published, folder.0.join("report (1).pdf"));
        assert_eq!(fs::read(published).unwrap(), b"complete");
        assert_eq!(fs::read(existing).unwrap(), b"original");
        assert_eq!(
            publish(&folder.0, "..", &temporary).unwrap(),
            folder.0.join("attachment")
        );
    }

    #[tokio::test]
    async fn two_large_downloads_stream_without_publishing_before_eof_and_a_waiting_job_can_cancel()
    {
        let folder = Folder::new();
        let mut server = Server::start();
        let (updates, mut received) = futures::channel::mpsc::unbounded();
        let mut downloads =
            OriginalDownloads::new(folder.0.clone(), Arc::new(AtomicBool::new(true)));
        for file in ["one", "two", "waiting"] {
            downloads.start(
                key(file),
                server.client(),
                "report.pdf".into(),
                updates.clone(),
            );
        }
        let release_one = server.request().await;
        let release_two = server.request().await;
        started(&mut received, 2).await;
        assert!(server.requests.try_recv().is_err());
        assert!(!folder.0.join("report.pdf").exists());
        assert!(!folder.0.join("report (1).pdf").exists());

        // Dedup does not take a slot; a queued job can finish canceling while both run.
        downloads.start(
            key("one"),
            server.client(),
            "report.pdf".into(),
            updates.clone(),
        );
        assert_eq!(downloads.jobs.len(), 3);
        for index in 3..JOB_LIMIT {
            let file = format!("waiting-{index}");
            downloads.start(
                key(&file),
                server.client(),
                "report.pdf".into(),
                updates.clone(),
            );
        }
        downloads.start(
            key("over-limit"),
            server.client(),
            "report.pdf".into(),
            updates,
        );
        let (path, result) = finished(&mut received).await;
        assert_eq!(path, key("over-limit").path);
        assert!(result.unwrap_err().contains("Too many downloads"));
        for index in 3..JOB_LIMIT {
            downloads.cancel(&key(&format!("waiting-{index}")));
        }
        downloads.cancel(&key("waiting"));
        for _ in 0..(JOB_LIMIT - ACTIVE_LIMIT) {
            let (path, result) = finished(&mut received).await;
            assert!(path.starts_with(&key("waiting").path));
            assert_eq!(result.unwrap_err(), CANCELED);
        }
        release_one.send(()).unwrap();
        release_two.send(()).unwrap();
        for _ in 0..2 {
            let (_, result) = finished(&mut received).await;
            let saved = result.unwrap();
            assert_eq!(fs::read(saved).unwrap(), vec![42; BODY_BYTES]);
        }
        let mut contents = folder.contents();
        contents.sort();
        assert_eq!(
            contents,
            [folder.0.join("report (1).pdf"), folder.0.join("report.pdf")]
        );
        assert!(server.requests.try_recv().is_err());
    }

    #[tokio::test]
    async fn stream_failure_cancel_and_sign_out_leave_no_partial_or_visible_file() {
        let folder = Folder::new();
        let mut server = Server::start();
        let (updates, mut received) = futures::channel::mpsc::unbounded();
        let active = Arc::new(AtomicBool::new(true));
        let mut downloads = OriginalDownloads::new(folder.0.clone(), active.clone());
        for file in ["truncated", "canceled", "shutdown"] {
            downloads.start(
                key(file),
                server.client(),
                "report.pdf".into(),
                updates.clone(),
            );
            let release = server.request().await;
            started(&mut received, 1).await;
            match file {
                "truncated" => {
                    release.send(()).unwrap();
                    let (_, result) = finished(&mut received).await;
                    assert!(result.is_err());
                }
                "canceled" => {
                    downloads.cancel(&key(file));
                    let (_, result) = finished(&mut received).await;
                    assert_eq!(result.unwrap_err(), CANCELED);
                }
                _ => {
                    active.store(false, Ordering::Release);
                    downloads.shutdown().await;
                    while let Ok(update) = received.try_recv() {
                        assert!(!matches!(update, Update::AttachmentDownloaded { .. }));
                    }
                }
            }
            assert!(folder.contents().is_empty());
        }
    }

    #[tokio::test]
    async fn rejects_oversized_and_unrequested_partial_bodies_before_staging() {
        let folder = Folder::new();
        let server = Server::start();
        let (updates, mut received) = futures::channel::mpsc::unbounded();
        let mut downloads =
            OriginalDownloads::new(folder.0.clone(), Arc::new(AtomicBool::new(true)));
        for (file, reason) in [
            ("oversize", SIZE_ERROR),
            ("range", "The server did not return the entire attachment."),
        ] {
            downloads.start(
                key(file),
                server.client(),
                "report.pdf".into(),
                updates.clone(),
            );
            let (_, result) = finished(&mut received).await;
            assert_eq!(result.unwrap_err(), reason);
        }
        assert!(!folder.0.exists());
    }
}
