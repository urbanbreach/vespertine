use std::default::Default;

use x11rb::protocol::{Event, xproto};
use xim::{AHashMap, AttributeName, Client, ClientError, ClientHandler, InputStyle};

pub enum XimCallbackEvent {
    XimXEvent(x11rb::protocol::Event),
    XimPreeditEvent(xproto::Window, String),
    XimCommitEvent(xproto::Window, String),
}

pub struct XimHandler {
    pub im_id: u16,
    pub ic_id: u16,
    pub connected: bool,
    pub window: xproto::Window,
    pub last_callback_event: Option<XimCallbackEvent>,
    /// The server's open reply has assigned `im_id`. Input contexts may only be
    /// created once this is true: before it, `im_id` holds its default value and
    /// a create request using it makes the server reject the connection.
    open_completed: bool,
    /// A focus-in has been seen; `window` is the most recently focused window.
    focused: bool,
    /// A context-creation request has been issued since the handshake began.
    ic_requested: bool,
}

impl XimHandler {
    pub fn new() -> Self {
        Self {
            im_id: Default::default(),
            ic_id: Default::default(),
            connected: false,
            window: Default::default(),
            last_callback_event: None,
            open_completed: false,
            focused: false,
            ic_requested: false,
        }
    }

    /// Records the latest focused window without creating a context.
    pub fn note_focus(&mut self, window: xproto::Window) {
        self.window = window;
        self.focused = true;
    }

    /// Whether the handshake has completed far enough to create contexts.
    pub fn can_create_ic(&self) -> bool {
        self.open_completed
    }

    /// The server's open reply assigned `input_method_id`.
    fn on_open_reply(&mut self, input_method_id: u16) {
        self.im_id = input_method_id;
        self.open_completed = true;
    }

    /// The window to create the handshake's context for, once the input-style
    /// query has been answered: the latest focused window. `None` when no window
    /// has been focused (the first focus-in creates the context) or when a
    /// focus-in already created one, so exactly one request is issued.
    fn handshake_ic_window(&mut self) -> Option<xproto::Window> {
        if !self.focused || self.ic_requested {
            return None;
        }
        self.ic_requested = true;
        Some(self.window)
    }

    /// Records that a context-creation request is being issued.
    pub fn note_ic_requested(&mut self) {
        self.ic_requested = true;
    }
}

impl<C: Client<XEvent = xproto::KeyPressEvent>> ClientHandler<C> for XimHandler {
    fn handle_connect(&mut self, client: &mut C) -> Result<(), ClientError> {
        client.open("C")
    }

    fn handle_open(&mut self, client: &mut C, input_method_id: u16) -> Result<(), ClientError> {
        self.on_open_reply(input_method_id);

        client.get_im_values(input_method_id, &[AttributeName::QueryInputStyle])
    }

    fn handle_get_im_values(
        &mut self,
        client: &mut C,
        input_method_id: u16,
        _attributes: AHashMap<AttributeName, Vec<u8>>,
    ) -> Result<(), ClientError> {
        let Some(window) = self.handshake_ic_window() else {
            return Ok(());
        };
        let ic_attributes = client
            .build_ic_attributes()
            .push(AttributeName::InputStyle, InputStyle::PREEDIT_CALLBACKS)
            .push(AttributeName::ClientWindow, window)
            .push(AttributeName::FocusWindow, window)
            .build();
        client.create_ic(input_method_id, ic_attributes)
    }

    fn handle_create_ic(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        input_context_id: u16,
    ) -> Result<(), ClientError> {
        self.connected = true;
        self.ic_id = input_context_id;
        Ok(())
    }

    fn handle_commit(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        _input_context_id: u16,
        text: &str,
    ) -> Result<(), ClientError> {
        self.last_callback_event = Some(XimCallbackEvent::XimCommitEvent(
            self.window,
            String::from(text),
        ));
        Ok(())
    }

