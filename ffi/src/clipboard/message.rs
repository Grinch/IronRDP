#[diplomat::bridge]
pub mod ffi {

    use core::fmt::Write as _;

    use diplomat_runtime::DiplomatWrite;

    use super::{format_data, is_visible_format_name};
    use crate::error::ffi::IronRdpError;
    use crate::utils::ffi::VecU8;

    #[diplomat::attr(dotnet, manually_disposable)]
    #[diplomat::opaque]
    pub struct ClipboardMessage(pub ironrdp::cliprdr::backend::ClipboardMessage);

    impl ClipboardMessage {
        #[diplomat::attr(auto, getter = "message_type")]
        pub fn get_message_type(&self) -> ClipboardMessageType {
            match &self.0 {
                ironrdp::cliprdr::backend::ClipboardMessage::SendInitiateCopy(_) => {
                    ClipboardMessageType::SendInitiateCopy
                }
                ironrdp::cliprdr::backend::ClipboardMessage::SendInitiateFileCopy(_) => {
                    ClipboardMessageType::SendInitiateFileCopy
                }
                ironrdp::cliprdr::backend::ClipboardMessage::SendFormatData(_) => ClipboardMessageType::SendFormatData,
                ironrdp::cliprdr::backend::ClipboardMessage::SendInitiatePaste(_) => {
                    ClipboardMessageType::SendInitiatePaste
                }
                ironrdp::cliprdr::backend::ClipboardMessage::SendFileContentsRequest(_) => {
                    ClipboardMessageType::SendFileContentsRequest
                }
                ironrdp::cliprdr::backend::ClipboardMessage::SendFileContentsResponse(_) => {
                    ClipboardMessageType::SendFileContentsResponse
                }
                ironrdp::cliprdr::backend::ClipboardMessage::Error(_) => ClipboardMessageType::Error,
            }
        }

        #[diplomat::attr(auto, getter = "send_initiate_copy")]
        pub fn get_send_initiate_copy(&self) -> Option<Box<ClipboardFormatIterator>> {
            match &self.0 {
                ironrdp::cliprdr::backend::ClipboardMessage::SendInitiateCopy(val) => Some(val.clone()),
                _ => None,
            }
            .map(ClipboardFormatIterator)
            .map(Box::new)
        }

        #[diplomat::attr(auto, getter = "send_format_data")]
        pub fn get_send_format_data(&self) -> Option<Box<FormatDataResponse>> {
            match &self.0 {
                ironrdp::cliprdr::backend::ClipboardMessage::SendFormatData(val) => Some(val.clone()),
                _ => None,
            }
            .map(Some)
            .map(FormatDataResponse)
            .map(Box::new)
        }

        #[diplomat::attr(auto, getter = "send_initiate_paste")]
        pub fn get_send_initiate_paste(&self) -> Option<Box<ClipboardFormatId>> {
            match &self.0 {
                ironrdp::cliprdr::backend::ClipboardMessage::SendInitiatePaste(val) => Some(*val),
                _ => None,
            }
            .map(ClipboardFormatId)
            .map(Box::new)
        }

        #[diplomat::attr(auto, getter = "send_file_contents_request")]
        pub fn get_send_file_contents_request(&self) -> Option<Box<FfiFileContentsRequest>> {
            match &self.0 {
                ironrdp::cliprdr::backend::ClipboardMessage::SendFileContentsRequest(val) => Some(val.clone()),
                _ => None,
            }
            .map(FfiFileContentsRequest)
            .map(Box::new)
        }

        #[diplomat::attr(auto, getter = "send_file_contents_response")]
        pub fn get_send_file_contents_response(&self) -> Option<Box<FfiFileContentsResponse>> {
            match &self.0 {
                ironrdp::cliprdr::backend::ClipboardMessage::SendFileContentsResponse(val) => Some(val.clone()),
                _ => None,
            }
            .map(FfiFileContentsResponse)
            .map(Box::new)
        }

        #[diplomat::attr(auto, getter = "error")]
        pub fn get_error(&self) -> Option<Box<IronRdpError>> {
            match &self.0 {
                ironrdp::cliprdr::backend::ClipboardMessage::Error(e) => {
                    let error_ref: &dyn ironrdp::cliprdr::backend::ClipboardError = e.as_ref();
                    Some(error_ref.into())
                }
                _ => None,
            }
        }
    }

