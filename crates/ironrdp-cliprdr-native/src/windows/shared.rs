use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, mpsc};

use ironrdp_cliprdr::backend::{ClipboardMessage, ClipboardMessageProxy};
use ironrdp_cliprdr::pdu::{ClipboardGeneralCapabilityFlags, FileContentsRequest, FileContentsResponse};
use tracing::debug;

/// Clipboard state shared by the clipboard thread, the `CLIPRDR` backend (on the RDP thread) and
/// the streams serving remote files (on COM worker threads).
///
/// The backend updates it as soon as the channel reports a change, so that a stream reading a
/// file in chunks stops at once when the data it reads is no longer valid, and hands file
/// contents responses directly to the stream waiting for them: reading remote files never
/// involves the clipboard thread.
#[derive(Debug)]
pub(crate) struct ClipboardShared {
    message_proxy: Mutex<Box<dyn ClipboardMessageProxy>>,
    capabilities: AtomicU32,
    /// Bumped on every remote format list: data fetched by format id is only valid for the
    /// remote clipboard it was advertised with.
    generation: AtomicU64,
    /// Bumped on every new `CLIPRDR` channel (connection or reconnection).
    session: AtomicU64,
    /// Outgoing locks `Cliprdr` has released in the current session.
    released_locks: Mutex<HashSet<u32>>,
    next_stream_id: AtomicU32,
    /// Streams waiting for a file contents response, by stream id.
    waiters: Mutex<HashMap<u32, mpsc::Sender<FileContentsResponse<'static>>>>,
    /// When the last file contents request went out, in [`crate::native_now_ms`] time; 0 before any.
    last_file_request_ms: AtomicU64,
}

/// A download from the remote counts as running until it has sent nothing for this long. Readers
/// such as Explorer pause briefly between files and between reads.
pub(crate) const DOWNLOAD_IDLE_MS: u64 = 2_000;

impl ClipboardShared {
    pub(crate) fn new(message_proxy: Box<dyn ClipboardMessageProxy>) -> Self {
        Self {
            message_proxy: Mutex::new(message_proxy),
            capabilities: AtomicU32::new(0),
            generation: AtomicU64::new(0),
            session: AtomicU64::new(0),
            released_locks: Mutex::new(HashSet::new()),
            next_stream_id: AtomicU32::new(1),
            waiters: Mutex::new(HashMap::new()),
            last_file_request_ms: AtomicU64::new(0),
        }
    }

    pub(crate) fn send(&self, message: ClipboardMessage) {
        self.message_proxy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send_clipboard_message(message);
    }

