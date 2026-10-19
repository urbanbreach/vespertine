//! Messages, error codes and the validated method catalog of the automation protocol (version 1).

use serde_json::{Map, Value, json};
use std::fmt;

/// The protocol version negotiated by `initialize`.
pub const PROTOCOL_VERSION: i64 = 1;

/// Error codes of the protocol. These are the only codes there are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// A wait expired with no match.
    Timeout,
    /// A strict operation found no element, or an element has no painted bounds.
    NotFound,
    /// A strict operation found several elements.
    Ambiguous,
    /// Malformed message, unknown method, invalid parameters or a protocol version mismatch.
    Protocol,
    /// The session is closed.
    Closed,
    /// The connected host lacks the capability, or its mode cannot perform the method.
    Unsupported,
    /// A request a host refuses as unsafe.
    Security,
    /// The request was cancelled by its caller.
    Cancelled,
}

impl ErrorCode {
    /// The wire name of the code.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Timeout => "Timeout",
            ErrorCode::NotFound => "NotFound",
            ErrorCode::Ambiguous => "Ambiguous",
            ErrorCode::Protocol => "Protocol",
            ErrorCode::Closed => "Closed",
            ErrorCode::Unsupported => "Unsupported",
            ErrorCode::Security => "Security",
            ErrorCode::Cancelled => "Cancelled",
        }
    }

    /// Parses a wire name.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "Timeout" => ErrorCode::Timeout,
            "NotFound" => ErrorCode::NotFound,
            "Ambiguous" => ErrorCode::Ambiguous,
            "Protocol" => ErrorCode::Protocol,
            "Closed" => ErrorCode::Closed,
            "Unsupported" => ErrorCode::Unsupported,
            "Security" => ErrorCode::Security,
            "Cancelled" => ErrorCode::Cancelled,
            _ => return None,
        })
    }
}

/// The error class of the automation contract: a code, a message and optional data.
#[derive(Clone, Debug, PartialEq)]
pub struct AutomationError {
    /// The error code.
    pub code: ErrorCode,
    /// A human-readable message.
    pub message: String,
    /// Optional structured data (for example the validation issues of a `Protocol` error or the
    /// match count of an `Ambiguous` error).
    pub data: Option<Value>,
}

impl AutomationError {
    /// An error with no data.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// An error with data.
    pub fn with_data(code: ErrorCode, message: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            message: message.into(),
            data: Some(data),
        }
    }

    /// A `Protocol` error carrying validation issues in `data.issues`.
    pub fn protocol_issues(message: impl Into<String>, issues: Vec<String>) -> Self {
        Self::with_data(ErrorCode::Protocol, message, json!({ "issues": issues }))
    }

    /// The wire form of the error object.
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("code".into(), Value::String(self.code.as_str().into()));
        object.insert("message".into(), Value::String(self.message.clone()));
        if let Some(data) = &self.data {
            object.insert("data".into(), data.clone());
        }
        Value::Object(object)
    }

    /// Parses the wire form of an error object.
    pub fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self {
            code: ErrorCode::parse(object.get("code")?.as_str()?)?,
            message: object.get("message")?.as_str()?.to_owned(),
            data: object.get("data").cloned(),
        })
    }
}

impl fmt::Display for AutomationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for AutomationError {}

/// What a connected host can do. A subset is reported by `initialize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Pointer and keyboard input, focus and scrolling.
    Input,
    /// PNG screenshots of the painted frame.
    Screenshot,
    /// The motion clock.
    Clock,
    /// Tree, text, bounds and selection inspection.
    Tree,
}

impl Capability {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Input => "input",
            Capability::Screenshot => "screenshot",
            Capability::Clock => "clock",
            Capability::Tree => "tree",
        }
    }

    /// Parses a wire name.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "input" => Capability::Input,
            "screenshot" => Capability::Screenshot,
            "clock" => Capability::Clock,
            "tree" => Capability::Tree,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum Ty {
    /// A JSON number with an integral value.
    Int,
    /// Any finite number.
    Number,
    /// A finite number >= 0.
    NonNegative,
    /// A mouse button index 0..=2.
    Button,
    String,
    Bool,
}