    pub enum ClipboardMessageType {
        SendInitiateCopy,
        SendInitiateFileCopy,
        SendFormatData,
        SendInitiatePaste,
        SendFileContentsRequest,
        SendFileContentsResponse,
        Error,
    }

    /// A list of clipboard formats: the formats on the remote clipboard, or the local ones to
    /// announce with `ActiveStage::initiate_clipboard_copy`.
    #[diplomat::opaque_mut]
    pub struct ClipboardFormatIterator(pub Vec<ironrdp::cliprdr::pdu::ClipboardFormat>);

    impl ClipboardFormatIterator {
        /// Creates an empty list.
        pub fn new() -> Box<ClipboardFormatIterator> {
            Box::new(ClipboardFormatIterator(Vec::new()))
        }

        /// Appends a format.
        pub fn push(&mut self, format: &ClipboardFormat) {
            self.0.push(format.0.clone());
        }

        pub fn len(&self) -> usize {
            self.0.len()
        }

        pub fn is_empty(&self) -> bool {
            self.0.is_empty()
        }

        /// Returns the format at `index`, or `None` past the end.
        pub fn get(&self, index: usize) -> Option<Box<ClipboardFormat>> {
            self.0.get(index).cloned().map(ClipboardFormat).map(Box::new)
        }
    }

    /// A clipboard format: an id, and a name for registered formats such as "HTML Format".
    #[diplomat::opaque]
    pub struct ClipboardFormat(pub ironrdp::cliprdr::pdu::ClipboardFormat);

    impl ClipboardFormat {
        /// Creates a format without a name, as standard formats such as `CF_UNICODETEXT` are.
        pub fn new(id: &ClipboardFormatId) -> Box<ClipboardFormat> {
            Box::new(ClipboardFormat(ironrdp::cliprdr::pdu::ClipboardFormat::new(id.0)))
        }

        /// Creates a registered format with its name.
        ///
        /// The name must not be empty or hold a control or invisible character, such as a null, a
        /// zero-width space or a direction override.
        /// When the remote does not use long format names, a name past 15 UTF-16 code units is
        /// shortened to fit a short format name ([MS-RDPECLIP] 2.2.3.1.1.1).
        ///
        /// [MS-RDPECLIP]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpeclip/fb9b7e0b-6db4-41c2-b83c-f889c1ee7688
        pub fn new_named(id: &ClipboardFormatId, name: &str) -> Result<Box<ClipboardFormat>, Box<IronRdpError>> {
            if !is_visible_format_name(name) {
                return Err("a clipboard format name must not be empty or hold control or invisible characters".into());
            }

            Ok(Box::new(ClipboardFormat(
                ironrdp::cliprdr::pdu::ClipboardFormat::new(id.0)
                    .with_name(ironrdp::cliprdr::pdu::ClipboardFormatName::new(name.to_owned())),
            )))
        }

        #[diplomat::attr(auto, getter = "id")]
        pub fn get_id(&self) -> Box<ClipboardFormatId> {
            Box::new(ClipboardFormatId(self.0.id()))
        }

        pub fn has_name(&self) -> bool {
            self.0.name().is_some()
        }

        /// Writes the format's name; nothing when it has none.
        #[diplomat::attr(auto, getter = "name")]
        pub fn get_name(&self, writeable: &mut DiplomatWrite) {
            if let Some(name) = self.0.name() {
                let _ = writeable.write_str(name.value());
            }
        }
    }

    /// The data of one clipboard format, sent by the remote or to be sent to it.
    #[diplomat::attr(dotnet, manually_disposable)]
    #[diplomat::opaque_mut]
    pub struct FormatDataResponse(pub Option<ironrdp::cliprdr::pdu::OwnedFormatDataResponse>);

    impl FormatDataResponse {
        /// Creates a response carrying `data`, already encoded for its format.
        pub fn new_data(data: &[u8]) -> Box<FormatDataResponse> {
            Box::new(FormatDataResponse(Some(
                ironrdp::cliprdr::pdu::OwnedFormatDataResponse::new_data(data.to_vec()),
            )))
        }

        /// Creates `CF_UNICODETEXT` data: UTF-16LE with a terminating null.
        pub fn new_unicode_text(text: &str) -> Box<FormatDataResponse> {
            Box::new(FormatDataResponse(Some(
                ironrdp::cliprdr::pdu::OwnedFormatDataResponse::new_unicode_string(text),
            )))
        }

