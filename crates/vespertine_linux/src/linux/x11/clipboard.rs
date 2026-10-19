/*
 * Copyright 2022 - 2025 Zed Industries, Inc.
 * License: Apache-2.0
 * See LICENSE-APACHE for complete license terms
 *
 * Adapted from the x11 submodule of the arboard project https://github.com/1Password/arboard
 *
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 *
 * Copyright 2022 The Arboard contributors
 *
 * The project to which this file belongs is licensed under either of
 * the Apache 2.0 or the MIT license at the licensee's choice. The terms
 * and conditions of the chosen license apply to this file.
*/

// More info about using the clipboard on X11:
// https://tronche.com/gui/x/icccm/sec-2.html#s-2.6
// https://freedesktop.org/wiki/ClipboardManager/

use std::{
    cell::RefCell,
    collections::{HashMap, hash_map::Entry},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    thread_local,
    time::{Duration, Instant},
};

use parking_lot::{Condvar, Mutex, MutexGuard, RwLock};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, NONE,
    connection::{Connection, RequestConnection},
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
            PropertyNotifyEvent, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
            SelectionRequestEvent, Time, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use crate::linux::selection::{
    self, Offered, Protocol, ReadOptions, ReadReport, SelectionSource, TargetFailure, TargetReader,
    read_item,
};
use vespertine::ClipboardItem;

type Result<T, E = Error> = std::result::Result<T, E>;

static CLIPBOARD: Mutex<Option<GlobalClipboard>> = parking_lot::const_mutex(None);

x11rb::atom_manager! {
    pub Atoms: AtomCookies {
        CLIPBOARD,
        PRIMARY,
        SECONDARY,

        CLIPBOARD_MANAGER,
        SAVE_TARGETS,
        TARGETS,
        ATOM,
        INCR,

        // This is just some random name for the property on our window, into which
        // the clipboard owner writes the data we requested.
        ARBOARD_CLIPBOARD,
    }
}

thread_local! {
    static ATOM_NAME_CACHE: RefCell<HashMap<Atom, &'static str>> = Default::default();
}

// Some clipboard items, like images, may take a very long time to produce a
// `SelectionNotify`. Multiple seconds long.
const LONG_TIMEOUT_DUR: Duration = Duration::from_millis(4000);
const SHORT_TIMEOUT_DUR: Duration = Duration::from_millis(10);

#[derive(Debug, PartialEq, Eq)]
enum ManagerHandoverState {
    Idle,
    InProgress,
    Finished,
}

struct GlobalClipboard {
    inner: Arc<Inner>,

    /// Join handle to the thread which serves selection requests.
    server_handle: JoinHandle<()>,
}

struct XContext {
    conn: RustConnection,
    win_id: u32,
}

struct Inner {
    /// The X display every clipboard connection uses.
    display: String,
    /// The context for the thread which serves clipboard read
    /// requests coming to us.
    server: XContext,
    atoms: Atoms,
    /// The atoms of the targets this application offers and reads.
    targets: Vec<(&'static str, Atom)>,

    clipboard: Selection,
    primary: Selection,
    secondary: Selection,

    handover_state: Mutex<ManagerHandoverState>,
    handover_cv: Condvar,

    serve_stopped: AtomicBool,
}

impl XContext {
    fn new(display: &str) -> Result<Self> {
        // create a new connection to an X11 server
        let (conn, screen_num): (RustConnection, _) = RustConnection::connect(Some(display))
            .map_err(|_| {
                Error::unknown("X11 server connection timed out because it was unreachable")
            })?;
        let screen = conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or(Error::unknown("no screen found"))?;
        let win_id = conn.generate_id().map_err(into_unknown)?;

        let event_mask =
            // Just in case that some program reports SelectionNotify events
            // with XCB_EVENT_MASK_PROPERTY_CHANGE mask.
            EventMask::PROPERTY_CHANGE |
            // To receive DestroyNotify event and stop the message loop.
            EventMask::STRUCTURE_NOTIFY;
        // create the window
        conn.create_window(
            // copy as much as possible from the parent, because no other specific input is needed
            COPY_DEPTH_FROM_PARENT,
            win_id,
            screen.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::COPY_FROM_PARENT,
            COPY_FROM_PARENT,
            // don't subscribe to any special events because we are requesting everything we need ourselves
            &CreateWindowAux::new().event_mask(event_mask),
        )
        .map_err(into_unknown)?;
        conn.flush().map_err(into_unknown)?;

        Ok(Self { conn, win_id })
    }
}

#[derive(Default)]
struct Selection {
    data: RwLock<Option<SelectionSource>>,
    /// Mutex around nothing to use with the below condvar.
    mutex: Mutex<()>,
    /// A condvar that is notified when the contents of this clipboard are changed.
    ///
    /// This is associated with `Self::mutex`.
    data_changed: Condvar,
}

#[derive(Debug, Clone)]
struct ClipboardData {
    bytes: Vec<u8>,