#[derive(Debug)]
struct Field {
    name: &'static str,
    ty: Ty,
    required: bool,
}

const fn req(name: &'static str, ty: Ty) -> Field {
    Field {
        name,
        ty,
        required: true,
    }
}

const fn opt(name: &'static str, ty: Ty) -> Field {
    Field {
        name,
        ty,
        required: false,
    }
}

/// The shape of a method's result.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// `{ "ok": true }`
    Ok,
    /// `[x, y]` or `null`
    OffsetOrNull,
    /// A tree node object or `null`
    TreeOrNull,
    /// An array of strings
    Strings,
    /// `{ x, y, width, height }` or `null`
    BoundsOrNull,
    /// A string or `null`
    StringOrNull,
    /// `{ "path": string }`
    Path,
    /// `{ "nowMs": number }`
    NowMs,
    /// The `initialize` result.
    Initialize,
}

/// One method of the catalog.
#[derive(Debug)]
pub struct Method {
    /// The method name.
    pub name: &'static str,
    /// The capability a host needs to perform it; `None` for session methods.
    pub capability: Option<Capability>,
    params: &'static [Field],
    result: Shape,
}

macro_rules! methods {
    ($( $name:literal, $cap:expr, [$($field:expr),* $(,)?], $result:expr; )*) => {
        /// Every method of the catalog.
        pub static METHODS: &[Method] = &[
            $(Method { name: $name, capability: $cap, params: &[$($field),*], result: $result }),*
        ];
    };
}

methods! {
    "initialize", None, [req("protocolVersion", Ty::Int), opt("client", Ty::String)], Shape::Initialize;
    "cancel", None, [req("id", Ty::Int)], Shape::Ok;
    "click", Some(Capability::Input), [req("x", Ty::Number), req("y", Ty::Number), opt("button", Ty::Button), opt("modifiers", Ty::String)], Shape::Ok;
    "mouseDown", Some(Capability::Input), [req("x", Ty::Number), req("y", Ty::Number), opt("button", Ty::Button), opt("modifiers", Ty::String)], Shape::Ok;
    "mouseUp", Some(Capability::Input), [req("x", Ty::Number), req("y", Ty::Number), opt("button", Ty::Button), opt("modifiers", Ty::String)], Shape::Ok;
    "mouseMove", Some(Capability::Input), [req("x", Ty::Number), req("y", Ty::Number), opt("pressedButton", Ty::Button), opt("modifiers", Ty::String)], Shape::Ok;
    "scrollWheel", Some(Capability::Input), [req("x", Ty::Number), req("y", Ty::Number), req("deltaX", Ty::Number), req("deltaY", Ty::Number), opt("modifiers", Ty::String)], Shape::Ok;
    "keystrokes", Some(Capability::Input), [req("keys", Ty::String), opt("elementId", Ty::Int)], Shape::Ok;
    "keyDown", Some(Capability::Input), [req("key", Ty::String), opt("isHeld", Ty::Bool), opt("elementId", Ty::Int)], Shape::Ok;
    "keyUp", Some(Capability::Input), [req("key", Ty::String), opt("elementId", Ty::Int)], Shape::Ok;
    "focus", Some(Capability::Input), [req("elementId", Ty::Int)], Shape::Ok;
    "blur", Some(Capability::Input), [], Shape::Ok;
    "scrollTo", Some(Capability::Input), [req("elementId", Ty::Int), req("x", Ty::Number), req("y", Ty::Number)], Shape::Ok;
    "getScrollOffset", Some(Capability::Tree), [req("elementId", Ty::Int)], Shape::OffsetOrNull;
    "getTree", Some(Capability::Tree), [], Shape::TreeOrNull;
    "getPaintedText", Some(Capability::Tree), [], Shape::Strings;
    "getAllText", Some(Capability::Tree), [], Shape::Strings;
    "getBounds", Some(Capability::Tree), [req("elementId", Ty::Int)], Shape::BoundsOrNull;
    "getSelectedText", Some(Capability::Tree), [], Shape::StringOrNull;
    "clearSelection", Some(Capability::Tree), [], Shape::Ok;
    "screenshot", Some(Capability::Screenshot), [req("path", Ty::String)], Shape::Path;
    "clockPause", Some(Capability::Clock), [], Shape::NowMs;
    "clockResume", Some(Capability::Clock), [], Shape::NowMs;
    "clockSet", Some(Capability::Clock), [req("nowMs", Ty::NonNegative)], Shape::NowMs;
    "clockFastForward", Some(Capability::Clock), [req("deltaMs", Ty::NonNegative)], Shape::NowMs;
}