        /// Creates a failure response, for a format whose data is not available.
        pub fn new_error() -> Box<FormatDataResponse> {
            Box::new(FormatDataResponse(Some(
                ironrdp::cliprdr::pdu::OwnedFormatDataResponse::new_error(),
            )))
        }

        /// Whether the remote reported a failure instead of data.
        pub fn is_error(&self) -> Result<bool, Box<IronRdpError>> {
            Ok(format_data(self)?.is_error())
        }

        /// The raw data, as encoded for its format.
        pub fn data(&self) -> Result<Box<VecU8>, Box<IronRdpError>> {
            Ok(Box::new(VecU8(format_data(self)?.data().to_vec())))
        }

        /// The number of data bytes.
        pub fn data_size(&self) -> Result<usize, Box<IronRdpError>> {
            Ok(format_data(self)?.data().len())
        }
    }

    #[diplomat::opaque]
    pub struct ClipboardFormatId(pub ironrdp::cliprdr::pdu::ClipboardFormatId);

    impl ClipboardFormatId {
        /// Creates a format id; standard ids are below 0xC000, such as 13 for `CF_UNICODETEXT`.
        pub fn new(id: u32) -> Box<ClipboardFormatId> {
            Box::new(ClipboardFormatId(ironrdp::cliprdr::pdu::ClipboardFormatId::new(id)))
        }

        #[diplomat::attr(auto, getter = "value")]
        pub fn get_value(&self) -> u32 {
            self.0.value()
        }
    }

    #[diplomat::opaque]
    pub struct FfiFileContentsRequest(pub ironrdp::cliprdr::pdu::FileContentsRequest);

    impl FfiFileContentsRequest {
        pub fn stream_id(&self) -> u32 {
            self.0.stream_id
        }

        pub fn index(&self) -> i32 {
            self.0.index
        }

        pub fn is_size_request(&self) -> bool {
            self.0.flags.contains(ironrdp::cliprdr::pdu::FileContentsFlags::SIZE)
        }

        pub fn is_range_request(&self) -> bool {
            self.0.flags.contains(ironrdp::cliprdr::pdu::FileContentsFlags::RANGE)
        }

        pub fn position(&self) -> u64 {
            self.0.position
        }

        pub fn requested_size(&self) -> u32 {
            self.0.requested_size
        }

        pub fn has_data_id(&self) -> bool {
            self.0.data_id.is_some()
        }

        pub fn data_id(&self) -> Result<u32, Box<IronRdpError>> {
            self.0.data_id.ok_or_else(|| "no data_id present in request".into())
        }
    }

    /// Wraps `OwnedFileContentsResponse`, which is a type alias for
    /// `FileContentsResponse<'static>` (generated by `impl_pdu_borrowing!`).
    #[diplomat::attr(dotnet, manually_disposable)]
    #[diplomat::opaque]
    pub struct FfiFileContentsResponse(pub ironrdp::cliprdr::pdu::OwnedFileContentsResponse);

    impl FfiFileContentsResponse {
        pub fn stream_id(&self) -> u32 {
            self.0.stream_id()
        }

        pub fn is_error(&self) -> bool {
            self.0.is_error()
        }

        pub fn data(&self) -> Box<VecU8> {
            Box::new(VecU8(self.0.data().to_vec()))
        }
    }
}

/// The response a `FormatDataResponse` holds until `ActiveStage::submit_clipboard_format_data` takes it.
fn format_data(
    response: &ffi::FormatDataResponse,
) -> Result<&ironrdp::cliprdr::pdu::OwnedFormatDataResponse, Box<crate::error::ffi::IronRdpError>> {
    response
        .0
        .as_ref()
        .ok_or_else(|| crate::error::ValueConsumedError::for_item("format_data_response").into())
}

/// Whether `name` can name a clipboard format: not empty, and free of characters that do not
/// show, so that a name reads as what it is.
///
/// Those are the control characters (general category Cc, so the null too) and the
/// Default_Ignorable_Code_Point characters of Unicode's DerivedCoreProperties.txt, such as
/// U+200B ZERO WIDTH SPACE and U+202E RIGHT-TO-LEFT OVERRIDE, plus the interlinear annotation
/// characters U+FFF9 to U+FFFB.
pub(crate) fn is_visible_format_name(name: &str) -> bool {
    !name.is_empty()
        && !name
            .chars()
            .any(|character| character.is_control() || is_invisible(character))
}

fn is_invisible(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFFB}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}
