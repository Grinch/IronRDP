//! A clipboard backend that hands the clipboard channel's events to the embedder.
//!
//! The backend lives inside the clipboard channel of the active stage, so its callbacks run on
//! whichever thread processes PDUs.
//! It only queues what happened; the embedder polls the queue from its own thread, talks to the
//! local clipboard, and answers through the `ActiveStage` clipboard methods.
//! Nothing in it depends on the operating system.

use core::time::Duration;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

use ironrdp::cliprdr::backend::{CliprdrBackend, CliprdrBackendFactory};
use ironrdp::cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FileDescriptor, FormatDataRequest, FormatDataResponse, LockDataId, OwnedFormatDataResponse,
};
use ironrdp::cliprdr::{CliprdrClient, CliprdrSvcMessages};
use ironrdp::pdu::PduResult;
use ironrdp_core::{IntoOwned as _, impl_as_any};
use tracing::{debug, warn};

use super::message::is_visible_format_name;

/// Events kept when the embedder passes no limit.
pub(crate) const DEFAULT_MAX_QUEUED_EVENTS: usize = 64;

/// Format data bytes kept per response when the embedder passes no limit.
pub(crate) const DEFAULT_MAX_FORMAT_DATA_SIZE: usize = 16 * 1024 * 1024;

/// Format data bytes kept across all queued responses when the embedder passes no limit.
pub(crate) const DEFAULT_MAX_QUEUED_DATA_SIZE: usize = 64 * 1024 * 1024;

/// UTF-16 code units of a short format name, leaving room for its null in the 32-byte block of
/// [MS-RDPECLIP] 2.2.3.1.1.1 "Short Format Name (CLIPRDR_SHORT_FORMAT_NAME)".
///
/// [MS-RDPECLIP]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpeclip/fb9b7e0b-6db4-41c2-b83c-f889c1ee7688
const MAX_SHORT_FORMAT_NAME_UNITS: usize = 15;

#[diplomat::bridge]
pub mod ffi {
    use super::{CallbackCliprdrInner, ClipboardEventKind, Limits};
    use crate::clipboard::ffi::CliprdrBackendFactory;
    use crate::clipboard::message::ffi::{ClipboardFormatId, ClipboardFormatIterator, FormatDataResponse};
    use crate::error::ffi::IronRdpError;

    /// A clipboard backend for every platform that queues the clipboard channel's events for the
    /// embedder to poll, instead of talking to an OS clipboard.
    ///
    /// Attach the factory with `ClientConnector::attach_static_cliprdr`, then poll
    /// `next_event_timeout` from a thread of your own and answer each event:
    ///
    /// - `FormatListRequested`: announce the local formats with
    ///   `ActiveStage::initiate_clipboard_copy`, even an empty list.
    ///   The channel does not finish initializing until it has a format list.
    /// - `RemoteFormatList`: the remote clipboard changed. Request the data you want with
    ///   `ActiveStage::initiate_clipboard_paste`.
    /// - `FormatDataRequested`: the remote wants local data in the given format. Answer with
    ///   `ActiveStage::submit_clipboard_format_data`, an error response if you have none.
    /// - `FormatDataResponse`: the answer to one paste you initiated, `response_format` naming the
    ///   format that paste requested. Every paste gets exactly one, in the order initiated; a paste
    ///   of a format this backend cannot receive, such as the file list, gets an error response at
    ///   once without reaching the remote.
    ///
    /// Event data comes from the remote and is untrusted.
    /// The queue and its data are bounded. When `max_queued_events` events wait, a new one is
    /// dropped and counted; the answers to your own pastes are never dropped, and are bounded by
    /// the pastes the channel keeps outstanding. A response larger than `max_format_data_size`,
    /// or one that would take the queued data past `max_queued_data_size`, arrives as an error
    /// response. The queue logs one warning when it starts dropping events and one, with the
    /// count, when it accepts events again.
    ///
    /// Building a backend for a new connection discards every event still queued from the
    /// previous one, and events of that connection are no longer queued.
    #[diplomat::attr(dotnet, manually_disposable)]
    #[diplomat::opaque]
    pub struct CallbackCliprdr(pub(crate) CallbackCliprdrInner);

    /// One event of a `CallbackCliprdr`.
    #[diplomat::attr(dotnet, manually_disposable)]
    #[diplomat::opaque]
    pub struct ClipboardEvent(pub(crate) ClipboardEventKind);

