use std::{
    fs::File,
    io::{ErrorKind, Write},
    os::fd::{AsRawFd, BorrowedFd, OwnedFd},
    path::PathBuf,
};

use calloop::{LoopHandle, PostAction};
use filedescriptor::Pipe;
use http_client::Url;
use smallvec::SmallVec;
use strum::IntoEnumIterator;
use wayland_client::{Connection, protocol::wl_data_offer::WlDataOffer};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1;

use crate::linux::platform::{PIPE_READ_TIMEOUT, read_fd_with_timeout};
use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths, Image, ImageFormat, hash};

/// Text mime types that we'll offer to other programs.
pub(crate) const TEXT_MIME_TYPES: [&str; 3] =
    ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
pub(crate) const FILE_LIST_MIME_TYPE: &str = "text/uri-list";

/// Parses a `text/uri-list` body (RFC 2483) into local paths. Skips comments,
/// blank lines and URIs that are not `file://` paths.
pub(crate) fn parse_file_uri_list(list: &str) -> SmallVec<[PathBuf; 2]> {
    list.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| Url::parse(line).ok())
        // `to_file_path` ignores the scheme, so `trash:/a` would become `/a`.
        .filter(|url| url.scheme() == "file")
        .filter_map(|url| url.to_file_path().ok())
        .collect()
}

/// Text mime types that we'll accept from other programs.
pub(crate) const ALLOWED_TEXT_MIME_TYPES: [&str; 2] = ["text/plain;charset=utf-8", "UTF8_STRING"];

pub(crate) struct Clipboard {
    connection: Connection,
    loop_handle: LoopHandle<'static, ()>,
    self_mime: String,

    // Internal clipboard
    contents: Option<ClipboardItem>,
    primary_contents: Option<ClipboardItem>,

    // External clipboard
    cached_read: Option<ClipboardItem>,
    current_offer: Option<DataOffer<WlDataOffer>>,
    cached_primary_read: Option<ClipboardItem>,
    current_primary_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>,
}

pub(crate) trait ReceiveData {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>);
}

impl ReceiveData for WlDataOffer {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

impl ReceiveData for ZwpPrimarySelectionOfferV1 {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

#[derive(Clone, Debug)]
/// Wrapper for `WlDataOffer` and `ZwpPrimarySelectionOfferV1`, used to help track mime types.
pub(crate) struct DataOffer<T: ReceiveData> {
    pub inner: T,
    mime_types: Vec<String>,
}

impl<T: ReceiveData> DataOffer<T> {
    pub fn new(offer: T) -> Self {
        Self {
            inner: offer,
            mime_types: Vec::new(),
        }
    }

    pub fn add_mime_type(&mut self, mime_type: String) {
        self.mime_types.push(mime_type)
    }

    fn has_mime_type(&self, mime_type: &str) -> bool {
        self.mime_types.iter().any(|t| t == mime_type)
    }

    fn read_bytes(&self, connection: &Connection, mime_type: &str) -> Option<Vec<u8>> {
        let pipe = Pipe::new().unwrap();
        self.inner.receive_data(mime_type.to_string(), unsafe {
            BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
        });
        let fd = pipe.read;
        drop(pipe.write);

        connection.flush().unwrap();

        match read_fd_with_timeout(fd, PIPE_READ_TIMEOUT) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                log::error!("error reading clipboard pipe: {err:?}");
                None
            }
        }
    }

    fn read_text(&self, connection: &Connection) -> Option<ClipboardItem> {
        let mime_type = self.mime_types.iter().find(|&mime_type| {
            ALLOWED_TEXT_MIME_TYPES
                .iter()
                .any(|&allowed| allowed == mime_type)
        })?;
        let bytes = self.read_bytes(connection, mime_type)?;
        let text_content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(e) => {
                log::error!("Failed to convert clipboard content to UTF-8: {}", e);
                return None;
            }
        };