    /// The atom representing the format in which the data is encoded.
    format: Atom,
}

enum ReadSelNotifyResult {
    GotData(ClipboardData),
    IncrStarted,
    EventNotRecognized,
}

impl Inner {
    fn new(display: &str) -> Result<Self> {
        let server = XContext::new(display)?;
        let atoms = Atoms::new(&server.conn)
            .map_err(into_unknown)?
            .reply()
            .map_err(into_unknown)?;

        let target_cookies = selection::all_targets()
            .map(|name| {
                let cookie = server
                    .conn
                    .intern_atom(false, name.as_bytes())
                    .map_err(into_unknown)?;
                Ok((name, cookie))
            })
            .collect::<Result<Vec<_>>>()?;
        let targets = target_cookies
            .into_iter()
            .map(|(name, cookie)| Ok((name, cookie.reply().map_err(into_unknown)?.atom)))
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            display: display.to_owned(),
            server,
            atoms,
            targets,
            clipboard: Selection::default(),
            primary: Selection::default(),
            secondary: Selection::default(),
            handover_state: Mutex::new(ManagerHandoverState::Idle),
            handover_cv: Condvar::new(),
            serve_stopped: AtomicBool::new(false),
        })
    }

    fn write(
        &self,
        source: SelectionSource,
        selection: ClipboardKind,
        wait: WaitConfig,
    ) -> Result<()> {
        if self.serve_stopped.load(Ordering::Relaxed) {
            return Err(Error::unknown(
                "The clipboard handler thread seems to have stopped. Logging messages may reveal the cause. (See the `log` crate.)",
            ));
        }

        let server_win = self.server.win_id;

        // ICCCM version 2, section 2.6.1.3 states that we should re-assert ownership whenever data
        // changes.
        self.server
            .conn
            .set_selection_owner(server_win, self.atom_of(selection), Time::CURRENT_TIME)
            .map_err(|_| Error::ClipboardOccupied)?;

        self.server.conn.flush().map_err(into_unknown)?;

        // Just setting the data, and the `serve_requests` will take care of the rest.
        let selection = self.selection_of(selection);
        let mut data_guard = selection.data.write();
        *data_guard = Some(source);

        // Lock the mutex to both ensure that no wakers of `data_changed` can wake us between
        // dropping the `data_guard` and calling `wait[_for]` and that we don't we wake other
        // threads in that position.
        let mut guard = selection.mutex.lock();

        // Notify any existing waiting threads that we have changed the data in the selection.
        // It is important that the mutex is locked to prevent this notification getting lost.
        selection.data_changed.notify_all();

        match wait {
            WaitConfig::None => {}
            WaitConfig::Forever => {
                drop(data_guard);
                selection.data_changed.wait(&mut guard);
            }
            WaitConfig::Until(deadline) => {
                drop(data_guard);
                selection.data_changed.wait_until(&mut guard, deadline);
            }
        }

        Ok(())
    }

    /// Reads what the owner of `selection` offers, through a connection of its own.
    fn read_selection(&self, selection: ClipboardKind) -> ReadReport {
        match self.try_read_selection(selection) {
            Ok(report) => report,
            Err(error) => {
                log::debug!("X11: failed to read the clipboard: {error}");
                ReadReport::unavailable()
            }
        }
    }

    fn try_read_selection(&self, selection: ClipboardKind) -> Result<ReadReport> {
        let owner = self
            .server
            .conn
            .get_selection_owner(self.atom_of(selection))
            .map_err(into_unknown)?
            .reply()
            .map_err(into_unknown)?
            .owner;
        if owner == NONE {
            return Ok(ReadReport::default());
        }

        let reader = XContext::new(&self.display)?;
        let mut lookup = Vec::new();
        let mut names = Vec::new();
        let listed = match self.read_single(&reader, selection, self.atoms.TARGETS) {
            Ok(ClipboardData { bytes, format }) if format == self.atoms.ATOM => {
                for atom in Self::parse_formats(&bytes) {
                    let name = match self.target_name(atom) {
                        Some(name) => name.to_owned(),
                        None => self.atom_name(atom).to_owned(),
                    };
                    lookup.push((name.clone(), atom));
                    names.push(name);
                }
                true
            }
            // The owner is not answering at all.
            Err(Error::Timeout) => return Ok(ReadReport::unavailable()),
            Ok(ClipboardData { format, .. }) => {
                log::trace!(
                    "Unexpected clipboard TARGETS format {}",
                    self.atom_name(format)
                );
                false
            }
            Err(error) => {
                log::trace!("Clipboard TARGETS query failed with {error:?}");
                false
            }
        };

        let options = ReadOptions {
            legacy_text: true,
            text_only: matches!(selection, ClipboardKind::Primary),
        };
        let mut target_reader = X11TargetReader {
            inner: self,
            reader: &reader,
            selection,
            lookup,
        };
        let offered = if listed {
            Offered::Listed(&names)
        } else {
            Offered::Unknown
        };
        Ok(read_item(offered, options, &mut target_reader))
    }

    fn parse_formats(bytes: &[u8]) -> Vec<Atom> {
        bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    fn read_single(
        &self,
        reader: &XContext,
        selection: ClipboardKind,
        target_format: Atom,
    ) -> Result<ClipboardData> {
        // Delete the property so that we can detect (using property notify)
        // when the selection owner receives our request.
        reader
            .conn
            .delete_property(reader.win_id, self.atoms.ARBOARD_CLIPBOARD)
            .map_err(into_unknown)?;

        // request to convert the clipboard selection to our data type(s)
        reader
            .conn
            .convert_selection(
                reader.win_id,
                self.atom_of(selection),
                target_format,
                self.atoms.ARBOARD_CLIPBOARD,
                Time::CURRENT_TIME,
            )
            .map_err(into_unknown)?;
        reader.conn.sync().map_err(into_unknown)?;

        log::trace!("Finished `convert_selection`");

        let mut incr_data: Vec<u8> = Vec::new();
        let mut using_incr = false;

        let mut timeout_end = Instant::now() + LONG_TIMEOUT_DUR;

        while Instant::now() < timeout_end {
            let event = reader.conn.poll_for_event().map_err(into_unknown)?;
            let event = match event {
                Some(e) => e,
                None => {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
            };
            match event {
                // The first response after requesting a selection.
                Event::SelectionNotify(event) => {
                    log::trace!("Read SelectionNotify");
                    let result = self.handle_read_selection_notify(
                        reader,
                        target_format,
                        &mut using_incr,
                        &mut incr_data,
                        event,
                    )?;
                    match result {
                        ReadSelNotifyResult::GotData(data) => return Ok(data),
                        ReadSelNotifyResult::IncrStarted => {
                            // This means we received an indication that an the
                            // data is going to be sent INCRementally. Let's
                            // reset our timeout.
                            timeout_end += SHORT_TIMEOUT_DUR;
                        }
                        ReadSelNotifyResult::EventNotRecognized => (),
                    }
                }
                // If the previous SelectionNotify event specified that the data
                // will be sent in INCR segments, each segment is transferred in
                // a PropertyNotify event.
                Event::PropertyNotify(event) => {
                    let result = self.handle_read_property_notify(
                        reader,
                        using_incr,
                        &mut incr_data,
                        &mut timeout_end,
                        event,
                    )?;
                    if result {
                        return Ok(ClipboardData {
                            bytes: incr_data,
                            format: target_format,
                        });
                    }
                }
                _ => log::trace!(
                    "An unexpected event arrived while reading the clipboard: {:?}",
                    event
                ),
            }
        }
        log::info!("Time-out hit while reading the clipboard.");
        Err(Error::Timeout)
    }

    fn atom_of(&self, selection: ClipboardKind) -> Atom {
        match selection {
            ClipboardKind::Clipboard => self.atoms.CLIPBOARD,
            ClipboardKind::Primary => self.atoms.PRIMARY,
            ClipboardKind::Secondary => self.atoms.SECONDARY,
        }
    }

    fn selection_of(&self, selection: ClipboardKind) -> &Selection {
        match selection {
            ClipboardKind::Clipboard => &self.clipboard,
            ClipboardKind::Primary => &self.primary,
            ClipboardKind::Secondary => &self.secondary,
        }
    }

    fn kind_of(&self, atom: Atom) -> Option<ClipboardKind> {
        match atom {
            a if a == self.atoms.CLIPBOARD => Some(ClipboardKind::Clipboard),
            a if a == self.atoms.PRIMARY => Some(ClipboardKind::Primary),
            a if a == self.atoms.SECONDARY => Some(ClipboardKind::Secondary),
            _ => None,
        }
    }

    /// The target name `atom` stands for, if it is one this application offers or reads.
    fn target_name(&self, atom: Atom) -> Option<&'static str> {
        self.targets
            .iter()
            .find_map(|&(name, candidate)| (candidate == atom).then_some(name))
    }

    /// The atom of the target `name`, if it is one this application offers or reads.
    fn atom_for_target(&self, name: &str) -> Option<Atom> {
        self.targets
            .iter()
            .find_map(|&(candidate, atom)| (candidate == name).then_some(atom))
    }

    fn is_owner(&self, selection: ClipboardKind) -> Result<bool> {
        let current = self
            .server
            .conn
            .get_selection_owner(self.atom_of(selection))
            .map_err(into_unknown)?
            .reply()
            .map_err(into_unknown)?
            .owner;

        Ok(current == self.server.win_id)
    }

    fn query_atom_name(&self, atom: x11rb::protocol::xproto::Atom) -> Result<String> {
        String::from_utf8(
            self.server
                .conn
                .get_atom_name(atom)
                .map_err(into_unknown)?
                .reply()
                .map_err(into_unknown)?
                .name,
        )
        .map_err(into_unknown)
    }

    fn atom_name(&self, atom: x11rb::protocol::xproto::Atom) -> &'static str {
        ATOM_NAME_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            match cache.entry(atom) {
                Entry::Occupied(entry) => *entry.get(),
                Entry::Vacant(entry) => {
                    let s = self
                        .query_atom_name(atom)
                        .map(|s| Box::leak(s.into_boxed_str()) as &str)
                        .unwrap_or("FAILED-TO-GET-THE-ATOM-NAME");
                    entry.insert(s);
                    s
                }
            }
        })
    }

    fn handle_read_selection_notify(
        &self,
        reader: &XContext,
        target_format: u32,
        using_incr: &mut bool,
        incr_data: &mut Vec<u8>,
        event: SelectionNotifyEvent,
    ) -> Result<ReadSelNotifyResult> {
        // The property being set to NONE means that the `convert_selection`
        // failed.

        // According to: https://tronche.com/gui/x/icccm/sec-2.html#s-2.4
        // the target must be set to the same as what we requested.
        if event.property == NONE || event.target != target_format {
            return Err(Error::ContentNotAvailable);
        }
        if self.kind_of(event.selection).is_none() {
            log::info!(
                "Received a SelectionNotify for a selection other than CLIPBOARD, PRIMARY or SECONDARY. This is unexpected."
            );
            return Ok(ReadSelNotifyResult::EventNotRecognized);
        }
        if *using_incr {
            log::warn!("Received a SelectionNotify while already expecting INCR segments.");
            return Ok(ReadSelNotifyResult::EventNotRecognized);
        }
        // Accept any property type. The property type will typically match the format type except
        // when it is `TARGETS` in which case it is `ATOM`. `ANY` is provided to handle the case
        // where the clipboard is not convertible to the requested format. In this case
        // `reply.type_` will have format information, but `bytes` will only be non-empty if `ANY`
        // is provided.
        let property_type = AtomEnum::ANY;
        // request the selection
        let mut reply = reader
            .conn
            .get_property(
                true,
                event.requestor,
                event.property,
                property_type,
                0,
                u32::MAX / 4,
            )
            .map_err(into_unknown)?
            .reply()
            .map_err(into_unknown)?;

        // we found something
        if reply.type_ == self.atoms.INCR {
            // Note that we call the get_property again because we are
            // indicating that we are ready to receive the data by deleting the
            // property, however deleting only works if the type matches the
            // property type. But the type didn't match in the previous call.
            reply = reader
                .conn
                .get_property(
                    true,
                    event.requestor,
                    event.property,
                    self.atoms.INCR,
                    0,
                    u32::MAX / 4,
                )
                .map_err(into_unknown)?
                .reply()
                .map_err(into_unknown)?;
            log::trace!("Receiving INCR segments");
            *using_incr = true;
            if reply.value_len == 4 {
                let min_data_len = reply
                    .value32()
                    .and_then(|mut vals| vals.next())
                    .unwrap_or(0);
                incr_data.reserve(min_data_len as usize);
            }
            Ok(ReadSelNotifyResult::IncrStarted)
        } else {
            Ok(ReadSelNotifyResult::GotData(ClipboardData {
                bytes: reply.value,
                format: reply.type_,
            }))
        }
    }

    /// Returns Ok(true) when the incr_data is ready
    fn handle_read_property_notify(
        &self,
        reader: &XContext,
        using_incr: bool,
        incr_data: &mut Vec<u8>,
        timeout_end: &mut Instant,
        event: PropertyNotifyEvent,
    ) -> Result<bool> {
        if event.atom != self.atoms.ARBOARD_CLIPBOARD || event.state != Property::NEW_VALUE {
            return Ok(false);
        }
        if !using_incr {
            // This must mean the selection owner received our request, and is
            // now preparing the data
            return Ok(false);
        }
        let reply = reader
            .conn
            .get_property(
                true,
                event.window,
                event.atom,
                AtomEnum::ANY,
                0,
                u32::MAX / 4,
            )
            .map_err(into_unknown)?
            .reply()
            .map_err(into_unknown)?;

        // log::trace!("Received segment. value_len {}", reply.value_len,);
        if reply.value_len == 0 {
            // This indicates that all the data has been sent.
            return Ok(true);
        }
        incr_data.extend(reply.value);

        // Let's reset our timeout, since we received a valid chunk.
        *timeout_end = Instant::now() + SHORT_TIMEOUT_DUR;

        // Not yet complete
        Ok(false)
    }

    fn handle_selection_request(&self, event: SelectionRequestEvent) -> Result<()> {
        let selection = match self.kind_of(event.selection) {
            Some(kind) => kind,
            None => {
                log::warn!(
                    "Received a selection request to a selection other than the CLIPBOARD, PRIMARY or SECONDARY. This is unexpected."
                );
                return Ok(());
            }
        };

        let success;
        // we are asked for a list of supported conversion targets
        if event.target == self.atoms.TARGETS {
            log::trace!(
                "Handling TARGETS, dst property is {}",
                self.atom_name(event.property)
            );
            let mut targets = vec![self.atoms.TARGETS, self.atoms.SAVE_TARGETS];
            let data = self.selection_of(selection).data.read();
            if let Some(source) = &*data {
                targets.extend(
                    source
                        .targets()
                        .filter_map(|target| self.atom_for_target(target)),
                );
            }
            self.server
                .conn
                .change_property32(
                    PropMode::REPLACE,
                    event.requestor,
                    event.property,
                    AtomEnum::ATOM,
                    &targets,
                )
                .map_err(into_unknown)?;
            self.server.conn.flush().map_err(into_unknown)?;
            success = true;
        } else {
            log::trace!("Handling request for (probably) the clipboard contents.");
            let data = self.selection_of(selection).data.read();
            // `None` here means we lost ownership of the data since the other side requested
            // the selection, or the target is not one we offer: respond with the property set
            // to none.
            let bytes = data
                .as_ref()
                .zip(self.target_name(event.target))
                .and_then(|(source, target)| source.serve(target));
            success = match bytes {
                Some(bytes) if bytes.len() > self.server.conn.maximum_request_bytes() - 64 => {
                    log::warn!(
                        "Refusing to send {} bytes of clipboard data in one property; the X server's request size limit is {}",
                        bytes.len(),
                        self.server.conn.maximum_request_bytes()
                    );
                    false
                }
                Some(bytes) => {
                    self.server
                        .conn
                        .change_property8(
                            PropMode::REPLACE,
                            event.requestor,
                            event.property,
                            event.target,
                            bytes,
                        )
                        .map_err(into_unknown)?;
                    self.server.conn.flush().map_err(into_unknown)?;
                    true
                }
                None => false,
            };
        }
        // on failure we notify the requester of it
        let property = if success {
            event.property
        } else {
            AtomEnum::NONE.into()
        };
        // tell the requestor that we finished sending data
        self.server
            .conn
            .send_event(
                false,
                event.requestor,
                EventMask::NO_EVENT,
                SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: event.sequence,
                    time: event.time,
                    requestor: event.requestor,
                    selection: event.selection,
                    target: event.target,
                    property,
                },
            )
            .map_err(into_unknown)?;

        self.server.conn.flush().map_err(into_unknown)
    }

    fn ask_clipboard_manager_to_request_our_data(&self) -> Result<()> {
        if self.server.win_id == 0 {
            // This shouldn't really ever happen but let's just check.
            log::error!("The server's window id was 0. This is unexpected");
            return Ok(());
        }

        if !self.is_owner(ClipboardKind::Clipboard)? {
            // We are not owning the clipboard, nothing to do.
            return Ok(());
        }
        if self
            .selection_of(ClipboardKind::Clipboard)
            .data
            .read()
            .is_none()
        {
            // If we don't have any data, there's nothing to do.
            return Ok(());
        }

        // It's important that we lock the state before sending the request
        // because we don't want the request server thread to lock the state
        // after the request but before we can lock it here.
        let mut handover_state = self.handover_state.lock();

        log::trace!("Sending the data to the clipboard manager");
        self.server
            .conn
            .convert_selection(
                self.server.win_id,
                self.atoms.CLIPBOARD_MANAGER,
                self.atoms.SAVE_TARGETS,
                self.atoms.ARBOARD_CLIPBOARD,
                Time::CURRENT_TIME,
            )
            .map_err(into_unknown)?;
        self.server.conn.flush().map_err(into_unknown)?;

        *handover_state = ManagerHandoverState::InProgress;
        let max_handover_duration = Duration::from_millis(100);

        // Note that we are using a parking_lot condvar here, which doesn't wake up
        // spuriously
        let result = self
            .handover_cv
            .wait_for(&mut handover_state, max_handover_duration);

        if *handover_state == ManagerHandoverState::Finished {
            return Ok(());
        }
        if result.timed_out() {
            log::warn!(
                "Could not hand the clipboard contents over to the clipboard manager. The request timed out."
            );
            return Ok(());
        }

        Err(Error::unknown(
            "The handover was not finished and the condvar didn't time out, yet the condvar wait ended. This should be unreachable.",
        ))
    }
}