    pub enum ClipboardEventType {
        /// The clipboard channel finished initializing.
        Ready,
        /// The channel asks for the local format list.
        FormatListRequested,
        /// The remote answered a format list we sent; see `format_list_accepted`.
        FormatListResponse,
        /// The remote clipboard changed; see `remote_formats`.
        RemoteFormatList,
        /// The remote requests local data; see `requested_format`.
        FormatDataRequested,
        /// The answer to a paste we initiated; see `response_format` and `format_data`.
        FormatDataResponse,
        /// The remote locked the local clipboard data; see `lock_id`.
        Lock,
        /// The remote released a lock; see `lock_id`.
        Unlock,
    }

    impl CallbackCliprdr {
        /// Creates the backend with the default limits: 64 queued events, 16 MiB per response and
        /// 64 MiB of queued data.
        pub fn new() -> Box<CallbackCliprdr> {
            Box::new(CallbackCliprdr(CallbackCliprdrInner::new(Limits {
                max_events: super::DEFAULT_MAX_QUEUED_EVENTS,
                max_format_data_size: super::DEFAULT_MAX_FORMAT_DATA_SIZE,
                max_queued_data_size: super::DEFAULT_MAX_QUEUED_DATA_SIZE,
            })))
        }

        /// Creates the backend with the given limits, each at least 1, and `max_queued_data_size`
        /// at least `max_format_data_size`.
        pub fn with_limits(
            max_queued_events: u32,
            max_format_data_size: u32,
            max_queued_data_size: u32,
        ) -> Result<Box<CallbackCliprdr>, Box<IronRdpError>> {
            if max_queued_events == 0 || max_format_data_size == 0 || max_queued_data_size == 0 {
                return Err("clipboard limits must be at least 1".into());
            }

            if max_queued_data_size < max_format_data_size {
                return Err("the queued clipboard data limit must hold at least one response".into());
            }

            Ok(Box::new(CallbackCliprdr(CallbackCliprdrInner::new(Limits {
                max_events: usize::try_from(max_queued_events).unwrap_or(usize::MAX),
                max_format_data_size: usize::try_from(max_format_data_size).unwrap_or(usize::MAX),
                max_queued_data_size: usize::try_from(max_queued_data_size).unwrap_or(usize::MAX),
            }))))
        }

        /// Builds backends that queue into this object, one per connection.
        pub fn backend_factory(&self) -> Box<CliprdrBackendFactory> {
            Box::new(CliprdrBackendFactory(Box::new(self.0.factory())))
        }

        /// Returns the next event, or `None` at once if there is none.
        pub fn next_event(&self) -> Option<Box<ClipboardEvent>> {
            self.0.pop(None).map(ClipboardEvent).map(Box::new)
        }

        /// Waits up to `timeout_ms` milliseconds for the next event; `None` if none came.
        pub fn next_event_timeout(&self, timeout_ms: u32) -> Option<Box<ClipboardEvent>> {
            self.0
                .pop(Some(super::Duration::from_millis(u64::from(timeout_ms))))
                .map(ClipboardEvent)
                .map(Box::new)
        }

        /// How many events were dropped because the queue was full, over the object's lifetime.
        #[diplomat::attr(auto, getter = "dropped_event_count")]
        pub fn get_dropped_event_count(&self) -> u64 {
            self.0.dropped()
        }
    }

    impl ClipboardEvent {
        #[diplomat::attr(auto, getter = "event_type")]
        pub fn get_event_type(&self) -> ClipboardEventType {
            match &self.0 {
                ClipboardEventKind::Ready => ClipboardEventType::Ready,
                ClipboardEventKind::FormatListRequested => ClipboardEventType::FormatListRequested,
                ClipboardEventKind::FormatListResponse { .. } => ClipboardEventType::FormatListResponse,
                ClipboardEventKind::RemoteFormatList(_) => ClipboardEventType::RemoteFormatList,
                ClipboardEventKind::FormatDataRequested(_) => ClipboardEventType::FormatDataRequested,
                ClipboardEventKind::FormatDataResponse { .. } => ClipboardEventType::FormatDataResponse,
                ClipboardEventKind::Lock(_) => ClipboardEventType::Lock,
                ClipboardEventKind::Unlock(_) => ClipboardEventType::Unlock,
            }
        }

