//! Display-independent clipboard selection logic shared by the Wayland and X11 backends.
//!
//! * [`SelectionSource`] turns a [`ClipboardItem`] into the representations a selection owner
//!   offers (which targets, which bytes) and answers target requests. Backends only move its
//!   output over their protocol.
//! * [`read_item`] assembles a [`ClipboardItem`] from whatever another selection owner offers,
//!   reading targets through a [`TargetReader`]; a failing target never discards the others.
//! * The `text/uri-list` and `x-special/gnome-copied-files` formats are encoded and parsed
//!   here.
//!
//! Nothing in this module touches a display server, so all of it is unit tested.

use std::{
    ffi::OsString,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::Arc,
};

use strum::IntoEnumIterator;
use vespertine::{
    ClipboardEntry, ClipboardItem, ClipboardReadError, ClipboardString, ExternalPaths, Image,
    ImageFormat,
};

pub(crate) const UTF8_STRING: &str = "UTF8_STRING";
pub(crate) const TEXT_UTF8_MIME: &str = "text/plain;charset=utf-8";
pub(crate) const TEXT_PLAIN_MIME: &str = "text/plain";
/// X11 legacy ISO 8859-1 text target.
pub(crate) const STRING: &str = "STRING";
/// X11 legacy text target in an unspecified encoding.
pub(crate) const TEXT: &str = "TEXT";
pub(crate) const URI_LIST_MIME: &str = "text/uri-list";
pub(crate) const GNOME_COPIED_FILES_MIME: &str = "x-special/gnome-copied-files";
/// The application-specific target carrying the metadata of a single string entry, so another
/// instance of this application can restore structure. The plain-text targets never change
/// because of metadata.
pub(crate) const METADATA_MIME: &str = "application/x-vespertine-clipboard-metadata";

/// Which protocol a [`SelectionSource`] serves; it decides the order text targets are offered
/// in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Protocol {
    Wayland,
    X11,
}

impl Protocol {
    fn text_targets(self) -> [&'static str; 3] {
        match self {
            Protocol::Wayland => [TEXT_UTF8_MIME, UTF8_STRING, TEXT_PLAIN_MIME],
            Protocol::X11 => [UTF8_STRING, TEXT_UTF8_MIME, TEXT_PLAIN_MIME],
        }
    }
}

struct Representation {
    targets: Vec<&'static str>,
    bytes: Arc<[u8]>,
}

/// The representations a selection owner offers for one clipboard item.
///
/// Only representations the item contains are offered. A request for a target is answered
/// with exactly the bytes of the representation that target belongs to.
pub(crate) struct SelectionSource {
    representations: Vec<Representation>,
}

impl SelectionSource {
    /// The representations for `item`: text, then file paths, then images, then metadata.
    ///
    /// * Text is offered if the item has a string entry, or else if it has paths (their
    ///   absolute paths joined by line feeds); an image-only item offers no text.
    /// * Paths are offered as `text/uri-list` and `x-special/gnome-copied-files`.
    /// * Each image is offered under the MIME type of its actual format with its original
    ///   bytes.
    /// * The metadata of a single-string item is offered under [`METADATA_MIME`].
    pub(crate) fn from_item(item: &ClipboardItem, protocol: Protocol) -> Self {
        let mut representations = Vec::new();

        let text = item.text();
        if let Some(text) = text.or_else(|| item.has_text_entry().then(String::new)) {
            representations.push(Representation {
                targets: protocol.text_targets().to_vec(),
                bytes: text.into_bytes().into(),
            });
        }

        let paths: Vec<&Path> = item.paths().collect();
        if !paths.is_empty() {
            representations.push(Representation {
                targets: vec![URI_LIST_MIME],
                bytes: format_uri_list(&paths).into_bytes().into(),
            });
            representations.push(Representation {
                targets: vec![GNOME_COPIED_FILES_MIME],
                bytes: format_gnome_copied_files(&paths).into_bytes().into(),
            });
        }

        for image in item.images() {
            let target = image.format.mime_type();
            if representations
                .iter()
                .any(|existing| existing.targets.contains(&target))
            {
                continue;
            }
            representations.push(Representation {
                targets: vec![target],
                bytes: image.bytes.as_slice().into(),
            });
        }

        if let Some(metadata) = item.metadata() {
            representations.push(Representation {
                targets: vec![METADATA_MIME],
                bytes: metadata.as_bytes().into(),
            });
        }

        Self { representations }
    }

