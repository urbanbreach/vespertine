use std::{
    fs::File,
    io::{ErrorKind, Write},
    os::fd::{AsRawFd, BorrowedFd, OwnedFd},
    sync::Arc,
};

use calloop::{LoopHandle, PostAction};
use filedescriptor::Pipe;
use wayland_client::{Connection, protocol::wl_data_offer::WlDataOffer};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1;

use crate::linux::{
    platform::{PIPE_READ_TIMEOUT, read_fd_with_timeout},
    selection::{
        Offered, Protocol, ReadOptions, ReadReport, SelectionSource, TargetFailure, TargetReader,
        read_item,
    },
};
use vespertine::{ClipboardItem, ClipboardReadError};

pub(crate) const FILE_LIST_MIME_TYPE: &str = crate::linux::selection::URI_LIST_MIME;

/// What this process offers on the clipboard and the primary selection.
///
/// Pure bookkeeping, independent of the Wayland connection: ownership is only taken, and the
/// in-process state only replaced, when the compositor would accept the selection.
#[derive(Default)]
pub(crate) struct OwnedSelections {
    contents: Option<ClipboardItem>,
    source: Option<SelectionSource>,
    primary_contents: Option<ClipboardItem>,
    primary_source: Option<SelectionSource>,
}

impl OwnedSelections {
    /// Starts writing `item` to the clipboard.
    ///
    /// Wayland ownership requires the serial of a recent keyboard or pointer press. Without
    /// one this is a harmless no-op: nothing is replaced, so the previously written item and
    /// the clipboard stay as they were, and `None` is returned. Otherwise the item replaces the
    /// in-process state and the serial and the targets to offer are returned.
    pub(crate) fn begin_write<S: Copy>(
        &mut self,
        item: ClipboardItem,
        serial: Option<S>,
    ) -> Option<(S, Vec<&'static str>)> {
        let serial = serial?;
        let source = SelectionSource::from_item(&item, Protocol::Wayland);
        if source.is_empty() {
            return None;
        }
        let targets = source.targets().collect();
        self.contents = Some(item);
        self.source = Some(source);
        Some((serial, targets))
    }

    /// Starts writing the text of `item` to the primary selection, which is text only: an item
    /// without text, or a write without a serial, is a no-op returning `None`.
    pub(crate) fn begin_primary_write<S: Copy>(
        &mut self,
        item: ClipboardItem,
        serial: Option<S>,
    ) -> Option<(S, Vec<&'static str>)> {
        let serial = serial?;
        let source = SelectionSource::text_only(&item, Protocol::Wayland)?;
        let targets = source.targets().collect();
        self.primary_contents = item.text().map(ClipboardItem::new_string);
        self.primary_source = Some(source);
        Some((serial, targets))
    }

    /// The bytes answering a clipboard request for `mime_type`.
    pub(crate) fn serve(&self, mime_type: &str) -> Option<Arc<[u8]>> {
        self.source.as_ref()?.serve(mime_type).cloned()
    }

    /// The bytes answering a primary selection request for `mime_type`.
    pub(crate) fn serve_primary(&self, mime_type: &str) -> Option<Arc<[u8]>> {
        self.primary_source.as_ref()?.serve(mime_type).cloned()
    }

    /// The item last written to the clipboard.
    pub(crate) fn contents(&self) -> Option<&ClipboardItem> {
        self.contents.as_ref()
    }

    /// The text item last written to the primary selection.
    pub(crate) fn primary_contents(&self) -> Option<&ClipboardItem> {
        self.primary_contents.as_ref()
    }
}

pub(crate) struct Clipboard {
    connection: Connection,
    loop_handle: LoopHandle<'static, ()>,
    self_mime: String,

    // Internal clipboard
    owned: OwnedSelections,

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

struct OfferReader<'a, T: ReceiveData> {
    offer: &'a DataOffer<T>,
    connection: &'a Connection,
}