        /// The formats on the remote clipboard (`RemoteFormatList`), leaving out those whose name
        /// is empty or holds a control or invisible character.
        #[diplomat::attr(auto, getter = "remote_formats")]
        pub fn get_remote_formats(&self) -> Option<Box<ClipboardFormatIterator>> {
            match &self.0 {
                ClipboardEventKind::RemoteFormatList(formats) => {
                    Some(Box::new(ClipboardFormatIterator(formats.clone())))
                }
                _ => None,
            }
        }

        /// The format the remote requests (`FormatDataRequested`).
        #[diplomat::attr(auto, getter = "requested_format")]
        pub fn get_requested_format(&self) -> Option<Box<ClipboardFormatId>> {
            match &self.0 {
                ClipboardEventKind::FormatDataRequested(format) => Some(Box::new(ClipboardFormatId(*format))),
                _ => None,
            }
        }

        /// The format the answered paste requested (`FormatDataResponse`).
        #[diplomat::attr(auto, getter = "response_format")]
        pub fn get_response_format(&self) -> Option<Box<ClipboardFormatId>> {
            match &self.0 {
                ClipboardEventKind::FormatDataResponse { format, .. } => Some(Box::new(ClipboardFormatId(*format))),
                _ => None,
            }
        }

        /// The data the remote sent, or an error response (`FormatDataResponse`).
        #[diplomat::attr(auto, getter = "format_data")]
        pub fn get_format_data(&self) -> Option<Box<FormatDataResponse>> {
            match &self.0 {
                ClipboardEventKind::FormatDataResponse { response, .. } => {
                    Some(Box::new(FormatDataResponse(Some(response.clone()))))
                }
                _ => None,
            }
        }

        /// Whether the remote accepted our format list (`FormatListResponse`).
        pub fn format_list_accepted(&self) -> Result<bool, Box<IronRdpError>> {
            match &self.0 {
                ClipboardEventKind::FormatListResponse { accepted } => Ok(*accepted),
                _ => Err("not a format list response event".into()),
            }
        }