fn serve_requests(context: Arc<Inner>) -> Result<(), Box<dyn std::error::Error>> {
    fn handover_finished(clip: &Arc<Inner>, mut handover_state: MutexGuard<ManagerHandoverState>) {
        log::trace!("Finishing clipboard manager handover.");
        *handover_state = ManagerHandoverState::Finished;

        // Not sure if unlocking the mutex is necessary here but better safe than sorry.
        drop(handover_state);

        clip.handover_cv.notify_all();
    }

    log::trace!("Started serve requests thread.");

    let _guard = vespertine_util::defer(|| {
        context.serve_stopped.store(true, Ordering::Relaxed);
    });

    let mut written = false;
    let mut notified = false;

    loop {
        match context.server.conn.wait_for_event().map_err(into_unknown)? {
            Event::DestroyNotify(_) => {
                // This window is being destroyed.
                log::trace!("Clipboard server window is being destroyed x_x");
                return Ok(());
            }
            Event::SelectionClear(event) => {
                // TODO: check if this works
                // Someone else has new content in the clipboard, so it is
                // notifying us that we should delete our data now.
                log::trace!("Somebody else owns the clipboard now");

                if let Some(selection) = context.kind_of(event.selection) {
                    let selection = context.selection_of(selection);
                    let mut data_guard = selection.data.write();
                    *data_guard = None;

                    // It is important that this mutex is locked at the time of calling
                    // `notify_all` to prevent notifications getting lost in case the sleeping
                    // thread has unlocked its `data_guard` and is just about to sleep.
                    // It is also important that the RwLock is kept write-locked for the same
                    // reason.
                    let _guard = selection.mutex.lock();
                    selection.data_changed.notify_all();
                }
            }
            Event::SelectionRequest(event) => {
                log::trace!(
                    "SelectionRequest - selection is: {}, target is {}",
                    context.atom_name(event.selection),
                    context.atom_name(event.target),
                );
                // Someone is requesting the clipboard content from us.
                context
                    .handle_selection_request(event)
                    .map_err(into_unknown)?;

                // if we are in the progress of saving to the clipboard manager
                // make sure we save that we have finished writing
                let handover_state = context.handover_state.lock();
                if *handover_state == ManagerHandoverState::InProgress {
                    // Only set written, when the actual contents were written,
                    // not just a response to what TARGETS we have.
                    if event.target != context.atoms.TARGETS {
                        log::trace!("The contents were written to the clipboard manager.");
                        written = true;
                        // if we have written and notified, make sure to notify that we are done
                        if notified {
                            handover_finished(&context, handover_state);
                        }
                    }
                }
            }
            Event::SelectionNotify(event) => {
                // We've requested the clipboard content and this is the answer.
                // Considering that this thread is not responsible for reading
                // clipboard contents, this must come from the clipboard manager
                // signaling that the data was handed over successfully.
                if event.selection != context.atoms.CLIPBOARD_MANAGER {
                    log::error!(
                        "Received a `SelectionNotify` from a selection other than the CLIPBOARD_MANAGER. This is unexpected in this thread."
                    );
                    continue;
                }
                let handover_state = context.handover_state.lock();
                if *handover_state == ManagerHandoverState::InProgress {
                    // Note that some clipboard managers send a selection notify
                    // before even sending a request for the actual contents.
                    // (That's why we use the "notified" & "written" flags)
                    log::trace!(
                        "The clipboard manager indicated that it's done requesting the contents from us."
                    );
                    notified = true;

                    // One would think that we could also finish if the property
                    // here is set 0, because that indicates failure. However
                    // this is not the case; for example on KDE plasma 5.18, we
                    // immediately get a SelectionNotify with property set to 0,
                    // but following that, we also get a valid SelectionRequest
                    // from the clipboard manager.
                    if written {
                        handover_finished(&context, handover_state);
                    }
                }
            }
            _event => {
                // May be useful for debugging but nothing else really.
                //log::trace!("Received unwanted event: {:?}", event);
            }
        }
    }
}