    /// A text-only source, as for the primary selection: images, paths and metadata are never
    /// offered. `None` when the item has no text.
    pub(crate) fn text_only(item: &ClipboardItem, protocol: Protocol) -> Option<Self> {
        let text = item.text()?;
        Some(Self {
            representations: vec![Representation {
                targets: protocol.text_targets().to_vec(),
                bytes: text.into_bytes().into(),
            }],
        })
    }

    /// Whether nothing is offered.
    pub(crate) fn is_empty(&self) -> bool {
        self.representations.is_empty()
    }

    /// Every offered target, in offer order.
    pub(crate) fn targets(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.representations
            .iter()
            .flat_map(|representation| representation.targets.iter().copied())
    }

    /// The bytes answering a request for `target`; `None` if the target is not offered.
    pub(crate) fn serve(&self, target: &str) -> Option<&Arc<[u8]>> {
        self.representations
            .iter()
            .find(|representation| representation.targets.contains(&target))
            .map(|representation| &representation.bytes)
    }
}

// ---------------------------------------------------------------------------------------
// text/uri-list and x-special/gnome-copied-files
// ---------------------------------------------------------------------------------------

/// Percent-encodes `bytes`, keeping RFC 3986 unreserved characters and `/`. Byte-wise, so
/// non-UTF-8 path bytes are preserved.
fn percent_encode_path(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 15) as usize] as char);
        }
    }
    encoded
}

fn percent_decode(bytes: &[u8]) -> Vec<u8> {
    let hex = |byte: u8| (byte as char).to_digit(16).map(|digit| digit as u8);
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    decoded
}

/// The `file://` URI of `path`, made absolute.
pub(crate) fn path_to_uri(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    format!(
        "file://{}",
        percent_encode_path(absolute.as_os_str().as_bytes())
    )
}

/// RFC 2483 `text/uri-list`: one URI per line, every line CRLF-terminated.
pub(crate) fn format_uri_list(paths: &[&Path]) -> String {
    paths
        .iter()
        .map(|path| path_to_uri(path) + "\r\n")
        .collect()
}

/// The GNOME file manager convention: a `copy` line followed by the URIs, LF-separated.
pub(crate) fn format_gnome_copied_files(paths: &[&Path]) -> String {
    let mut formatted = String::from("copy");
    for path in paths {
        formatted.push('\n');
        formatted.push_str(&path_to_uri(path));
    }
    formatted
}

/// Parses a `text/uri-list` or `x-special/gnome-copied-files` payload into local paths.
///
/// Only `file://` URIs (empty or `localhost` host, percent-decoded) yield paths. Comment
/// lines (`#`), the `copy`/`cut` action line, non-file URIs and URIs naming another host are
/// ignored.
pub(crate) fn parse_uri_list(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|&byte| byte == b'\n')
        .filter_map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.first() == Some(&b'#') {
                return None;
            }
            file_uri_to_path(line)
        })
        .collect()
}

fn file_uri_to_path(uri: &[u8]) -> Option<PathBuf> {
    let scheme = uri.get(..5)?;
    if !scheme.eq_ignore_ascii_case(b"file:") {
        return None;
    }
    let rest = &uri[5..];
    let path = if let Some(authority_and_path) = rest.strip_prefix(b"//") {
        let path_start = authority_and_path
            .iter()
            .position(|&byte| byte == b'/')
            .unwrap_or(authority_and_path.len());
        let (host, path) = authority_and_path.split_at(path_start);
        if !host.is_empty() && !host.eq_ignore_ascii_case(b"localhost") {
            return None;
        }
        path
    } else {
        rest
    };
    if path.first() != Some(&b'/') {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(percent_decode(path))))
}

// ---------------------------------------------------------------------------------------
// Reading another owner's selection
// ---------------------------------------------------------------------------------------

/// Why reading one target failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetFailure {
    /// The owner did not answer within the per-target timeout (4 seconds).
    Timeout,
    /// The owner refused to convert the selection to this target.
    Refused,
    /// The transfer failed for another reason.
    Other,
}

/// Reads the bytes of one target of another owner's selection.
pub(crate) trait TargetReader {
    fn read(&mut self, target: &str) -> Result<Vec<u8>, TargetFailure>;
}