        /// The clipDataId of the lock (`Lock`, `Unlock`), as in [MS-RDPECLIP] 2.2.4.6
        /// "Lock Clipboard Data PDU (CLIPRDR_LOCK_CLIPDATA)".
        ///
        /// [MS-RDPECLIP]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpeclip/fb9b7e0b-6db4-41c2-b83c-f889c1ee7688
        pub fn lock_id(&self) -> Result<u32, Box<IronRdpError>> {
            match &self.0 {
                ClipboardEventKind::Lock(id) | ClipboardEventKind::Unlock(id) => Ok(*id),
                _ => Err("not a lock or unlock event".into()),
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum ClipboardEventKind {
    Ready,
    FormatListRequested,
    FormatListResponse {
        accepted: bool,
    },
    RemoteFormatList(Vec<ClipboardFormat>),
    FormatDataRequested(ClipboardFormatId),
    FormatDataResponse {
        format: ClipboardFormatId,
        response: OwnedFormatDataResponse,
    },
    Lock(u32),
    Unlock(u32),
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    max_events: usize,
    max_format_data_size: usize,
    max_queued_data_size: usize,
}

#[derive(Debug)]
struct EventQueue {
    state: Mutex<QueueState>,
    available: Condvar,
    limits: Limits,
}

#[derive(Debug, Default)]
struct QueueState {
    events: VecDeque<ClipboardEventKind>,
    /// Queued events other than format data responses.
    /// INVARIANT: `bounded_events <= max_events`.
    bounded_events: usize,
    /// Format data bytes in the queued responses.
    /// INVARIANT: `queued_data_size <= max_queued_data_size`.
    queued_data_size: usize,
    dropped: u64,
    /// Events dropped since the queue last accepted one; nonzero while the queue overflows.
    overflow_dropped: u64,
    /// The connection whose backend may queue; each backend built starts a new one.
    connection: u64,
}

impl EventQueue {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Starts a new connection: discards the events queued so far and stops the earlier
    /// connection's backend from queueing more.
    fn start_connection(&self) -> u64 {
        let mut state = self.lock();
        state.connection = state.connection.wrapping_add(1);
        state.events.clear();
        state.bounded_events = 0;
        state.queued_data_size = 0;
        state.overflow_dropped = 0;
        state.connection
    }

    fn push(&self, connection: u64, event: ClipboardEventKind) {
        let mut state = self.lock();
        if state.connection != connection {
            debug!("Ignored a clipboard event of an earlier connection");
            return;
        }

        // Only the latest remote format list matters, so a newer one replaces any still queued.
        if matches!(event, ClipboardEventKind::RemoteFormatList(_)) {
            let before = state.events.len();
            state
                .events
                .retain(|queued| !matches!(queued, ClipboardEventKind::RemoteFormatList(_)));
            let removed = before - state.events.len();
            state.bounded_events -= removed;
        }

        // UNTESTED: the warning is logged once when the queue starts dropping and once, with the count, when it
        // accepts an event again, so a remote flooding the channel cannot flood the log. When changing this, keep
        // the per-event path free of logging at warning level.
        if self.limits.max_events <= state.bounded_events {
            state.dropped = state.dropped.saturating_add(1);
            if state.overflow_dropped == 0 {
                warn!(
                    max_events = self.limits.max_events,
                    "Clipboard event queue is full, dropping new events until the embedder takes some"
                );
            }
            state.overflow_dropped = state.overflow_dropped.saturating_add(1);
            return;
        }

        if state.overflow_dropped != 0 {
            warn!(
                dropped = state.overflow_dropped,
                "Clipboard event queue accepts events again after dropping some"
            );
            state.overflow_dropped = 0;
        }

        state.bounded_events += 1;
        state.events.push_back(event);
        drop(state);
        self.available.notify_one();
    }

    /// Queues the answer to a paste. It is never dropped, so that every paste is answered; data
    /// past the size limits is replaced by an error response.
    fn push_response(&self, connection: u64, format: ClipboardFormatId, response: FormatDataResponse<'_>) {
        let mut state = self.lock();
        if state.connection != connection {
            debug!("Ignored a clipboard format data response of an earlier connection");
            return;
        }

        let size = response.data().len();
        let response = if self.limits.max_format_data_size < size {
            warn!(
                size,
                max_size = self.limits.max_format_data_size,
                "Clipboard data is too large, passing an error response instead"
            );
            OwnedFormatDataResponse::new_error()
        } else if self.limits.max_queued_data_size - state.queued_data_size < size {
            warn!(
                size,
                queued = state.queued_data_size,
                max_queued = self.limits.max_queued_data_size,
                "Queued clipboard data would exceed its limit, passing an error response instead"
            );
            OwnedFormatDataResponse::new_error()
        } else {
            response.into_owned()
        };

        state.queued_data_size += response.data().len();
        state
            .events
            .push_back(ClipboardEventKind::FormatDataResponse { format, response });
        drop(state);
        self.available.notify_one();
    }

    fn pop(&self, timeout: Option<Duration>) -> Option<ClipboardEventKind> {
        let mut state = self.lock();
        let Some(timeout) = timeout else {
            return state.take_front();
        };

        let deadline = Instant::now().checked_add(timeout);

        // UNTESTED: a spurious wakeup must not extend the wait past `timeout`, because an embedder sizes its poll to
        // stay responsive to shutdown.
        // When changing this loop, keep the remaining time derived from the deadline after every wakeup.
        loop {
            if let Some(event) = state.take_front() {
                return Some(event);
            }

            let remaining = match deadline {
                Some(deadline) => deadline.saturating_duration_since(Instant::now()),
                None => timeout,
            };
            if remaining.is_zero() {
                return None;
            }

            state = self
                .available
                .wait_timeout(state, remaining)
                .map(|(state, _)| state)
                .unwrap_or_else(|poisoned| poisoned.into_inner().0);
        }
    }
}

impl QueueState {
    fn take_front(&mut self) -> Option<ClipboardEventKind> {
        let event = self.events.pop_front()?;
        match &event {
            ClipboardEventKind::FormatDataResponse { response, .. } => {
                self.queued_data_size -= response.data().len();
            }
            _ => self.bounded_events -= 1,
        }

        Some(event)
    }
}

pub(crate) struct CallbackCliprdrInner {
    queue: Arc<EventQueue>,
}

impl CallbackCliprdrInner {
    fn new(limits: Limits) -> Self {
        Self {
            queue: Arc::new(EventQueue {
                state: Mutex::new(QueueState::default()),
                available: Condvar::new(),
                limits,
            }),
        }
    }

    fn factory(&self) -> CallbackCliprdrBackendFactory {
        CallbackCliprdrBackendFactory {
            queue: Arc::clone(&self.queue),
        }
    }

    fn pop(&self, timeout: Option<Duration>) -> Option<ClipboardEventKind> {
        self.queue.pop(timeout)
    }

    fn dropped(&self) -> u64 {
        self.queue.lock().dropped
    }
}

struct CallbackCliprdrBackendFactory {
    queue: Arc<EventQueue>,
}

impl CliprdrBackendFactory for CallbackCliprdrBackendFactory {
    fn build_cliprdr_backend(&self) -> Box<dyn CliprdrBackend> {
        Box::new(CallbackCliprdrBackend {
            connection: self.queue.start_connection(),
            queue: Arc::clone(&self.queue),
            pending_pastes: VecDeque::new(),
            initiating_paste: None,
            remote_file_list_format: None,
            long_format_names: true,
        })
    }
}

#[derive(Debug)]
struct CallbackCliprdrBackend {
    queue: Arc<EventQueue>,
    connection: u64,
    /// The formats of the pastes sent and not yet answered, oldest first.
    ///
    /// A Format Data Response PDU names no format ([MS-RDPECLIP] 2.2.5.2), but one is sent for each
    /// request (3.1.5.4.3) and the channel is ordered, so the oldest entry is the paste each
    /// response answers. The channel fails a paste at once past its own bound on unanswered
    /// requests, which bounds this queue too.
    ///
    /// [MS-RDPECLIP]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpeclip/fb9b7e0b-6db4-41c2-b83c-f889c1ee7688
    pending_pastes: VecDeque<ClipboardFormatId>,
    /// The format of the paste `initiate_paste` is handing to the channel. A response the channel
    /// reports meanwhile is its failing that paste at once, without sending it.
    initiating_paste: Option<ClipboardFormatId>,
    /// The id of "FileGroupDescriptorW" on the remote clipboard; the channel keeps that format's
    /// data for file transfer instead of reporting it.
    remote_file_list_format: Option<ClipboardFormatId>,
    /// Whether format lists use Long Format Names; the channel announces them until the remote's
    /// capabilities say otherwise.
    long_format_names: bool,
}

impl_as_any!(CallbackCliprdrBackend);

impl CallbackCliprdrBackend {
    fn push(&self, event: ClipboardEventKind) {
        self.queue.push(self.connection, event);
    }

    /// Queues the answer to the paste the next response belongs to; a response that answers no
    /// paste is ignored, since nothing waits for it.
    fn answer_paste(&mut self, response: FormatDataResponse<'_>) {
        let Some(format) = self.initiating_paste.take().or_else(|| self.pending_pastes.pop_front()) else {
            warn!("Ignored a clipboard format data response that answers no paste");
            return;
        };

        self.queue.push_response(self.connection, format, response);
    }
}

impl CliprdrBackend for CallbackCliprdrBackend {
    fn temporary_directory(&self) -> &str {
        ".cliprdr"
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        // The backend serves no file contents requests, so it advertises neither STREAM_FILECLIP_ENABLED nor
        // CAN_LOCK_CLIPDATA.
        ClipboardGeneralCapabilityFlags::empty()
    }

    fn on_ready(&mut self) {
        self.push(ClipboardEventKind::Ready);
    }

    fn on_request_format_list(&mut self) {
        self.push(ClipboardEventKind::FormatListRequested);
    }

    fn on_format_list_response(&mut self, ok: bool) {
        self.push(ClipboardEventKind::FormatListResponse { accepted: ok });
    }

    fn on_process_negotiated_capabilities(&mut self, capabilities: ClipboardGeneralCapabilityFlags) {
        debug!(?capabilities, "Clipboard capabilities negotiated");
        self.long_format_names = capabilities.contains(ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES);
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        // The channel recognizes the file list by this name, so the same rule finds the format it keeps.
        self.remote_file_list_format = available_formats
            .iter()
            .find(|format| {
                format
                    .name()
                    .is_some_and(|name| name.value() == ClipboardFormatName::FILE_LIST.value())
            })
            .map(ClipboardFormat::id);

        let formats = available_formats
            .iter()
            .filter(|format| {
                let visible = format.name().is_none_or(|name| is_visible_format_name(name.value()));
                if !visible {
                    debug!(
                        id = format.id().value(),
                        "Left out a remote clipboard format with a hidden character"
                    );
                }
                visible
            })
            .cloned()
            .collect();
        self.push(ClipboardEventKind::RemoteFormatList(formats));
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.push(ClipboardEventKind::FormatDataRequested(request.format));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        self.answer_paste(response);
    }

    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        // File transfer is not negotiated, so a conforming remote does not send this.
        debug!(
            stream_id = request.stream_id,
            "Ignored a clipboard file contents request"
        );
    }

    fn on_file_contents_response(&mut self, response: FileContentsResponse<'_>) {
        debug!(
            stream_id = response.stream_id(),
            "Ignored a clipboard file contents response"
        );
    }

    fn on_remote_file_list(&mut self, files: &[FileDescriptor], _clip_data_id: Option<u32>) {
        // UNTESTED: `initiate_paste` fails a paste of the file list before sending it, so this runs only if the two
        // ever disagree on which format is the file list. The paste must still be answered, or the embedder waits
        // for it forever. When changing either, keep every paste answered exactly once.
        debug!(
            file_count = files.len(),
            "Received a clipboard file list, which this backend does not transfer"
        );
        self.answer_paste(OwnedFormatDataResponse::new_error());
    }

    fn on_lock(&mut self, data_id: LockDataId) {
        self.push(ClipboardEventKind::Lock(data_id.0));
    }

    fn on_unlock(&mut self, data_id: LockDataId) {
        self.push(ClipboardEventKind::Unlock(data_id.0));
    }

    fn now_ms(&self) -> u64 {
        ironrdp_cliprdr_native::native_now_ms()
    }
}

/// Requests the remote's data in `format`, remembering the paste so that a `CallbackCliprdr`
/// response names the format it answers.
///
/// With another backend this is [`CliprdrClient::initiate_paste`].
pub(crate) fn initiate_paste(
    cliprdr: &mut CliprdrClient,
    format: ClipboardFormatId,
) -> PduResult<CliprdrSvcMessages<ironrdp::cliprdr::Client>> {
    let Some(backend) = cliprdr.downcast_backend_mut::<CallbackCliprdrBackend>() else {
        return cliprdr.initiate_paste(format);
    };

    if Some(format) == backend.remote_file_list_format {
        debug!(
            ?format,
            "Failing a paste of the clipboard file list, which this backend does not transfer"
        );
        backend
            .queue
            .push_response(backend.connection, format, OwnedFormatDataResponse::new_error());
        return Ok(Vec::new().into());
    }

    backend.initiating_paste = Some(format);
    let result = cliprdr.initiate_paste(format);

    if let Some(backend) = cliprdr.downcast_backend_mut::<CallbackCliprdrBackend>() {
        // Still set unless the channel failed the paste at once, which answered it already.
        if let Some(format) = backend.initiating_paste.take() {
            if result.is_ok() {
                backend.pending_pastes.push_back(format);
            }
        }
    }

    result
}

/// The local formats to announce, with each name shortened to fit a short format name when the
/// remote does not use long ones, as [MS-RDPECLIP] 2.2.3.1.1.1 requires.
///
/// With another backend, the formats are returned unchanged.
///
/// [MS-RDPECLIP]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpeclip/fb9b7e0b-6db4-41c2-b83c-f889c1ee7688
pub(crate) fn local_formats(cliprdr: &CliprdrClient, formats: &[ClipboardFormat]) -> Vec<ClipboardFormat> {
    let short_names = cliprdr
        .downcast_backend::<CallbackCliprdrBackend>()
        .is_some_and(|backend| !backend.long_format_names);
    if !short_names {
        return formats.to_vec();
    }

    formats
        .iter()
        .map(|format| match format.name() {
            Some(name) => {
                ClipboardFormat::new(format.id()).with_name(ClipboardFormatName::new(short_format_name(name.value())))
            }
            None => format.clone(),
        })
        .collect()
}

/// The longest prefix of `name` that fits a short format name, never splitting a character.
fn short_format_name(name: &str) -> String {
    let mut units = 0;
    name.chars()
        .take_while(|character| {
            units += character.len_utf16();
            units <= MAX_SHORT_FORMAT_NAME_UNITS
        })
        .collect()
}