/// Looks a method up in the catalog.
pub fn method(name: &str) -> Option<&'static Method> {
    METHODS.iter().find(|method| method.name == name)
}

fn check(ty: Ty, value: &Value) -> Result<(), &'static str> {
    match ty {
        Ty::Int => match value.as_f64() {
            Some(number) if number.is_finite() && number.fract() == 0.0 => Ok(()),
            _ => Err("expected an integer"),
        },
        Ty::Number => match value.as_f64() {
            Some(number) if number.is_finite() => Ok(()),
            _ => Err("expected a finite number"),
        },
        Ty::NonNegative => match value.as_f64() {
            Some(number) if number.is_finite() && number >= 0.0 => Ok(()),
            Some(_) => Err("expected a number >= 0"),
            None => Err("expected a number"),
        },
        Ty::Button => match value.as_f64() {
            Some(number) if number.fract() == 0.0 && (0.0..=2.0).contains(&number) => Ok(()),
            _ => Err("expected a button index 0, 1 or 2"),
        },
        Ty::String => value.is_string().then_some(()).ok_or("expected a string"),
        Ty::Bool => value.is_boolean().then_some(()).ok_or("expected a boolean"),
    }
}

impl Method {
    /// Validates request parameters against the catalog. Unknown parameters are rejected.
    pub fn validate_params(&self, params: &Value) -> Result<(), AutomationError> {
        let mut issues = Vec::new();
        let empty = Map::new();
        let object = match params {
            Value::Object(object) => object,
            Value::Null => &empty,
            _ => {
                issues.push("params: expected an object".to_owned());
                &empty
            }
        };
        for field in self.params {
            match object.get(field.name) {
                None | Some(Value::Null) if !field.required => {}
                None | Some(Value::Null) => {
                    issues.push(format!("{}: required", field.name));
                }
                Some(value) => {
                    if let Err(problem) = check(field.ty, value) {
                        issues.push(format!("{}: {problem}", field.name));
                    }
                }
            }
        }
        for key in object.keys() {
            if !self.params.iter().any(|field| field.name == key) {
                issues.push(format!("{key}: unknown parameter"));
            }
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(AutomationError::protocol_issues(
                format!(
                    "invalid parameters for {}: {}",
                    self.name,
                    issues.join("; ")
                ),
                issues,
            ))
        }
    }

    /// Validates a result against the catalog.
    pub fn validate_result(&self, result: &Value) -> Result<(), AutomationError> {
        let ok = match self.result {
            Shape::Ok => result.get("ok") == Some(&Value::Bool(true)),
            Shape::OffsetOrNull => {
                result.is_null()
                    || result.as_array().is_some_and(|pair| {
                        pair.len() == 2
                            && pair
                                .iter()
                                .all(|number| number.is_f64() || number.is_i64() || number.is_u64())
                    })
            }
            Shape::TreeOrNull => result.is_null() || result.get("id").is_some_and(Value::is_u64),
            Shape::Strings => result
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
            Shape::BoundsOrNull => {
                result.is_null()
                    || ["x", "y", "width", "height"]
                        .iter()
                        .all(|key| result.get(key).is_some_and(Value::is_number))
            }
            Shape::StringOrNull => result.is_null() || result.is_string(),
            Shape::Path => result.get("path").is_some_and(Value::is_string),
            Shape::NowMs => result.get("nowMs").is_some_and(Value::is_number),
            Shape::Initialize => {
                result.get("protocolVersion").is_some_and(Value::is_i64)
                    && result.get("pid").is_some_and(Value::is_u64)
                    && result
                        .get("capabilities")
                        .and_then(Value::as_array)
                        .is_some_and(|caps| {
                            caps.iter()
                                .all(|cap| cap.as_str().and_then(Capability::parse).is_some())
                        })
                    && result.get("width").is_some_and(Value::is_number)
                    && result.get("height").is_some_and(Value::is_number)
            }
        };
        if ok {
            Ok(())
        } else {
            Err(AutomationError::protocol_issues(
                format!("invalid result for {}", self.name),
                vec![format!(
                    "result does not match the {} result shape: {result}",
                    self.name
                )],
            ))
        }
    }
}

