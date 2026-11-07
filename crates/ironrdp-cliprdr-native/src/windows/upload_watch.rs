//! Notices the remote giving up on files it was reading from here.
//!
//! Windows' `rdpclip` abandons a paste of the client's files as soon as something is copied on
//! the remote: it stops asking for the rest, though it keeps its lock. Nothing the client can do
//! revives it, but it can say so, since the remote's own error doesn't say why. To be sure it is
//! that and not a paste that finished or was canceled, all of these must hold: the remote was
//! reading a file part of the way through, its clipboard changed while it was, and then it asked
//! for nothing more for [`STALL_MS`] while still holding the lock it was reading under.

use ironrdp_cliprdr::pdu::{FileContentsFlags, FileContentsRequest};

/// Reads this recent count as the remote still reading when its clipboard changes.
pub(crate) const READING_MS: u64 = 1_000;

/// No request for this long after the remote's clipboard changed means it gave up.
pub(crate) const STALL_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy)]
struct Read {
    at_ms: u64,
    data_id: Option<u32>,
    index: i32,
    /// Where the bytes asked for end.
    end: u64,
    size: Option<u64>,
}

impl Read {
    fn partial(&self) -> bool {
        self.size.is_some_and(|size| self.end < size)
    }
}

#[derive(Debug, Clone, Copy)]
struct Suspect {
    since_ms: u64,
    data_id: Option<u32>,
}

#[derive(Debug, Default)]
pub(crate) struct UploadWatch {
    last: Option<Read>,
    suspect: Option<Suspect>,
}

impl UploadWatch {
    /// A request for local files, with the size of the file it is for when known.
    pub(crate) fn on_request(&mut self, request: &FileContentsRequest, size: Option<u64>, now_ms: u64) {
        // Still reading: whatever changed on the remote didn't stop it.
        self.suspect = None;

        if !request.flags.contains(FileContentsFlags::RANGE) {
            return;
        }

        let end = request.position.saturating_add(u64::from(request.requested_size));
        let continues = self
            .last
            .is_some_and(|last| last.index == request.index && last.data_id == request.data_id);
        let end = match self.last {
            // Readers may ask for a chunk again or out of order: the furthest point counts.
            Some(last) if continues => end.max(last.end),
            _ => end,
        };

        self.last = Some(Read {
            at_ms: now_ms,
            data_id: request.data_id,
            index: request.index,
            end,
            size,
        });
    }

    /// The remote's clipboard changed.
    pub(crate) fn on_remote_format_list(&mut self, now_ms: u64) {
        let Some(last) = self.last else {
            return;
        };

        if now_ms.saturating_sub(last.at_ms) < READING_MS && last.partial() {
            self.suspect = Some(Suspect {
                since_ms: now_ms,
                data_id: last.data_id,
            });
        }
    }

    /// The remote released a lock: a paste under it is over, one way or another.
    pub(crate) fn on_unlock(&mut self, clip_data_id: u32) {
        if self
            .suspect
            .is_some_and(|suspect| suspect.data_id == Some(clip_data_id))
        {
            self.suspect = None;
        }

        if self.last.is_some_and(|last| last.data_id == Some(clip_data_id)) {
            self.last = None;
        }
    }

    /// Whether a check is due; nothing to check otherwise.
    pub(crate) fn watching(&self) -> bool {
        self.suspect.is_some()
    }

    /// True once, when the remote has given up on the files it was reading.
    pub(crate) fn check(&mut self, now_ms: u64, lock_held: impl Fn(u32) -> bool) -> bool {
        let Some(suspect) = self.suspect else {
            return false;
        };

        if now_ms.saturating_sub(suspect.since_ms) < STALL_MS {
            return false;
        }

        self.suspect = None;
        self.last = None;
        // Without a lock, nothing shows the remote still meant to read them.
        suspect.data_id.is_some_and(lock_held)
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: u64 = 10 * 1024 * 1024;
    const CHUNK: u32 = 64 * 1024;

    fn range(position: u64, data_id: Option<u32>) -> FileContentsRequest {
        FileContentsRequest {
            stream_id: 1,
            index: 0,
            flags: FileContentsFlags::RANGE,
            position,
            requested_size: CHUNK,
            data_id,
        }
    }

    /// The remote reads the first chunks under lock 1, the last at 1 000 ms.
    fn reading() -> UploadWatch {
        let mut watch = UploadWatch::default();
        for (i, at) in [(0u64, 900u64), (1, 950), (2, 1_000)] {
            watch.on_request(&range(i * u64::from(CHUNK), Some(1)), Some(SIZE), at);
        }
        watch
    }

    #[test]
    fn a_remote_copy_that_stops_a_partial_read_is_reported_once() {
        let mut watch = reading();
        watch.on_remote_format_list(1_007);
        assert!(watch.watching());

        assert!(!watch.check(1_007 + STALL_MS - 1, |_| true));
        assert!(watch.check(1_007 + STALL_MS, |_| true));
        assert!(!watch.check(1_007 + 2 * STALL_MS, |_| true));
    }

    #[test]
    fn reading_on_after_the_remote_copy_is_not_reported() {
        let mut watch = reading();
        watch.on_remote_format_list(1_007);
        watch.on_request(&range(3 * u64::from(CHUNK), Some(1)), Some(SIZE), 1_050);

        assert!(!watch.check(1_007 + STALL_MS, |_| true));
    }

    #[test]
    fn a_finished_read_is_not_reported() {
        let mut watch = UploadWatch::default();
        watch.on_request(&range(SIZE - u64::from(CHUNK), Some(1)), Some(SIZE), 1_000);
        watch.on_remote_format_list(1_007);

        assert!(!watch.watching());
    }

    #[test]
    fn a_remote_copy_long_after_the_last_read_is_not_reported() {
        let mut watch = reading();
        watch.on_remote_format_list(1_000 + READING_MS);

        assert!(!watch.watching());
    }

    #[test]
    fn a_released_lock_means_the_paste_ended_on_its_own() {
        let mut watch = reading();
        watch.on_remote_format_list(1_007);
        watch.on_unlock(1);

        assert!(!watch.check(1_007 + STALL_MS, |_| true));
    }

    #[test]
    fn nothing_is_reported_once_the_lock_is_gone_or_without_one() {
        let mut watch = reading();
        watch.on_remote_format_list(1_007);
        assert!(!watch.check(1_007 + STALL_MS, |_| false));

        let mut unlocked = UploadWatch::default();
        unlocked.on_request(&range(0, None), Some(SIZE), 1_000);
        unlocked.on_remote_format_list(1_007);
        assert!(!unlocked.check(1_007 + STALL_MS, |_| true));
    }

    #[test]
    fn a_file_of_unknown_size_is_not_reported() {
        let mut watch = UploadWatch::default();
        watch.on_request(&range(0, Some(1)), None, 1_000);
        watch.on_remote_format_list(1_007);

        assert!(!watch.watching());
    }
}