    pub(crate) fn capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::from_bits_retain(self.capabilities.load(Ordering::Acquire))
    }

    pub(crate) fn set_capabilities(&self, capabilities: ClipboardGeneralCapabilityFlags) {
        self.capabilities.store(capabilities.bits(), Ordering::Release);
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Invalidates data of the previous remote clipboard, returning the new generation.
    pub(crate) fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    pub(crate) fn session(&self) -> u64 {
        self.session.load(Ordering::Acquire)
    }

    /// Invalidates everything fetched or requested in the previous channel.
    pub(crate) fn start_session(&self) {
        self.session.fetch_add(1, Ordering::AcqRel);
        self.next_generation();
        self.set_capabilities(ClipboardGeneralCapabilityFlags::empty());
        self.lock_released_locks().clear();
        // The previous channel answers nothing more: its waiters fail at once.
        self.lock_waiters().clear();
        self.last_file_request_ms.store(0, Ordering::Release);
    }

    /// Whether files are being read from the remote: a request is unanswered, or one went out
    /// within [`DOWNLOAD_IDLE_MS`].
    ///
    /// Windows' `rdpclip` stops serving a download, lock or no lock, once the client announces a
    /// new clipboard, so a local copy is held back while this is true.
    pub(crate) fn downloading(&self) -> bool {
        self.downloading_at(crate::native_now_ms())
    }

    fn downloading_at(&self, now_ms: u64) -> bool {
        if !self.lock_waiters().is_empty() {
            return true;
        }

        let last = self.last_file_request_ms.load(Ordering::Acquire);
        last != 0 && now_ms.saturating_sub(last) < DOWNLOAD_IDLE_MS
    }

    /// Invalidates everything handed out, for good: the clipboard is going away, and streams or
    /// data objects that outlive it fail instead of waiting for answers.
    pub(crate) fn close(&self) {
        self.start_session();
    }

    pub(crate) fn release_locks(&self, clip_data_ids: impl IntoIterator<Item = u32>) {
        self.lock_released_locks().extend(clip_data_ids);
    }

    pub(crate) fn is_lock_released(&self, clip_data_id: u32) -> bool {
        self.lock_released_locks().contains(&clip_data_id)
    }

    fn lock_released_locks(&self) -> std::sync::MutexGuard<'_, HashSet<u32>> {
        self.released_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Sends `request` with a fresh stream id, returning where its response will arrive.
    ///
    /// `Cliprdr` answers every request it sent or rejected, synthesizing a failure on its own
    /// transfer timeout, so every waiter gets an answer. One nobody waits for anymore is dropped.
    pub(crate) fn request_file_contents(
        &self,
        mut request: FileContentsRequest,
    ) -> mpsc::Receiver<FileContentsResponse<'static>> {
        let stream_id = self.next_stream_id.fetch_add(1, Ordering::AcqRel);
        request.stream_id = stream_id;

        let (sender, receiver) = mpsc::channel();
        self.lock_waiters().insert(stream_id, sender);
        self.last_file_request_ms
            .store(crate::native_now_ms().max(1), Ordering::Release);

        self.send(ClipboardMessage::SendFileContentsRequest(request));

        receiver
    }

    /// Hands a file contents response to the stream waiting for it.
    pub(crate) fn deliver(&self, response: FileContentsResponse<'static>) {
        let waiter = self.lock_waiters().remove(&response.stream_id());

        match waiter {
            Some(waiter) => {
                // The stream may have given up on it meanwhile.
                let _ = waiter.send(response);
            }
            None => debug!(
                stream_id = response.stream_id(),
                "Dropping unexpected file contents response"
            ),
        }
    }

    fn lock_waiters(&self) -> std::sync::MutexGuard<'_, HashMap<u32, mpsc::Sender<FileContentsResponse<'static>>>> {
        self.waiters.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use ironrdp_cliprdr::pdu::FileContentsFlags;

    use super::*;

    #[derive(Debug)]
    struct NoProxy;

    impl ClipboardMessageProxy for NoProxy {
        fn send_clipboard_message(&self, _message: ClipboardMessage) {}
    }

    fn request() -> FileContentsRequest {
        FileContentsRequest {
            stream_id: 0,
            index: 0,
            flags: FileContentsFlags::RANGE,
            position: 0,
            requested_size: 1024,
            data_id: Some(1),
        }
    }

    #[test]
    fn a_download_runs_while_a_request_is_unanswered_and_shortly_after() {
        let shared = ClipboardShared::new(Box::new(NoProxy));
        assert!(!shared.downloading());

        let receiver = shared.request_file_contents(request());
        assert!(shared.downloading());

        shared.deliver(FileContentsResponse::new_data_response(1, vec![0; 4]));
        assert!(receiver.recv().is_ok());
        // Answered, but the reader is about to ask for the next chunk.
        assert!(shared.downloading());

        let sent = shared.last_file_request_ms.load(Ordering::Acquire);
        assert!(shared.downloading_at(sent + DOWNLOAD_IDLE_MS - 1));
        assert!(!shared.downloading_at(sent + DOWNLOAD_IDLE_MS));
    }

    #[test]
    fn a_new_session_ends_a_download() {
        let shared = ClipboardShared::new(Box::new(NoProxy));
        let _receiver = shared.request_file_contents(request());

        shared.start_session();

        assert!(!shared.downloading());
    }
}