impl<T: ReceiveData> TargetReader for OfferReader<'_, T> {
    fn read(&mut self, target: &str) -> Result<Vec<u8>, TargetFailure> {
        let pipe = Pipe::new().map_err(|error| {
            log::error!("error creating clipboard pipe: {error:?}");
            TargetFailure::Other
        })?;
        self.offer.inner.receive_data(target.to_string(), unsafe {
            BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
        });
        let fd = pipe.read;
        drop(pipe.write);

        self.connection.flush().map_err(|error| {
            log::error!("error flushing clipboard request: {error:?}");
            TargetFailure::Other
        })?;

        read_fd_with_timeout(fd, PIPE_READ_TIMEOUT).map_err(|error| {
            log::error!("error reading clipboard pipe: {error:?}");
            match error.downcast_ref::<std::io::Error>() {
                Some(io_error) if io_error.kind() == ErrorKind::TimedOut => TargetFailure::Timeout,
                _ => TargetFailure::Other,
            }
        })
    }
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

    /// Reads every supported representation of the offer. Blocks until the owner has
    /// answered or timed out (4 seconds per target), so it must not run on the UI thread
    /// when the offer belongs to another process.
    fn read_item(&self, connection: &Connection, text_only: bool) -> ReadReport {
        read_item(
            Offered::Listed(&self.mime_types),
            ReadOptions {
                legacy_text: false,
                text_only,
            },
            &mut OfferReader {
                offer: self,
                connection,
            },
        )
    }
}

/// How an asynchronous clipboard read proceeds.
pub(crate) enum AsyncRead {
    /// The answer is known without waiting on another process.
    Ready(Result<Option<ClipboardItem>, ClipboardReadError>),
    /// Read the offer of another process; to be run off the UI thread.
    Offer(PendingOfferRead),
}

/// A clipboard read of another process's offer, detached from the client state so it can run
/// on a background thread.
pub(crate) struct PendingOfferRead {
    offer: DataOffer<WlDataOffer>,
    connection: Connection,
}

impl PendingOfferRead {
    /// Blocks while reading; call from a background thread.
    pub(crate) fn run(self) -> Result<Option<ClipboardItem>, ClipboardReadError> {
        self.offer.read_item(&self.connection, false).into_result()
    }
}

impl Clipboard {
    pub fn new(connection: Connection, loop_handle: LoopHandle<'static, ()>) -> Self {
        Self {
            connection,
            loop_handle,
            self_mime: format!("pid/{}", std::process::id()),

            owned: OwnedSelections::default(),

            cached_read: None,
            current_offer: None,
            cached_primary_read: None,
            current_primary_offer: None,
        }
    }

    /// See [`OwnedSelections::begin_write`].
    pub fn begin_write<S: Copy>(
        &mut self,
        item: ClipboardItem,
        serial: Option<S>,
    ) -> Option<(S, Vec<&'static str>)> {
        self.owned.begin_write(item, serial)
    }

    /// See [`OwnedSelections::begin_primary_write`].
    pub fn begin_primary_write<S: Copy>(
        &mut self,
        item: ClipboardItem,
        serial: Option<S>,
    ) -> Option<(S, Vec<&'static str>)> {
        self.owned.begin_primary_write(item, serial)
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

    pub fn send(&self, mime_type: String, fd: OwnedFd) {
        if let Some(bytes) = self.owned.serve(&mime_type) {
            self.send_bytes(fd, bytes);
        }
    }

    pub fn send_primary(&self, mime_type: String, fd: OwnedFd) {
        if let Some(bytes) = self.owned.serve_primary(&mime_type) {
            self.send_bytes(fd, bytes);
        }
    }

    pub fn read(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_offer.as_ref()?;
        if let Some(cached) = self.cached_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.owned.contents().cloned();
        }

        let item = offer.read_item(&self.connection, false).into_item()?;

        self.cached_read = Some(item.clone());
        Some(item)
    }

    /// Prepares an asynchronous clipboard read: answers from this process's state where
    /// possible, otherwise hands back the offer to read on a background thread.
    pub fn begin_async_read(&self) -> AsyncRead {
        let Some(offer) = self.current_offer.as_ref() else {
            return AsyncRead::Ready(Ok(None));
        };
        if let Some(cached) = self.cached_read.clone() {
            return AsyncRead::Ready(Ok(Some(cached)));
        }
        if offer.has_mime_type(&self.self_mime) {
            return AsyncRead::Ready(Ok(self.owned.contents().cloned()));
        }
        AsyncRead::Offer(PendingOfferRead {
            offer: offer.clone(),
            connection: self.connection.clone(),
        })
    }

    pub fn read_primary(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_primary_offer.as_ref()?;
        if let Some(cached) = self.cached_primary_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.owned.primary_contents().cloned();
        }

        let item = offer.read_item(&self.connection, true).into_item()?;