/// Reads the targets of another client's selection through a connection of our own.
struct X11TargetReader<'a> {
    inner: &'a Inner,
    reader: &'a XContext,
    selection: ClipboardKind,
    /// Target names with the atoms the owner listed them under; names the owner did not list
    /// are resolved through the atoms this application knows.
    lookup: Vec<(String, Atom)>,
}

impl TargetReader for X11TargetReader<'_> {
    fn read(&mut self, target: &str) -> std::result::Result<Vec<u8>, TargetFailure> {
        let atom = self
            .lookup
            .iter()
            .find_map(|(name, atom)| (name == target).then_some(*atom))
            .or_else(|| self.inner.atom_for_target(target))
            .ok_or(TargetFailure::Refused)?;
        self.inner
            .read_single(self.reader, self.selection, atom)
            .map(|data| data.bytes)
            .map_err(|error| match error {
                Error::Timeout => TargetFailure::Timeout,
                Error::ContentNotAvailable => TargetFailure::Refused,
                _ => TargetFailure::Other,
            })
    }
}

#[derive(Clone)]
pub(crate) struct Clipboard {
    inner: Arc<Inner>,
}

impl Clipboard {
    /// Returns the process's clipboard, connecting it to `display` if it doesn't exist yet.
    pub(crate) fn new(display: &str) -> Result<Self> {
        let mut global_cb = CLIPBOARD.lock();
        if let Some(global_cb) = &*global_cb {
            return Ok(Self {
                inner: Arc::clone(&global_cb.inner),
            });
        }
        // At this point we know that the clipboard does not exist.
        let ctx = Arc::new(Inner::new(display)?);
        let join_handle = std::thread::Builder::new()
            .name("Clipboard".to_owned())
            .spawn({
                let ctx = Arc::clone(&ctx);
                move || {
                    if let Err(error) = serve_requests(ctx) {
                        log::error!("Worker thread errored with: {}", error);
                    }
                }
            })
            .unwrap();
        *global_cb = Some(GlobalClipboard {
            inner: Arc::clone(&ctx),
            server_handle: join_handle,
        });
        Ok(Self { inner: ctx })
    }