        // Normalize the text to unix line endings, otherwise
        // copying from eg: firefox inserts a lot of blank
        // lines, and that is super annoying.
        let result = text_content.replace("\r\n", "\n");
        Some(ClipboardItem::new_string(result))
    }

    /// Copied files come first, so a paste can take them as files; the text or
    /// image stays for readers that want it, as on macOS.
    fn read_item(&self, connection: &Connection) -> Option<ClipboardItem> {
        let files = self.read_files(connection);
        let contents = self
            .read_text(connection)
            .or_else(|| self.read_image(connection));

        let mut entries: Vec<ClipboardEntry> = files
            .map(ClipboardEntry::ExternalPaths)
            .into_iter()
            .collect();
        entries.extend(contents.into_iter().flat_map(|item| item.entries));
        if entries.is_empty() {
            return None;
        }

        Some(ClipboardItem { entries })
    }

    fn read_files(&self, connection: &Connection) -> Option<ExternalPaths> {
        if !self.has_mime_type(FILE_LIST_MIME_TYPE) {
            return None;
        }

        let bytes = self.read_bytes(connection, FILE_LIST_MIME_TYPE)?;
        let list = String::from_utf8(bytes).ok()?;
        let paths = parse_file_uri_list(&list);
        if paths.is_empty() {
            return None;
        }

        Some(ExternalPaths(paths))
    }

    fn read_image(&self, connection: &Connection) -> Option<ClipboardItem> {
        for format in ImageFormat::iter() {
            let mime_type = format.mime_type();
            if !self.has_mime_type(mime_type) {
                continue;
            }

            if let Some(bytes) = self.read_bytes(connection, mime_type) {
                let id = hash(&bytes);
                return Some(ClipboardItem {
                    entries: vec![ClipboardEntry::Image(Image { format, bytes, id })],
                });
            }
        }
        None
    }
}

impl Clipboard {
    pub fn new(connection: Connection, loop_handle: LoopHandle<'static, ()>) -> Self {
        Self {
            connection,
            loop_handle,
            self_mime: format!("pid/{}", std::process::id()),

            contents: None,
            primary_contents: None,

            cached_read: None,
            current_offer: None,
            cached_primary_read: None,
            current_primary_offer: None,
        }
    }

    pub fn set(&mut self, item: ClipboardItem) {
        self.contents = Some(item);
    }

    pub fn set_primary(&mut self, item: ClipboardItem) {
        self.primary_contents = Some(item);
    }

    pub fn set_offer(&mut self, data_offer: Option<DataOffer<WlDataOffer>>) {
        self.cached_read = None;
        self.current_offer = data_offer;
    }

    pub fn set_primary_offer(&mut self, data_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>) {
        self.cached_primary_read = None;
        self.current_primary_offer = data_offer;
    }

    pub fn self_mime(&self) -> String {
        self.self_mime.clone()
    }

    pub fn send(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self.contents.as_ref().and_then(|contents| contents.text()) {
            self.send_bytes(fd, text.as_bytes().to_owned());
        }
    }

    pub fn send_primary(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self
            .primary_contents
            .as_ref()
            .and_then(|contents| contents.text())
        {
            self.send_bytes(fd, text.as_bytes().to_owned());
        }
    }

    pub fn read(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_offer.as_ref()?;
        if let Some(cached) = self.cached_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.contents.clone();
        }

        let item = offer.read_item(&self.connection)?;

