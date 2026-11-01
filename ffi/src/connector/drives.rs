/// A local folder to announce as a redirected drive.
#[derive(Debug, Clone)]
pub(crate) struct SharedFolder {
    pub(crate) name: String,
    pub(crate) path: String,
}

/// Returns why `name` cannot name a redirected drive, if it cannot.
///
/// The name is announced as a NUL-terminated string and shown as `\\tsclient\<name>`, so an empty name, a NUL or a
/// path separator would reach the server cut short or as a different path.
fn invalid_drive_name(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("drive name is empty");
    }
    if name.chars().any(|c| c == '\0' || c == '\\' || c == '/') {
        return Some("drive name contains NUL or a path separator");
    }
    None
}

#[diplomat::bridge]
pub mod ffi {
    use super::SharedFolder;
    use crate::error::ffi::IronRdpError;

    /// Local folders to share with the server as drives, shown in the session as `\\tsclient\<name>`.
    ///
    /// Attach them with `ClientConnector::with_static_channel_rdpdr_drives`.
    #[diplomat::opaque_mut]
    #[derive(Default)]
    pub struct DriveRedirection(pub(crate) Vec<SharedFolder>);

    impl DriveRedirection {
        pub fn new() -> Box<DriveRedirection> {
            Box::<DriveRedirection>::default()
        }

        /// Whether this platform can share more than one folder in a session.
        ///
        /// The Linux and macOS backend serves a single folder, so `add_drive` accepts only one drive there.
        pub fn supports_multiple_drives() -> bool {
            cfg!(windows)
        }

        /// Shares the local folder `path` as the drive `name`.
        ///
        /// `path` must be an absolute path to an existing folder; on Windows it must be on a drive letter
        /// (`C:\Users\me`), not a UNC path. `name` must be nonempty, must not repeat the name of another drive
        /// (ignoring case), and must not contain NUL or a path separator.
        ///
        /// Fails when a check above fails, and on Linux and macOS when a drive was already added
        /// (see `supports_multiple_drives`).
        pub fn add_drive(&mut self, name: &str, path: &str) -> Result<(), Box<IronRdpError>> {
            if let Some(reason) = super::invalid_drive_name(name) {
                return Err(reason.into());
            }
            if self.0.iter().any(|folder| folder.name.eq_ignore_ascii_case(name)) {
                return Err("a drive with this name was already added".into());
            }
            if !Self::supports_multiple_drives() && !self.0.is_empty() {
                return Err("only one drive can be shared on this platform".into());
            }

            let path_ref = std::path::Path::new(path);
            if !path_ref.is_absolute() {
                return Err("drive path is not absolute".into());
            }
            if !path_ref.is_dir() {
                return Err("drive path is not an existing folder".into());
            }

            // The Windows backend rejects UNC, verbatim and relative roots; checking here reports them when the
            // drive is added rather than when it is attached.
            #[cfg(windows)]
            ironrdp_rdpdr_native::RedirectedDrive::new(1, name, path, false).map_err(|error| {
                crate::error::GenericError(anyhow::Error::new(error).context("invalid redirected drive"))
            })?;

            self.0.push(SharedFolder {
                name: name.to_owned(),
                path: path.to_owned(),
            });
            Ok(())
        }

        /// The number of drives added.
        #[diplomat::attr(auto, getter = "count")]
        pub fn get_count(&self) -> usize {
            self.0.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::invalid_drive_name;

    #[test]
    fn drive_names_reject_empty_nul_and_separators() {
        assert!(invalid_drive_name("Documents").is_none());
        assert!(invalid_drive_name("My Files").is_none());
        assert!(invalid_drive_name("").is_some());
        assert!(invalid_drive_name("a\0b").is_some());
        assert!(invalid_drive_name("a\\b").is_some());
        assert!(invalid_drive_name("a/b").is_some());
    }

    #[test]
    fn add_drive_rejects_relative_paths_and_repeated_names() {
        let mut drives = super::ffi::DriveRedirection::new();
        let folder = std::env::temp_dir();
        let folder = folder.to_str().expect("the temporary folder path is UTF-8");

        assert!(drives.add_drive("Temp", "relative").is_err());
        assert!(drives.add_drive("Temp", folder).is_ok());
        assert!(drives.add_drive("TEMP", folder).is_err());
        assert_eq!(drives.get_count(), 1);
    }
}