    /// Offers `item` on `selection`: only the representations the item contains, under the
    /// targets of [`SelectionSource`]. The primary selection is text-only.
    pub(crate) fn write_item(
        &self,
        item: &ClipboardItem,
        selection: ClipboardKind,
        wait: WaitConfig,
    ) -> Result<()> {
        let source = match selection {
            ClipboardKind::Primary => SelectionSource::text_only(item, Protocol::X11),
            ClipboardKind::Clipboard | ClipboardKind::Secondary => {
                Some(SelectionSource::from_item(item, Protocol::X11))
                    .filter(|source| !source.is_empty())
            }
        }
        .ok_or_else(|| Error::unknown("the item has no representation to offer"))?;
        self.inner.write(source, selection, wait)
    }

    /// Reads the selection of whichever client owns it, through the X server and a connection
    /// of its own, never shortcutting to this process's own item. Blocks for up to four seconds
    /// per target when the owner is unresponsive, so call it off the UI thread.
    pub(crate) fn read_item(&self, selection: ClipboardKind) -> ReadReport {
        self.inner.read_selection(selection)
    }

    pub fn is_owner(&self, selection: ClipboardKind) -> bool {
        self.inner.is_owner(selection).unwrap_or(false)
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        // There are always at least 3 owners:
        // the global, the server thread, and one `Clipboard::inner`
        const MIN_OWNERS: usize = 3;

        // We start with locking the global guard to prevent race
        // conditions below.
        let mut global_cb = CLIPBOARD.lock();
        if Arc::strong_count(&self.inner) == MIN_OWNERS {
            // If the are the only owners of the clipboard are ourselves and
            // the global object, then we should destroy the global object,
            // and send the data to the clipboard manager.
            //
            // The handover waits for the clipboard manager, so it runs on its own thread rather
            // than blocking this one, which can be the UI thread switching display modes. A
            // clipboard created meanwhile is independent of this one.
            let Some(global_cb) = global_cb.take() else {
                return;
            };
            let inner = Arc::clone(&self.inner);
            match std::thread::Builder::new()
                .name("Clipboard handover".to_owned())
                .spawn(move || shut_down(inner, global_cb))
            {
                Ok(handover) => HANDOVERS.lock().push(handover),
                Err(error) => {
                    log::error!(
                        "Failed to hand the clipboard over to the clipboard manager: {error}"
                    )
                }
            }
        }
    }
}