/// What the owner says it offers.
pub(crate) enum Offered<'a> {
    /// The owner's target list.
    Listed(&'a [String]),
    /// The target list could not be obtained: every target we understand is tried and a
    /// refusal means "not there".
    Unknown,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ReadOptions {
    /// Also accept the X11 legacy `STRING` and `TEXT` targets when no UTF-8 target is offered.
    pub(crate) legacy_text: bool,
    /// Read text only (the primary selection).
    pub(crate) text_only: bool,
}

/// The result of [`read_item`].
#[derive(Debug, Default)]
pub(crate) struct ReadReport {
    pub(crate) item: Option<ClipboardItem>,
    /// The owner offers a target this application does not understand.
    offered_unrecognized: bool,
    /// A target we understand carried content that cannot be used (invalid UTF-8 text).
    rejected: bool,
    refused: bool,
    failed: bool,
}

/// Every target this application offers or reads, without duplicates. Backends that need
/// protocol atoms for targets (X11) obtain them for exactly this set.
pub(crate) fn all_targets() -> impl Iterator<Item = &'static str> {
    [
        UTF8_STRING,
        TEXT_UTF8_MIME,
        TEXT_PLAIN_MIME,
        STRING,
        TEXT,
        URI_LIST_MIME,
        GNOME_COPIED_FILES_MIME,
        METADATA_MIME,
    ]
    .into_iter()
    .chain(ImageFormat::iter().flat_map(ImageFormat::mime_types))
}

impl ReadReport {
    /// A report for a read that could not reach the owner at all.
    pub(crate) fn unavailable() -> Self {
        Self {
            failed: true,
            ..Self::default()
        }
    }

    /// Maps the report to the asynchronous read result: the item; no item for an empty
    /// clipboard; otherwise why content could not be read: [`ClipboardReadError::Denied`] if
    /// the owner refused, [`ClipboardReadError::Unavailable`] if it did not answer, and
    /// [`ClipboardReadError::UnsupportedContent`] if it offers only content this application
    /// cannot represent (other targets, invalid UTF-8 text).
    pub(crate) fn into_result(self) -> Result<Option<ClipboardItem>, ClipboardReadError> {
        if self.item.is_some() {
            return Ok(self.item);
        }
        if self.refused {
            return Err(ClipboardReadError::Denied(
                "the selection owner refused every conversion".into(),
            ));
        }
        if self.failed {
            return Err(ClipboardReadError::Unavailable);
        }
        if self.rejected || self.offered_unrecognized {
            return Err(ClipboardReadError::UnsupportedContent);
        }
        Ok(None)
    }

    /// The item, discarding why there is none.
    pub(crate) fn into_item(self) -> Option<ClipboardItem> {
        self.item
    }
}

/// Targets that describe the selection itself, not its content.
fn is_bookkeeping_target(target: &str) -> bool {
    matches!(
        target,
        "TARGETS"
            | "SAVE_TARGETS"
            | "TIMESTAMP"
            | "MULTIPLE"
            | "DELETE"
            | "INSERT_PROPERTY"
            | "INSERT_SELECTION"
            | "LENGTH"
            | "ATOM"
            | "ATOM_PAIR"
    ) || target.starts_with("pid/")
}

/// Targets this application reads.
fn is_recognized_target(target: &str) -> bool {
    [
        UTF8_STRING,
        TEXT_UTF8_MIME,
        TEXT_PLAIN_MIME,
        STRING,
        TEXT,
        URI_LIST_MIME,
        GNOME_COPIED_FILES_MIME,
        METADATA_MIME,
    ]
    .iter()
    .any(|recognized| recognized.eq_ignore_ascii_case(target))
        || ImageFormat::iter().any(|format| format.mime_types().any(|mime| mime == target))
}

/// Decodes the bytes of a text target: `STRING` as ISO 8859-1, everything else as UTF-8.
/// Invalid UTF-8 yields `None` (dropped, not repaired). CRLF becomes LF.
fn decode_text(target: &str, bytes: Vec<u8>) -> Option<String> {
    let text = if target == STRING {
        bytes.into_iter().map(char::from).collect()
    } else {
        String::from_utf8(bytes).ok()?
    };
    Some(if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text
    })
}