        self.cached_primary_read = Some(item.clone());
        Some(item)
    }

    pub fn send_bytes(&self, fd: OwnedFd, bytes: impl AsRef<[u8]> + 'static) {
        let mut written = 0;
        self.loop_handle
            .insert_source(
                calloop::generic::Generic::new(
                    File::from(fd),
                    calloop::Interest::WRITE,
                    calloop::Mode::Level,
                ),
                move |_, file, _| {
                    let bytes = bytes.as_ref();
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
    use vespertine::{ClipboardEntry, ClipboardString, Image, ImageFormat};

    use super::*;

    #[test]
    fn write_without_serial_leaves_previous_state_unchanged() {
        let mut owned = OwnedSelections::default();
        let first = ClipboardItem::new_string("first".into());
        let (serial, targets) = owned.begin_write(first.clone(), Some(7u32)).unwrap();
        assert_eq!(serial, 7);
        assert_eq!(
            targets,
            ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"]
        );

        // No recent keyboard or pointer press: no ownership request, nothing replaced.
        let image = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]);
        assert!(
            owned
                .begin_write(ClipboardItem::new_image(&image), None::<u32>)
                .is_none()
        );
        assert!(
            owned
                .begin_primary_write(ClipboardItem::new_string("p".into()), None::<u32>)
                .is_none()
        );
        assert_eq!(owned.contents(), Some(&first));
        assert_eq!(owned.serve("UTF8_STRING").as_deref(), Some(&b"first"[..]));
        assert!(owned.serve("image/png").is_none());
        assert_eq!(owned.primary_contents(), None);
    }

    #[test]
    fn written_items_serve_exactly_their_representations() {
        let mut owned = OwnedSelections::default();
        let image = Image::from_bytes(ImageFormat::Jpeg, vec![9, 8, 7]);
        let item = ClipboardItem::from_entries([
            ClipboardEntry::String(ClipboardString::new("alt".into())),
            ClipboardEntry::Image(image),
        ]);
        let (_, targets) = owned.begin_write(item.clone(), Some(1u32)).unwrap();
        assert_eq!(
            targets,
            [
                "text/plain;charset=utf-8",
                "UTF8_STRING",
                "text/plain",
                "image/jpeg"
            ]
        );
        assert_eq!(owned.serve("image/jpeg").as_deref(), Some(&[9u8, 8, 7][..]));
        assert_eq!(owned.serve("text/plain").as_deref(), Some(&b"alt"[..]));
        assert_eq!(owned.contents(), Some(&item));
    }

    #[test]
    fn in_process_state_returns_the_original_item_with_png_alpha_intact() {
        let mut pixels = Vec::new();
        for index in 0..16u8 {
            pixels.extend([index * 16, 255 - index * 8, index, index * 17]);
        }
        let png = vespertine::clipboard::image_tasks::encode_rgba_png(4, 4, &pixels).unwrap();
        let item = ClipboardItem::new_image(&png);
        let mut owned = OwnedSelections::default();
        owned.begin_write(item.clone(), Some(1u32)).unwrap();

        let read = owned.contents().unwrap();
        assert_eq!(read, &item);
        let image = read.images().next().unwrap();
        assert_eq!(image.format, ImageFormat::Png);
        let decoded = image
            .to_image_data(vespertine::SvgRenderer::new(std::sync::Arc::new(())))
            .unwrap();
        let mut straight = decoded.as_bytes(0).unwrap().to_vec();
        for pixel in straight.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        assert_eq!(straight, pixels);
        // An image request is answered with the image bytes, a text request has no answer.
        assert_eq!(
            owned.serve("image/png").as_deref(),
            Some(png.bytes.as_slice())
        );
        assert!(owned.serve("text/plain").is_none());
    }

    #[test]
    fn primary_selection_is_text_only() {
        let mut owned = OwnedSelections::default();
        let image = Image::from_bytes(ImageFormat::Png, vec![1]);
        let item = ClipboardItem::from_entries([
            ClipboardEntry::String(ClipboardString::new("sel".into())),
            ClipboardEntry::Image(image.clone()),
        ]);
        let (_, targets) = owned.begin_primary_write(item, Some(1u32)).unwrap();
        assert_eq!(
            targets,
            ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"]
        );
        assert_eq!(
            owned.serve_primary("UTF8_STRING").as_deref(),
            Some(&b"sel"[..])
        );
        assert!(owned.serve_primary("image/png").is_none());
        assert_eq!(
            owned.primary_contents(),
            Some(&ClipboardItem::new_string("sel".into()))
        );
        // An image-only item has no primary representation.
        assert!(
            owned
                .begin_primary_write(ClipboardItem::new_image(&image), Some(2u32))
                .is_none()
        );
        assert_eq!(
            owned.serve_primary("UTF8_STRING").as_deref(),
            Some(&b"sel"[..])
        );
    }
}