/// Threads handing a dropped clipboard's data to the clipboard manager.
static HANDOVERS: Mutex<Vec<JoinHandle<()>>> = parking_lot::const_mutex(Vec::new());

/// Waits for dropped clipboards to finish handing their data to the clipboard manager.
///
/// Called before the process exits, which would otherwise lose data still being handed over.
pub(crate) fn wait_for_clipboard_handovers() {
    let handovers = std::mem::take(&mut *HANDOVERS.lock());
    for handover in handovers {
        if handover.join().is_err() {
            log::error!("The clipboard handover thread panicked.");
        }
    }
}

/// Hands the clipboard's data to the clipboard manager, then stops serving it.
fn shut_down(inner: Arc<Inner>, global_cb: GlobalClipboard) {
    if let Err(e) = inner.ask_clipboard_manager_to_request_our_data() {
        log::error!(
            "Could not hand the clipboard data over to the clipboard manager: {}",
            e
        );
    }
    if let Err(e) = inner.server.conn.destroy_window(inner.server.win_id) {
        log::error!("Failed to destroy the clipboard window. Error: {}", e);
        return;
    }
    if let Err(e) = inner.server.conn.flush() {
        log::error!("Failed to flush the clipboard window. Error: {}", e);
        return;
    }
    if let Err(e) = global_cb.server_handle.join() {
        // Let's try extracting the error message
        let message;
        if let Some(msg) = e.downcast_ref::<&'static str>() {
            message = Some((*msg).to_string());
        } else if let Some(msg) = e.downcast_ref::<String>() {
            message = Some(msg.clone());
        } else {
            message = None;
        }
        if let Some(message) = message {
            log::error!(
                "The clipboard server thread panicked. Panic message: '{}'",
                message,
            );
        } else {
            log::error!("The clipboard server thread panicked.");
        }
    }
}

fn into_unknown<E: std::fmt::Display>(error: E) -> Error {
    Error::Unknown {
        description: error.to_string(),
    }
}

/// Clipboard selection
///
/// Linux has a concept of clipboard "selections" which tend to be used in different contexts. This
/// enum provides a way to get/set to a specific clipboard
///
/// See <https://specifications.freedesktop.org/clipboards-spec/clipboards-0.1.txt> for a better
/// description of the different clipboards.
#[derive(Copy, Clone, Debug)]
pub enum ClipboardKind {
    /// Typically used selection for explicit cut/copy/paste actions (ie. windows/macos like
    /// clipboard behavior)
    Clipboard,

    /// Typically used for mouse selections and/or currently selected text. Accessible via middle
    /// mouse click.
    Primary,