        self.cached_read = Some(item.clone());
        Some(item)
    }

    pub fn read_primary(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_primary_offer.as_ref()?;
        if let Some(cached) = self.cached_primary_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.primary_contents.clone();
        }

        let item = offer
            .read_text(&self.connection)
            .or_else(|| offer.read_image(&self.connection))?;

        self.cached_primary_read = Some(item.clone());
        Some(item)
    }

    pub fn send_bytes(&self, fd: OwnedFd, bytes: Vec<u8>) {
        let mut written = 0;
        self.loop_handle
            .insert_source(
                calloop::generic::Generic::new(
                    File::from(fd),
                    calloop::Interest::WRITE,
                    calloop::Mode::Level,
                ),
                move |_, file, _| {
                    let file = unsafe { file.get_mut() };
                    loop {
                        match file.write(&bytes[written..]) {
                            Ok(n) if written + n == bytes.len() => {
                                written += n;
                                break Ok(PostAction::Remove);
                            }
                            Ok(n) => written += n,
                            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                                break Ok(PostAction::Continue);
                            }
                            Err(_) => break Ok(PostAction::Remove),
                        }
                    }
                },
            )
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::HashMap, os::unix::net::UnixStream};

    use gpui::ClipboardString;
    use smallvec::smallvec;

    use super::*;

    #[test]
    fn parses_percent_encoded_spaces_in_file_names() {
        let paths = parse_file_uri_list("file:///home/me/My%20Docs/a%20b.txt\r\n");

        assert_eq!(
            paths.into_vec(),
            vec![PathBuf::from("/home/me/My Docs/a b.txt")]
        );
    }

    #[test]
    fn parses_unicode_file_names_encoded_and_raw() {
        let paths = parse_file_uri_list(
            "file:///tmp/caf%C3%A9%20%E6%97%A5.txt\r\nfile:///tmp/naïve.txt\r\n",
        );

        assert_eq!(
            paths.into_vec(),
            vec![
                PathBuf::from("/tmp/café 日.txt"),
                PathBuf::from("/tmp/naïve.txt"),
            ]
        );
    }

    #[test]
    fn parses_several_files() {
        let paths = parse_file_uri_list("file:///tmp/a\r\nfile:///tmp/b\r\nfile:///tmp/c\r\n");

        assert_eq!(
            paths.into_vec(),
            vec![
                PathBuf::from("/tmp/a"),
                PathBuf::from("/tmp/b"),
                PathBuf::from("/tmp/c"),
            ]
        );
    }

    #[test]
    fn skips_comment_lines() {
        let paths = parse_file_uri_list("# copied from Dolphin\r\nfile:///tmp/a\r\n");

        assert_eq!(paths.into_vec(), vec![PathBuf::from("/tmp/a")]);
    }

    #[test]
    fn skips_blank_lines() {
        let paths = parse_file_uri_list("\r\nfile:///tmp/a\r\n\r\nfile:///tmp/b\r\n\r\n");

        assert_eq!(
            paths.into_vec(),
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
    }

    #[test]
    fn accepts_lf_only_line_ends() {
        let paths = parse_file_uri_list("file:///tmp/a\nfile:///tmp/b");

        assert_eq!(
            paths.into_vec(),
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
    }

    #[test]
    fn skips_non_file_uris() {
        let paths = parse_file_uri_list("https://example.com/a.txt\r\nfile:///tmp/a\r\n");

        assert_eq!(paths.into_vec(), vec![PathBuf::from("/tmp/a")]);
    }

    #[test]
    fn skips_uris_of_other_schemes_that_have_a_path() {
        let paths = parse_file_uri_list("trash:/0-a.txt\r\nfile:///tmp/a\r\n");

        assert_eq!(paths.into_vec(), vec![PathBuf::from("/tmp/a")]);
    }

    #[test]
    fn keeps_trailing_unicode_whitespace_in_file_names() {
        let paths = parse_file_uri_list("file:///tmp/a\u{a0}\r\n");

        assert_eq!(paths.into_vec(), vec![PathBuf::from("/tmp/a\u{a0}")]);
    }

    #[test]
    fn returns_no_paths_when_no_uri_is_a_file() {
        let paths = parse_file_uri_list("https://example.com/a.txt\r\nmailto:me@example.com\r\n");

        assert!(paths.is_empty());
    }

    #[derive(Default)]
    struct FakeOfferSource {
        contents: HashMap<String, Vec<u8>>,
        requested: RefCell<Vec<String>>,
    }

    impl ReceiveData for FakeOfferSource {
        fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
            self.requested.borrow_mut().push(mime_type.clone());
            let bytes = self.contents.get(&mime_type).cloned().unwrap_or_default();
            let mut file = File::from(fd.try_clone_to_owned().unwrap());
            file.write_all(&bytes).unwrap();
        }
    }

    fn fake_offer(entries: &[(&str, &str)]) -> DataOffer<FakeOfferSource> {
        let byte_entries: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(mime_type, content)| (*mime_type, content.as_bytes()))
            .collect();
        fake_offer_bytes(&byte_entries)
    }

    fn fake_offer_bytes(entries: &[(&str, &[u8])]) -> DataOffer<FakeOfferSource> {
        let mut offer = DataOffer::new(FakeOfferSource::default());
        for (mime_type, content) in entries {
            offer.add_mime_type(mime_type.to_string());
            offer
                .inner
                .contents
                .insert(mime_type.to_string(), content.to_vec());
        }
        offer
    }

    fn test_connection() -> (Connection, UnixStream) {
        let (client, server) = UnixStream::pair().unwrap();
        (Connection::from_socket(client).unwrap(), server)
    }

    #[test]
    fn reads_text_when_offer_has_no_file_list() {
        let (connection, _server) = test_connection();
        let offer = fake_offer(&[("text/plain;charset=utf-8", "hello")]);

        assert_eq!(
            offer.read_item(&connection),
            Some(ClipboardItem::new_string("hello".to_string()))
        );
    }

    #[test]
    fn requests_no_file_list_the_offer_does_not_have() {
        let (connection, _server) = test_connection();
        let offer = fake_offer(&[("text/plain;charset=utf-8", "hello")]);

        offer.read_item(&connection);

        assert_eq!(
            *offer.inner.requested.borrow(),
            vec!["text/plain;charset=utf-8".to_string()]
        );
    }

    #[test]
    fn reads_text_when_file_list_has_no_file_uris() {
        let (connection, _server) = test_connection();
        let offer = fake_offer(&[
            (FILE_LIST_MIME_TYPE, "https://example.com/a.txt\r\n"),
            ("text/plain;charset=utf-8", "hello"),
        ]);

        assert_eq!(
            offer.read_item(&connection),
            Some(ClipboardItem::new_string("hello".to_string()))
        );
    }

    #[test]
    fn reads_text_when_file_list_is_not_utf8() {
        let (connection, _server) = test_connection();
        let offer = fake_offer_bytes(&[
            (FILE_LIST_MIME_TYPE, b"file:///tmp/bad\xff.txt\r\n"),
            ("text/plain;charset=utf-8", b"hello"),
        ]);

        assert_eq!(
            offer.read_item(&connection),
            Some(ClipboardItem::new_string("hello".to_string()))
        );
    }

    #[test]
    fn reads_file_list_as_paths_before_text() {
        let (connection, _server) = test_connection();
        let offer = fake_offer(&[
            (
                FILE_LIST_MIME_TYPE,
                "file:///tmp/My%20Docs/caf%C3%A9.txt\r\n",
            ),
            ("text/plain;charset=utf-8", "/tmp/My Docs/café.txt"),
        ]);

        assert_eq!(
            offer.read_item(&connection),
            Some(ClipboardItem {
                entries: vec![
                    ClipboardEntry::ExternalPaths(ExternalPaths(smallvec![PathBuf::from(
                        "/tmp/My Docs/café.txt"
                    )])),
                    ClipboardEntry::String(ClipboardString::new(
                        "/tmp/My Docs/café.txt".to_string()
                    )),
                ],
            })
        );
    }

    #[test]
    fn reads_file_list_as_paths_before_image() {
        let (connection, _server) = test_connection();
        let offer = fake_offer(&[
            (FILE_LIST_MIME_TYPE, "file:///tmp/shot.png\r\n"),
            ("image/png", "png bytes"),
        ]);

        assert_eq!(
            offer.read_item(&connection),
            Some(ClipboardItem {
                entries: vec![
                    ClipboardEntry::ExternalPaths(ExternalPaths(smallvec![PathBuf::from(
                        "/tmp/shot.png"
                    )])),
                    ClipboardEntry::Image(Image {
                        format: ImageFormat::Png,
                        bytes: b"png bytes".to_vec(),
                        id: hash(&b"png bytes".to_vec()),
                    }),
                ],
            })
        );
    }
}