/// A parsed request envelope with a known method.
#[derive(Debug)]
pub struct Request {
    /// The request id.
    pub id: i64,
    /// The catalog method.
    pub method: &'static Method,
    /// The validated parameters (an object; `null` is normalized to `{}`).
    pub params: Value,
}

/// A message of the protocol.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// A request: `{ id, method, params }`.
    Request {
        /// The raw id (an integer for a valid request).
        id: Value,
        /// The method name.
        method: String,
        /// The raw parameters.
        params: Value,
    },
    /// A response: `{ id, result }` or `{ id, error }`.
    Response {
        /// The id of the request being answered; `null` when the request had no valid id.
        id: Value,
        /// The outcome.
        outcome: Result<Value, AutomationError>,
    },
    /// A server-originated event: `{ event, params }`.
    Event {
        /// The event name (`console`, `frame`, `closed`, ...).
        name: String,
        /// The event parameters.
        params: Value,
    },
}

impl Message {
    /// The JSON object of the message.
    pub fn to_json(&self) -> Value {
        match self {
            Message::Request { id, method, params } => {
                json!({ "id": id, "method": method, "params": params })
            }
            Message::Response { id, outcome } => match outcome {
                Ok(result) => json!({ "id": id, "result": result }),
                Err(error) => json!({ "id": id, "error": error.to_json() }),
            },
            Message::Event { name, params } => json!({ "event": name, "params": params }),
        }
    }

    /// Interprets a decoded JSON value as a message; `None` when it is not one.
    pub fn from_json(value: &Value) -> Option<Message> {
        let object = value.as_object()?;
        if let Some(event) = object.get("event") {
            return Some(Message::Event {
                name: event.as_str()?.to_owned(),
                params: object.get("params").cloned().unwrap_or(Value::Null),
            });
        }
        let id = object.get("id").cloned().unwrap_or(Value::Null);
        if let Some(method) = object.get("method") {
            return Some(Message::Request {
                id,
                method: method.as_str()?.to_owned(),
                params: object.get("params").cloned().unwrap_or(Value::Null),
            });
        }
        if let Some(error) = object.get("error") {
            return Some(Message::Response {
                id,
                outcome: Err(AutomationError::from_json(error)?),
            });
        }
        object.get("result").map(|result| Message::Response {
            id,
            outcome: Ok(result.clone()),
        })
    }
}

/// Validates a request message: integer id, known method, valid parameters. On failure the
/// error is a `Protocol` error carrying the validation issues.
pub fn validate_request(
    id: &Value,
    method_name: &str,
    params: &Value,
) -> Result<Request, AutomationError> {
    let id = match id.as_i64() {
        Some(id) => id,
        None => match id.as_f64() {
            Some(number) if number.fract() == 0.0 && number.abs() < 9.0e15 => number as i64,
            _ => {
                return Err(AutomationError::protocol_issues(
                    "the request id must be an integer",
                    vec![format!("id: expected an integer, got {id}")],
                ));
            }
        },
    };
    let method = method(method_name).ok_or_else(|| {
        AutomationError::protocol_issues(
            format!("unknown method {method_name:?}"),
            vec![format!("method: unknown method {method_name:?}")],
        )
    })?;
    method.validate_params(params)?;
    Ok(Request {
        id,
        method,
        params: if params.is_null() {
            Value::Object(Map::new())
        } else {
            params.clone()
        },
    })
}