    /// The secondary clipboard is rarely used but theoretically available on X11.
    Secondary,
}

/// Configuration on how long to wait for a new X11 copy event is emitted.
#[derive(Default)]
pub(crate) enum WaitConfig {
    /// Waits until the given [`Instant`] has reached.
    #[allow(
        unused,
        reason = "Right now we don't wait for clipboard contents to sync on app close, but we may in the future"
    )]
    Until(Instant),

    /// Waits forever until a new event is reached.
    #[allow(unused)]
    #[allow(
        unused,
        reason = "Right now we don't wait for clipboard contents to sync on app close, but we may in the future"
    )]
    Forever,

    /// It shouldn't wait.
    #[default]
    None,
}

#[non_exhaustive]
pub enum Error {
    /// The clipboard contents were not available in the requested format.
    /// This could either be due to the clipboard being empty or the clipboard contents having
    /// an incompatible format to the requested one (eg when calling `get_image` on text)
    ContentNotAvailable,

    /// The selection owner did not answer within the read timeout.
    Timeout,

    /// The native clipboard is not accessible due to being held by an other party.
    ///
    /// This "other party" could be a different process or it could be within
    /// the same program. So for example you may get this error when trying
    /// to interact with the clipboard from multiple threads at once.
    ///
    /// Note that it's OK to have multiple `Clipboard` instances. The underlying
    /// implementation will make sure that the native clipboard is only
    /// opened for transferring data and then closed as soon as possible.
    ClipboardOccupied,

    /// Any error that doesn't fit the other error types.
    ///
    /// The `description` field is only meant to help the developer and should not be relied on as a
    /// means to identify an error case during runtime.
    Unknown { description: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
			Error::ContentNotAvailable => f.write_str("The clipboard contents were not available in the requested format or the clipboard is empty."),
			Error::Timeout => f.write_str("The selection owner did not answer within the read timeout."),
			Error::ClipboardOccupied => f.write_str("The native clipboard is not accessible due to being held by an other party."),
			Error::Unknown { description } => f.write_fmt(format_args!("Unknown error while interacting with the clipboard: {description}")),
		}
    }
}

impl std::error::Error for Error {}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use Error::*;
        macro_rules! kind_to_str {
			($( $e: pat ),*) => {
				match self {
					$(
						$e => stringify!($e),
					)*
				}
			}
		}
        let name = kind_to_str!(
            ContentNotAvailable,
            Timeout,
            ClipboardOccupied,
            Unknown { .. }
        );
        f.write_fmt(format_args!("{name} - \"{self}\""))
    }
}