    fn handle_forward_event(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        _input_context_id: u16,
        _flag: xim::ForwardEventFlag,
        xev: C::XEvent,
    ) -> Result<(), ClientError> {
        match xev.response_type {
            x11rb::protocol::xproto::KEY_PRESS_EVENT => {
                self.last_callback_event = Some(XimCallbackEvent::XimXEvent(Event::KeyPress(xev)));
            }
            x11rb::protocol::xproto::KEY_RELEASE_EVENT => {
                self.last_callback_event =
                    Some(XimCallbackEvent::XimXEvent(Event::KeyRelease(xev)));
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_close(&mut self, client: &mut C, _input_method_id: u16) -> Result<(), ClientError> {
        client.disconnect()
    }

    fn handle_preedit_draw(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        _input_context_id: u16,
        _caret: i32,
        _chg_first: i32,
        _chg_len: i32,
        _status: xim::PreeditDrawStatus,
        preedit_string: &str,
        _feedbacks: Vec<xim::Feedback>,
    ) -> Result<(), ClientError> {
        // XIMReverse: 1, XIMPrimary: 8, XIMTertiary: 32: selected text
        // XIMUnderline: 2, XIMSecondary: 16: underlined text
        // XIMHighlight: 4: normal text
        // XIMVisibleToForward: 64, XIMVisibleToBackward: 128, XIMVisibleCenter: 256: text align position
        // XIMPrimary, XIMHighlight, XIMSecondary, XIMTertiary are not specified,
        // but interchangeable as above
        // Currently there's no way to support these.
        self.last_callback_event = Some(XimCallbackEvent::XimPreeditEvent(
            self.window,
            String::from(preedit_string),
        ));
        Ok(())
    }
}

/// Whether a context may be created now: the handler exists (it is absent when
/// no input method is available or while it is taken during event processing)
/// and its handshake has completed.
pub(crate) fn ime_ready(handler: Option<&XimHandler>) -> bool {
    handler.is_some_and(XimHandler::can_create_ic)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Models the focus-in path: returns the window a context is created for.
    fn focus_in(handler: &mut XimHandler, window: xproto::Window) -> Option<xproto::Window> {
        handler.note_focus(window);
        if handler.can_create_ic() {
            handler.note_ic_requested();
            Some(window)
        } else {
            None
        }
    }

    #[test]
    fn focus_during_handshake_creates_one_context_for_latest_window() {
        let mut handler = XimHandler::new();
        assert_eq!(focus_in(&mut handler, 10), None);
        assert_eq!(focus_in(&mut handler, 20), None);
        assert!(!handler.can_create_ic());

        handler.on_open_reply(7);
        assert_eq!(handler.im_id, 7);
        assert!(handler.can_create_ic());
        assert_eq!(handler.handshake_ic_window(), Some(20));
        assert_eq!(handler.handshake_ic_window(), None);
    }

    #[test]
    fn no_context_without_focus_until_first_focus_in() {
        let mut handler = XimHandler::new();
        handler.on_open_reply(3);
        assert_eq!(handler.handshake_ic_window(), None);
        assert_eq!(focus_in(&mut handler, 5), Some(5));
    }

    #[test]
    fn focus_after_handshake_uses_normal_path() {
        let mut handler = XimHandler::new();
        handler.on_open_reply(3);
        assert_eq!(handler.handshake_ic_window(), None);
        assert_eq!(focus_in(&mut handler, 5), Some(5));
        assert_eq!(focus_in(&mut handler, 6), Some(6));
    }

    #[test]
    fn focus_between_open_and_input_style_reply_is_not_duplicated() {
        let mut handler = XimHandler::new();
        handler.on_open_reply(3);
        assert_eq!(focus_in(&mut handler, 5), Some(5));
        assert_eq!(handler.handshake_ic_window(), None);
    }

    #[test]
    fn absent_handler_is_not_ready() {
        assert!(!ime_ready(None));
        let mut handler = XimHandler::new();
        assert!(!ime_ready(Some(&handler)));
        handler.on_open_reply(1);
        assert!(ime_ready(Some(&handler)));
    }
}