/// Reads the representations another selection owner offers and assembles an item with every
/// supported one, in the order text, paths, image. See the specification of PLT-009 R2:
///
/// * text: the first readable of `UTF8_STRING`, `text/plain;charset=utf-8`, `text/plain`, and
///   with `legacy_text` the `STRING` and `TEXT` targets when no UTF-8 target is offered;
/// * paths: `text/uri-list`, then `x-special/gnome-copied-files`;
/// * image: the first supported format found in the order PNG, JPEG, WebP, GIF, SVG, BMP,
///   TIFF, ICO, PNM, with the true format and a content id; bytes are never transcoded;
/// * metadata of a text-only result from [`METADATA_MIME`], if offered.
pub(crate) fn read_item(
    offered: Offered<'_>,
    options: ReadOptions,
    reader: &mut impl TargetReader,
) -> ReadReport {
    let mut report = ReadReport::default();
    let unknown = matches!(offered, Offered::Unknown);
    let find = |target: &str| -> Option<String> {
        match &offered {
            Offered::Listed(list) => list
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(target))
                .cloned(),
            Offered::Unknown => Some(target.to_owned()),
        }
    };
    if let Offered::Listed(list) = &offered {
        report.offered_unrecognized = list
            .iter()
            .any(|target| !is_bookkeeping_target(target) && !is_recognized_target(target));
    }

    // Reads a target; records failures for the report. In unknown mode a refusal only means
    // the target is absent.
    let mut read = |report: &mut ReadReport, name: &str| -> Option<Vec<u8>> {
        match reader.read(name) {
            Ok(bytes) => Some(bytes),
            Err(TargetFailure::Refused) if unknown => None,
            Err(TargetFailure::Refused) => {
                report.refused = true;
                None
            }
            Err(TargetFailure::Timeout | TargetFailure::Other) => {
                report.failed = true;
                None
            }
        }
    };

    // Text.
    let utf8_offered = [UTF8_STRING, TEXT_UTF8_MIME]
        .iter()
        .any(|target| !unknown && find(target).is_some());
    let mut text_targets = vec![UTF8_STRING, TEXT_UTF8_MIME, TEXT_PLAIN_MIME];
    if options.legacy_text && !utf8_offered {
        text_targets.extend([STRING, TEXT]);
    }
    let mut text: Option<String> = None;
    for target in text_targets {
        let Some(name) = find(target) else { continue };
        let Some(bytes) = read(&mut report, &name) else {
            continue;
        };
        // Invalid UTF-8 text is dropped, not repaired, and later targets carry the same text.
        match decode_text(target, bytes) {
            Some(decoded) => text = Some(decoded).filter(|text| !text.is_empty()),
            None => report.rejected = true,
        }
        break;
    }

    let mut paths: Vec<PathBuf> = Vec::new();
    let mut image: Option<Image> = None;
    if !options.text_only {
        // Paths.
        for target in [URI_LIST_MIME, GNOME_COPIED_FILES_MIME] {
            let Some(name) = find(target) else { continue };
            let Some(bytes) = read(&mut report, &name) else {
                continue;
            };
            paths = parse_uri_list(&bytes);
            if !paths.is_empty() {
                break;
            }
        }

        // Image: the first supported format found, in probe order.
        'formats: for format in ImageFormat::iter() {
            for mime_type in format.mime_types() {
                let Some(name) = find(mime_type) else {
                    continue;
                };
                let Some(bytes) = read(&mut report, &name) else {
                    continue;
                };
                if bytes.is_empty() {
                    continue;
                }
                image = Some(Image::from_bytes(format, bytes));
                break 'formats;
            }
        }
    }

    let mut entries = Vec::new();
    let single_entry =
        (text.is_some() as u8 + !paths.is_empty() as u8 + image.is_some() as u8) == 1;
    if let Some(text) = text {
        let mut string = ClipboardString::new(text);
        if single_entry
            && let Some(name) = find(METADATA_MIME)
            && let Some(bytes) = read(&mut report, &name)
        {
            string.metadata = String::from_utf8(bytes).ok();
        }
        entries.push(ClipboardEntry::String(string));
    }
    if !paths.is_empty() {
        entries.push(ClipboardEntry::ExternalPaths(ExternalPaths(
            paths.into_iter().collect(),
        )));
    }
    if let Some(image) = image {
        entries.push(ClipboardEntry::Image(image));
    }

    if !entries.is_empty() {
        report.item = Some(ClipboardItem { entries });
        // Failures of other targets do not turn a partial read into an error.
        report.failed = false;
        report.refused = false;
    }
    report
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn png_image() -> Image {
        Image::from_bytes(ImageFormat::Png, b"\x89PNG-bytes".to_vec())
    }

    fn jpeg_image() -> Image {
        Image::from_bytes(ImageFormat::Jpeg, b"\xff\xd8-jpeg-bytes".to_vec())
    }

    fn paths_item() -> ClipboardItem {
        ClipboardItem::new_paths([PathBuf::from("/tmp/a b.txt"), PathBuf::from("/tmp/é")])
    }

    /// Serves a [`SelectionSource`] as another process would see it.
    struct SourceReader<'a> {
        source: &'a SelectionSource,
        requests: Vec<String>,
    }

    impl TargetReader for SourceReader<'_> {
        fn read(&mut self, target: &str) -> Result<Vec<u8>, TargetFailure> {
            self.requests.push(target.to_owned());
            self.source
                .serve(target)
                .map(|bytes| bytes.to_vec())
                .ok_or(TargetFailure::Refused)
        }
    }

    fn round_trip(item: &ClipboardItem, protocol: Protocol) -> Option<ClipboardItem> {
        let source = SelectionSource::from_item(item, protocol);
        let targets: Vec<String> = source.targets().map(str::to_owned).collect();
        let mut reader = SourceReader {
            source: &source,
            requests: Vec::new(),
        };
        read_item(
            Offered::Listed(&targets),
            ReadOptions {
                legacy_text: protocol == Protocol::X11,
                text_only: false,
            },
            &mut reader,
        )
        .into_item()
    }

    #[test]
    fn text_item_offers_only_text_targets_per_protocol() {
        let item = ClipboardItem::new_string("héllo".into());
        let wayland = SelectionSource::from_item(&item, Protocol::Wayland);
        assert_eq!(
            wayland.targets().collect::<Vec<_>>(),
            ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"]
        );
        let x11 = SelectionSource::from_item(&item, Protocol::X11);
        assert_eq!(
            x11.targets().collect::<Vec<_>>(),
            ["UTF8_STRING", "text/plain;charset=utf-8", "text/plain"]
        );
        for target in x11.targets() {
            assert_eq!(&x11.serve(target).unwrap()[..], "héllo".as_bytes());
        }
        assert!(x11.serve("image/png").is_none());
    }

    #[test]
    fn image_item_offers_real_mime_with_original_bytes_and_no_text() {
        for (image, mime) in [
            (png_image(), "image/png"),
            (jpeg_image(), "image/jpeg"),
            (
                Image::from_bytes(ImageFormat::Svg, b"<svg/>".to_vec()),
                "image/svg+xml",
            ),
            (
                Image::from_bytes(ImageFormat::Ico, vec![0, 0, 1, 0]),
                "image/x-icon",
            ),
        ] {
            let item = ClipboardItem::new_image(&image);
            let source = SelectionSource::from_item(&item, Protocol::X11);
            assert_eq!(source.targets().collect::<Vec<_>>(), [mime]);
            assert_eq!(&source.serve(mime).unwrap()[..], image.bytes.as_slice());
            assert!(source.serve("UTF8_STRING").is_none());
            assert!(source.serve("text/plain").is_none());
        }
    }

    #[test]
    fn image_with_text_answers_each_target_with_its_own_bytes() {
        let item = ClipboardItem::from_entries([
            ClipboardEntry::String(ClipboardString::new("alt".into())),
            ClipboardEntry::Image(png_image()),
        ]);
        let source = SelectionSource::from_item(&item, Protocol::Wayland);
        assert_eq!(&source.serve("UTF8_STRING").unwrap()[..], b"alt");
        assert_eq!(&source.serve("text/plain").unwrap()[..], b"alt");
        assert_eq!(
            &source.serve("image/png").unwrap()[..],
            png_image().bytes.as_slice()
        );
        assert!(source.serve("image/jpeg").is_none());
    }

    #[test]
    fn path_item_offers_uri_list_gnome_form_and_newline_joined_text() {
        let item = paths_item();
        let source = SelectionSource::from_item(&item, Protocol::X11);
        assert_eq!(
            &source.serve(URI_LIST_MIME).unwrap()[..],
            b"file:///tmp/a%20b.txt\r\nfile:///tmp/%C3%A9\r\n"
        );
        assert_eq!(
            &source.serve(GNOME_COPIED_FILES_MIME).unwrap()[..],
            b"copy\nfile:///tmp/a%20b.txt\nfile:///tmp/%C3%A9"
        );
        assert_eq!(
            &source.serve("UTF8_STRING").unwrap()[..],
            "/tmp/a b.txt\n/tmp/é".as_bytes()
        );
        assert!(source.serve(METADATA_MIME).is_none());
    }

    #[test]
    fn non_utf8_path_bytes_are_percent_encoded_bytewise() {
        let path = PathBuf::from(OsString::from_vec(b"/tmp/\xff\xfe".to_vec()));
        assert_eq!(path_to_uri(&path), "file:///tmp/%FF%FE");
        assert_eq!(parse_uri_list(b"file:///tmp/%FF%FE\r\n"), [path]);
    }

    #[test]
    fn metadata_is_offered_on_its_own_target_without_touching_text() {
        let item = ClipboardItem::new_string_with_metadata("text".into(), "{\"v\":1}".into());
        let source = SelectionSource::from_item(&item, Protocol::Wayland);
        assert_eq!(&source.serve(METADATA_MIME).unwrap()[..], b"{\"v\":1}");
        assert_eq!(&source.serve("UTF8_STRING").unwrap()[..], b"text");
        assert_eq!(source.targets().count(), 4);
    }

    #[test]
    fn primary_source_is_text_only() {
        let item = ClipboardItem::from_entries([
            ClipboardEntry::String(ClipboardString::new("t".into())),
            ClipboardEntry::Image(png_image()),
        ]);
        let source = SelectionSource::text_only(&item, Protocol::X11).unwrap();
        assert!(
            source
                .targets()
                .all(|target| target.starts_with("text/") || target == UTF8_STRING)
        );
        assert!(
            SelectionSource::text_only(&ClipboardItem::new_image(&png_image()), Protocol::X11)
                .is_none()
        );
    }

    #[test]
    fn round_trips_unicode_exactly() {
        let text = "astral 𝄞 😀 e\u{301} 日本語\nline two";
        for protocol in [Protocol::Wayland, Protocol::X11] {
            let read = round_trip(&ClipboardItem::new_string(text.into()), protocol).unwrap();
            assert_eq!(read, ClipboardItem::new_string(text.into()));
        }
    }

    #[test]
    fn round_trips_png_alpha_bytes_and_format() {
        let image = png_image();
        for protocol in [Protocol::Wayland, Protocol::X11] {
            let read = round_trip(&ClipboardItem::new_image(&image), protocol).unwrap();
            assert_eq!(read, ClipboardItem::new_image(&image));
        }
        let read = round_trip(&ClipboardItem::new_image(&jpeg_image()), Protocol::X11).unwrap();
        assert_eq!(read.images().next().unwrap().format, ImageFormat::Jpeg);
    }

    #[test]
    fn round_trips_paths_and_metadata() {
        let read = round_trip(&paths_item(), Protocol::Wayland).unwrap();
        // The reader sees text (newline joined paths) and the paths themselves.
        assert_eq!(read.text().as_deref(), Some("/tmp/a b.txt\n/tmp/é"));
        assert_eq!(
            read.paths().collect::<Vec<_>>(),
            [Path::new("/tmp/a b.txt"), Path::new("/tmp/é")]
        );
        let with_metadata = ClipboardItem::new_string_with_metadata("t".into(), "meta".into());
        assert_eq!(
            round_trip(&with_metadata, Protocol::X11).unwrap(),
            with_metadata
        );
    }

    #[test]
    fn metadata_is_not_attached_to_multi_entry_reads() {
        let mut item = ClipboardItem::new_string_with_metadata("t".into(), "meta".into());
        item.entries.push(ClipboardEntry::Image(png_image()));
        let read = round_trip(&item, Protocol::X11).unwrap();
        assert_eq!(read.entries.len(), 2);
        assert_eq!(read.metadata(), None);
    }

    struct MapReader(HashMap<&'static str, Result<Vec<u8>, TargetFailure>>);

    impl TargetReader for MapReader {
        fn read(&mut self, target: &str) -> Result<Vec<u8>, TargetFailure> {
            self.0
                .get(target)
                .cloned()
                .unwrap_or(Err(TargetFailure::Refused))
        }
    }

    fn listed(targets: &[&str]) -> Vec<String> {
        targets.iter().map(|target| target.to_string()).collect()
    }

    fn read_map(
        targets: &[&str],
        options: ReadOptions,
        map: Vec<(&'static str, Result<Vec<u8>, TargetFailure>)>,
    ) -> ReadReport {
        read_item(
            Offered::Listed(&listed(targets)),
            options,
            &mut MapReader(map.into_iter().collect()),
        )
    }

    #[test]
    fn crlf_text_from_other_applications_reads_as_lf() {
        for target in [UTF8_STRING, TEXT_UTF8_MIME, TEXT_PLAIN_MIME, STRING] {
            let report = read_map(
                &[target],
                ReadOptions {
                    legacy_text: true,
                    text_only: false,
                },
                vec![(
                    match target {
                        UTF8_STRING => UTF8_STRING,
                        TEXT_UTF8_MIME => TEXT_UTF8_MIME,
                        TEXT_PLAIN_MIME => TEXT_PLAIN_MIME,
                        _ => STRING,
                    },
                    Ok(b"one\r\ntwo\r\n\r\nthree\rfour".to_vec()),
                )],
            );
            assert_eq!(
                report.item.unwrap().text().as_deref(),
                Some("one\ntwo\n\nthree\rfour"),
                "{target}"
            );
        }
    }

    #[test]
    fn legacy_string_is_latin1_and_only_used_without_utf8_targets() {
        let options = ReadOptions {
            legacy_text: true,
            text_only: false,
        };
        let latin1 = read_map(
            &[STRING],
            options,
            vec![(STRING, Ok(vec![b'a', 0xe9, 0xff]))],
        );
        assert_eq!(latin1.item.unwrap().text().as_deref(), Some("aéÿ"));

        let text_target = read_map(
            &[TEXT],
            options,
            vec![(TEXT, Ok("héllo".as_bytes().to_vec()))],
        );
        assert_eq!(text_target.item.unwrap().text().as_deref(), Some("héllo"));

        // A UTF-8 target is offered, so STRING is never consulted even if the UTF-8 read fails.
        let report = read_map(
            &[UTF8_STRING, STRING],
            options,
            vec![
                (UTF8_STRING, Err(TargetFailure::Timeout)),
                (STRING, Ok(b"x".to_vec())),
            ],
        );
        assert!(report.item.is_none());

        // Wayland does not accept the legacy targets.
        let report = read_map(
            &[STRING],
            ReadOptions::default(),
            vec![(STRING, Ok(b"x".to_vec()))],
        );
        assert!(report.item.is_none());
    }

    #[test]
    fn invalid_utf8_text_is_dropped_but_other_targets_survive() {
        let report = read_map(
            &[UTF8_STRING, "image/png"],
            ReadOptions::default(),
            vec![
                (UTF8_STRING, Ok(vec![b'a', 0xff, b'b'])),
                ("image/png", Ok(b"png".to_vec())),
            ],
        );
        let item = report.item.unwrap();
        assert_eq!(item.text(), None);
        assert_eq!(item.images().next().unwrap().bytes, b"png");

        let only_invalid = read_map(
            &[UTF8_STRING],
            ReadOptions::default(),
            vec![(UTF8_STRING, Ok(vec![0xff]))],
        );
        assert_eq!(
            only_invalid.into_result(),
            Err(ClipboardReadError::UnsupportedContent)
        );
    }

    #[test]
    fn one_failing_target_never_discards_the_others() {
        let report = read_map(
            &[UTF8_STRING, URI_LIST_MIME, "image/png"],
            ReadOptions::default(),
            vec![
                (UTF8_STRING, Err(TargetFailure::Timeout)),
                (URI_LIST_MIME, Ok(b"file:///tmp/x\r\n".to_vec())),
                ("image/png", Err(TargetFailure::Other)),
            ],
        );
        let item = report.into_result().unwrap().unwrap();
        assert_eq!(item.text().as_deref(), Some("/tmp/x"));
        assert_eq!(item.paths().collect::<Vec<_>>(), [Path::new("/tmp/x")]);
    }

    #[test]
    fn entries_are_ordered_text_paths_image() {
        let report = read_map(
            &["image/png", URI_LIST_MIME, UTF8_STRING],
            ReadOptions::default(),
            vec![
                ("image/png", Ok(b"png".to_vec())),
                (URI_LIST_MIME, Ok(b"file:///a\r\n".to_vec())),
                (UTF8_STRING, Ok(b"names".to_vec())),
            ],
        );
        let kinds: Vec<_> = report
            .item
            .unwrap()
            .entries
            .iter()
            .map(|entry| match entry {
                ClipboardEntry::String(_) => "text",
                ClipboardEntry::ExternalPaths(_) => "paths",
                ClipboardEntry::Image(_) => "image",
            })
            .collect();
        assert_eq!(kinds, ["text", "paths", "image"]);
    }

    #[test]
    fn image_probe_order_is_png_jpeg_webp_gif_svg_bmp_tiff_ico_pnm() {
        let offered = [
            "image/x-portable-anymap",
            "image/x-icon",
            "image/tiff",
            "image/bmp",
            "image/svg+xml",
            "image/gif",
            "image/webp",
            "image/jpeg",
            "image/png",
        ];
        let map: Vec<_> = offered
            .iter()
            .map(|mime| (*mime, Ok(mime.as_bytes().to_vec())))
            .collect();
        let mut remaining: Vec<&str> = offered.to_vec();
        let mut found = Vec::new();
        while !remaining.is_empty() {
            let report = read_map(&remaining, ReadOptions::default(), map.clone());
            let image = report.item.unwrap().images().next().unwrap().clone();
            found.push(image.format);
            remaining.retain(|mime| *mime != image.format.mime_type());
        }
        assert_eq!(
            found,
            [
                ImageFormat::Png,
                ImageFormat::Jpeg,
                ImageFormat::Webp,
                ImageFormat::Gif,
                ImageFormat::Svg,
                ImageFormat::Bmp,
                ImageFormat::Tiff,
                ImageFormat::Ico,
                ImageFormat::Pnm,
            ]
        );
    }

    #[test]
    fn image_content_id_is_the_hash_of_the_bytes_and_aliases_are_accepted() {
        let report = read_map(
            &["image/vnd.microsoft.icon"],
            ReadOptions::default(),
            vec![("image/vnd.microsoft.icon", Ok(vec![0, 0, 1, 0]))],
        );
        let item = report.item.unwrap();
        let image = item.images().next().unwrap();
        assert_eq!(image.format, ImageFormat::Ico);
        assert_eq!(
            image.id,
            Image::from_bytes(ImageFormat::Ico, vec![0, 0, 1, 0]).id
        );
    }

    #[test]
    fn empty_clipboard_reads_as_none_not_error() {
        let report = read_map(&[], ReadOptions::default(), vec![]);
        assert_eq!(report.into_result(), Ok(None));
        let bookkeeping = read_map(&["TARGETS", "pid/42"], ReadOptions::default(), vec![]);
        assert_eq!(bookkeeping.into_result(), Ok(None));
        // An owner that refuses everything while the list is unknown is an empty clipboard.
        let unknown = read_item(
            Offered::Unknown,
            ReadOptions::default(),
            &mut MapReader(HashMap::new()),
        );
        assert_eq!(unknown.into_result(), Ok(None));
        // An empty text is not an item.
        let empty_text = read_map(
            &[UTF8_STRING],
            ReadOptions::default(),
            vec![(UTF8_STRING, Ok(Vec::new()))],
        );
        assert!(empty_text.item.is_none());
    }

    #[test]
    fn async_read_errors_are_distinct() {
        let unsupported = read_map(&["text/html"], ReadOptions::default(), vec![]);
        assert_eq!(
            unsupported.into_result(),
            Err(ClipboardReadError::UnsupportedContent)
        );

        let refused = read_map(&[UTF8_STRING], ReadOptions::default(), vec![]);
        assert!(matches!(
            refused.into_result(),
            Err(ClipboardReadError::Denied(_))
        ));

        let unanswered = read_map(
            &[UTF8_STRING],
            ReadOptions::default(),
            vec![(UTF8_STRING, Err(TargetFailure::Timeout))],
        );
        assert_eq!(
            unanswered.into_result(),
            Err(ClipboardReadError::Unavailable)
        );
    }

    #[test]
    fn primary_reads_never_look_at_images_or_paths() {
        let report = read_map(
            &[UTF8_STRING, "image/png", URI_LIST_MIME],
            ReadOptions {
                legacy_text: true,
                text_only: true,
            },
            vec![
                (UTF8_STRING, Ok(b"t".to_vec())),
                ("image/png", Ok(b"png".to_vec())),
                (URI_LIST_MIME, Ok(b"file:///a\r\n".to_vec())),
            ],
        );
        assert_eq!(report.item.unwrap(), ClipboardItem::new_string("t".into()));
    }

    #[test]
    fn uri_lists_keep_only_local_file_uris() {
        let list = b"# comment\r\nhttp://example.com/a\r\nfile:///tmp/a%20b\r\nfile://localhost/tmp/c\r\nFILE:///tmp/d\r\nfile://other-host/tmp/e\r\nftp://x/y\r\n\r\nfile:/tmp/f\r\nfile:relative\r\n";
        assert_eq!(
            parse_uri_list(list),
            [
                PathBuf::from("/tmp/a b"),
                PathBuf::from("/tmp/c"),
                PathBuf::from("/tmp/d"),
                PathBuf::from("/tmp/f"),
            ]
        );
        assert!(parse_uri_list(b"http://example.com/a\r\n# only\r\n").is_empty());
        // The GNOME form's action line is ignored.
        assert_eq!(
            parse_uri_list(b"cut\nfile:///tmp/x\nfile:///tmp/y"),
            [PathBuf::from("/tmp/x"), PathBuf::from("/tmp/y")]
        );
    }

    #[test]
    fn all_ignored_uri_list_yields_no_path_entry() {
        let report = read_map(
            &[URI_LIST_MIME, UTF8_STRING],
            ReadOptions::default(),
            vec![
                (URI_LIST_MIME, Ok(b"http://example.com/a\r\n".to_vec())),
                (UTF8_STRING, Ok(b"http://example.com/a".to_vec())),
            ],
        );
        let item = report.item.unwrap();
        assert_eq!(item.paths().count(), 0);
        assert_eq!(item.text().as_deref(), Some("http://example.com/a"));
    }

    #[test]
    fn gnome_form_is_used_when_the_uri_list_is_missing() {
        let report = read_map(
            &[GNOME_COPIED_FILES_MIME],
            ReadOptions::default(),
            vec![(GNOME_COPIED_FILES_MIME, Ok(b"copy\nfile:///tmp/g".to_vec()))],
        );
        assert_eq!(
            report.item.unwrap().paths().collect::<Vec<_>>(),
            [Path::new("/tmp/g")]
        );
    }
}