impl Error {
    pub(crate) fn unknown<M: Into<String>>(message: M) -> Self {
        Error::Unknown {
            description: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use vespertine::{ClipboardEntry, ClipboardReadError, ClipboardString, Image, ImageFormat};

    use super::*;
    use crate::linux::selection::{
        GNOME_COPIED_FILES_MIME, TargetFailure, TargetReader, URI_LIST_MIME,
    };

    /// Serializes the tests, which share the process-wide clipboard connection.
    static SERIAL: Mutex<()> = parking_lot::const_mutex(());

    /// A clipboard connected to the real X server named by `DISPLAY`, with the serving thread
    /// of this process owning the private SECONDARY selection (nothing else uses it). The
    /// reading side is a second X client: every read opens its own connection.
    struct TestClipboard {
        clipboard: Option<Clipboard>,
        _serial: parking_lot::MutexGuard<'static, ()>,
    }

    impl TestClipboard {
        /// `None` when no X server is reachable; the caller skips.
        fn connect() -> Option<Self> {
            let serial = SERIAL.lock();
            let display = std::env::var("DISPLAY").ok()?;
            let clipboard = match Clipboard::new(&display) {
                Ok(clipboard) => clipboard,
                Err(error) => {
                    eprintln!("skipping: no X server reachable on {display}: {error}");
                    return None;
                }
            };
            Some(Self {
                clipboard: Some(clipboard),
                _serial: serial,
            })
        }

        fn clipboard(&self) -> &Clipboard {
            self.clipboard.as_ref().unwrap()
        }

        fn write(&self, item: &ClipboardItem) {
            self.clipboard()
                .write_item(item, ClipboardKind::Secondary, WaitConfig::None)
                .unwrap();
        }

        fn read(&self) -> Result<Option<ClipboardItem>, ClipboardReadError> {
            self.clipboard()
                .read_item(ClipboardKind::Secondary)
                .into_result()
        }

        /// Requests one target from the owner, as a second client would.
        fn request(&self, target: &str) -> Result<Vec<u8>, TargetFailure> {
            let inner = &self.clipboard().inner;
            let reader = XContext::new(&inner.display).unwrap();
            X11TargetReader {
                inner,
                reader: &reader,
                selection: ClipboardKind::Secondary,
                lookup: Vec::new(),
            }
            .read(target)
        }
    }

    impl Drop for TestClipboard {
        fn drop(&mut self) {
            // Let the serving window go away so the next test starts without an owner.
            self.clipboard = None;
            wait_for_clipboard_handovers();
        }
    }

    macro_rules! connect_or_skip {
        () => {
            match TestClipboard::connect() {
                Some(clipboard) => clipboard,
                None => return,
            }
        };
    }

    fn alpha_png() -> Image {
        let mut pixels = Vec::new();
        for index in 0..16u8 {
            pixels.extend([index * 16, 255 - index * 8, index, index * 17]);
        }
        vespertine::clipboard::image_tasks::encode_rgba_png(4, 4, &pixels).unwrap()
    }

    fn straight_rgba(image: &Image) -> Vec<u8> {
        let decoded = image
            .to_image_data(vespertine::SvgRenderer::new(std::sync::Arc::new(())))
            .unwrap();
        let mut bytes = decoded.as_bytes(0).unwrap().to_vec();
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        bytes
    }

    #[test]
    fn cross_process_target_read_timeout_is_four_seconds() {
        assert_eq!(LONG_TIMEOUT_DUR, Duration::from_secs(4));
    }

    #[test]
    fn unicode_text_round_trips_through_the_x_server() {
        let clipboard = connect_or_skip!();
        let text = "astral 𝄞 😀, combining e\u{301}, 日本語\nsecond line";
        clipboard.write(&ClipboardItem::new_string(text.into()));
        assert_eq!(
            clipboard.read().unwrap(),
            Some(ClipboardItem::new_string(text.into()))
        );
        // Text targets answer with text bytes, UTF-8 exactly as given.
        for target in ["UTF8_STRING", "text/plain;charset=utf-8", "text/plain"] {
            assert_eq!(
                clipboard.request(target).unwrap(),
                text.as_bytes(),
                "{target}"
            );
        }
        // Nothing else is offered.
        assert_eq!(clipboard.request("image/png"), Err(TargetFailure::Refused));
        assert_eq!(
            clipboard.request(URI_LIST_MIME),
            Err(TargetFailure::Refused)
        );
    }

    #[test]
    fn png_with_alpha_round_trips_through_the_x_server() {
        let clipboard = connect_or_skip!();
        let image = alpha_png();
        clipboard.write(&ClipboardItem::new_image(&image));
        let read = clipboard.read().unwrap().unwrap();
        let read_image = read.images().next().unwrap();
        assert_eq!(read_image.format, ImageFormat::Png);
        assert_eq!(read_image.bytes, image.bytes);
        assert_eq!(read_image.id, image.id);
        assert_eq!(straight_rgba(read_image), straight_rgba(&image));
        // An image target request is answered with image bytes; text targets are absent.
        assert_eq!(clipboard.request("image/png").unwrap(), image.bytes);
        assert_eq!(
            clipboard.request("UTF8_STRING"),
            Err(TargetFailure::Refused)
        );
        assert_eq!(read.text(), None);
    }

    #[test]
    fn jpeg_stays_jpeg_through_the_x_server() {
        let clipboard = connect_or_skip!();
        let image = Image::from_bytes(
            ImageFormat::Jpeg,
            include_bytes!("../../../../vespertine/examples/image/exif-orientation-rotate-180.jpg")
                .to_vec(),
        );
        clipboard.write(&ClipboardItem::new_image(&image));
        let read = clipboard.read().unwrap().unwrap();
        let read_image = read.images().next().unwrap();
        assert_eq!(read_image.format, ImageFormat::Jpeg);
        assert_eq!(read_image.bytes, image.bytes);
        assert_eq!(clipboard.request("image/jpeg").unwrap(), image.bytes);
        assert_eq!(clipboard.request("image/png"), Err(TargetFailure::Refused));
    }

    #[test]
    fn image_with_text_answers_each_target_with_its_own_bytes() {
        let clipboard = connect_or_skip!();
        let image = alpha_png();
        let item = ClipboardItem::from_entries([
            ClipboardEntry::String(ClipboardString::new("alt text".into())),
            ClipboardEntry::Image(image.clone()),
        ]);
        clipboard.write(&item);
        assert_eq!(clipboard.request("text/plain").unwrap(), b"alt text");
        assert_eq!(clipboard.request("image/png").unwrap(), image.bytes);
        let read = clipboard.read().unwrap().unwrap();
        assert_eq!(read, item);
    }

    #[test]
    fn paths_are_offered_as_uri_list_gnome_form_and_text() {
        let clipboard = connect_or_skip!();
        let item = ClipboardItem::new_paths([
            std::path::PathBuf::from("/tmp/a b"),
            std::path::PathBuf::from("/tmp/c"),
        ]);
        clipboard.write(&item);
        assert_eq!(
            clipboard.request(URI_LIST_MIME).unwrap(),
            b"file:///tmp/a%20b\r\nfile:///tmp/c\r\n"
        );
        assert_eq!(
            clipboard.request(GNOME_COPIED_FILES_MIME).unwrap(),
            b"copy\nfile:///tmp/a%20b\nfile:///tmp/c"
        );
        // A plain-text consumer receives the newline-joined paths.
        assert_eq!(
            clipboard.request("UTF8_STRING").unwrap(),
            b"/tmp/a b\n/tmp/c"
        );
        let read = clipboard.read().unwrap().unwrap();
        assert_eq!(
            read.paths().collect::<Vec<_>>(),
            [
                std::path::Path::new("/tmp/a b"),
                std::path::Path::new("/tmp/c")
            ]
        );
        assert_eq!(read.text().as_deref(), Some("/tmp/a b\n/tmp/c"));
    }

    #[test]
    fn metadata_travels_on_its_own_target_without_altering_text() {
        let clipboard = connect_or_skip!();
        let item = ClipboardItem::new_string_with_metadata("t".into(), "{\"v\":1}".into());
        clipboard.write(&item);
        assert_eq!(clipboard.request("UTF8_STRING").unwrap(), b"t");
        assert_eq!(
            clipboard
                .request("application/x-vespertine-clipboard-metadata")
                .unwrap(),
            b"{\"v\":1}"
        );
        assert_eq!(clipboard.read().unwrap().unwrap(), item);
    }

    #[test]
    fn unowned_selection_reads_as_none() {
        let clipboard = connect_or_skip!();
        assert_eq!(clipboard.read(), Ok(None));
    }

    #[test]
    fn the_writing_process_is_detected_as_the_owner() {
        let clipboard = connect_or_skip!();
        assert!(!clipboard.clipboard().is_owner(ClipboardKind::Secondary));
        clipboard.write(&ClipboardItem::new_string("mine".into()));
        assert!(clipboard.clipboard().is_owner(ClipboardKind::Secondary));
    }
}
