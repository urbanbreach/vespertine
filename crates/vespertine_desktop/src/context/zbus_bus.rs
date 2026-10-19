//! The real AT-SPI accessibility bus over zbus (Linux).
//!
//! The accessibility bus address comes from the session bus `org.a11y.Bus`
//! service; the registry root is the `org.a11y.atspi.Registry` accessible whose
//! children are applications; process ids come from the accessibility bus
//! daemon's connection credentials.

use super::{AccessibilityBus, AccessibilityConnection, BusError, NodeRole, NodeState, ObjectRef};
use futures::future::{BoxFuture, FutureExt};
use std::sync::Arc;
use zbus::{
    Connection, Message,
    zvariant::{OwnedObjectPath, OwnedValue},
};

const ATSPI_REGISTRY_BUS: &str = "org.a11y.atspi.Registry";
const ATSPI_REGISTRY_ROOT: &str = "/org/a11y/atspi/accessible/root";
const ATSPI_ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const ATSPI_TEXT: &str = "org.a11y.atspi.Text";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const A11Y_BUS: &str = "org.a11y.Bus";
const A11Y_BUS_PATH: &str = "/org/a11y/bus";

/// `ROLE_PASSWORD_TEXT` in the AT-SPI role enumeration.
const ROLE_PASSWORD_TEXT: u32 = 40;
/// `STATE_ACTIVE` and `STATE_FOCUSED` in the AT-SPI state enumeration.
const STATE_ACTIVE: u32 = 1;
const STATE_FOCUSED: u32 = 12;

fn bus_error(error: impl std::fmt::Display) -> BusError {
    BusError(error.to_string())
}

pub(crate) async fn call_raw<B>(
    connection: &Connection,
    destination: &str,
    path: &str,
    interface: &str,
    member: &str,
    body: &B,
) -> Result<Message, BusError>
where
    B: serde_body::Body,
{
    connection
        .call_method(Some(destination), path, Some(interface), member, body)
        .await
        .map_err(bus_error)
}

/// Marker bound that names the serde/zvariant requirements of a method body
/// without a direct `serde` dependency.
pub(crate) mod serde_body {
    pub trait Body: zbus::export::serde::ser::Serialize + zbus::zvariant::DynamicType {}
    impl<T> Body for T where T: zbus::export::serde::ser::Serialize + zbus::zvariant::DynamicType {}
}

pub(crate) async fn get_property(
    connection: &Connection,
    destination: &str,
    path: &str,
    interface: &str,
    property: &str,
) -> Result<OwnedValue, BusError> {
    let reply = call_raw(
        connection,
        destination,
        path,
        PROPERTIES,
        "Get",
        &(interface, property),
    )
    .await?;
    reply.body().deserialize::<OwnedValue>().map_err(bus_error)
}

/// Connects to the session bus.
pub(crate) async fn session_connection() -> Result<Connection, BusError> {
    Connection::session().await.map_err(bus_error)
}

/// Asks `org.a11y.Bus` on the session bus for the accessibility bus address.
pub(crate) async fn accessibility_bus_address(session: &Connection) -> Result<String, BusError> {
    let reply = call_raw(
        session,
        A11Y_BUS,
        A11Y_BUS_PATH,
        A11Y_BUS,
        "GetAddress",
        &(),
    )
    .await?;
    let address = reply.body().deserialize::<String>().map_err(bus_error)?;
    if address.is_empty() {
        return Err(BusError("empty accessibility bus address".into()));
    }
    Ok(address)
}

/// Opens a private connection to the accessibility bus.
pub(crate) async fn connect_accessibility_bus() -> Result<Connection, BusError> {
    let session = session_connection().await?;
    let address = accessibility_bus_address(&session).await?;
    zbus::connection::Builder::address(address.as_str())
        .map_err(bus_error)?
        .build()
        .await
        .map_err(bus_error)
}

/// Whether the accessibility bus can be reached (address obtainable and a
/// connection established). Performs no writes.
pub fn probe_bus_available() -> BoxFuture<'static, bool> {
    async { connect_accessibility_bus().await.is_ok() }.boxed()
}

/// The real accessibility bus.
#[derive(Clone, Copy, Debug, Default)]
pub struct ZbusAccessibilityBus;

impl AccessibilityBus for ZbusAccessibilityBus {
    fn connect(&self) -> BoxFuture<'static, Result<Arc<dyn AccessibilityConnection>, BusError>> {
        async {
            let connection = connect_accessibility_bus().await?;
            Ok(Arc::new(ZbusConnection { connection }) as Arc<dyn AccessibilityConnection>)
        }
        .boxed()
    }
}

struct ZbusConnection {
    connection: Connection,
}

impl ZbusConnection {
    async fn accessible_call<B: serde_body::Body>(
        &self,
        node: &ObjectRef,
        interface: &str,
        member: &str,
        body: &B,
    ) -> Result<Message, BusError> {
        call_raw(
            &self.connection,
            &node.bus_name,
            &node.path,
            interface,
            member,
            body,
        )
        .await
    }

    async fn accessible_property(
        &self,
        node: &ObjectRef,
        interface: &str,
        property: &str,
    ) -> Result<OwnedValue, BusError> {
        get_property(
            &self.connection,
            &node.bus_name,
            &node.path,
            interface,
            property,
        )
        .await
    }
}

fn object_refs(entries: Vec<(String, OwnedObjectPath)>) -> Vec<ObjectRef> {
    entries
        .into_iter()
        .map(|(bus_name, path)| ObjectRef::new(bus_name, path.as_str()))
        .collect()
}

fn state_bit(words: &[u32], bit: u32) -> bool {
    words
        .get((bit / 32) as usize)
        .is_some_and(|word| word & (1 << (bit % 32)) != 0)
}

impl AccessibilityConnection for ZbusConnection {
    fn applications(&self) -> BoxFuture<'_, Result<Vec<ObjectRef>, BusError>> {
        async move {
            let root = ObjectRef::new(ATSPI_REGISTRY_BUS, ATSPI_REGISTRY_ROOT);
            self.children(&root).await
        }
        .boxed()
    }

    fn process_id<'a>(&'a self, bus_name: &'a str) -> BoxFuture<'a, Result<u32, BusError>> {
        async move {
            let reply = call_raw(
                &self.connection,
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "GetConnectionUnixProcessID",
                &(bus_name,),
            )
            .await?;
            reply.body().deserialize::<u32>().map_err(bus_error)
        }
        .boxed()
    }

    fn children<'a>(
        &'a self,
        node: &'a ObjectRef,
    ) -> BoxFuture<'a, Result<Vec<ObjectRef>, BusError>> {
        async move {
            let reply = self
                .accessible_call(node, ATSPI_ACCESSIBLE, "GetChildren", &())
                .await?;
            let entries = reply
                .body()
                .deserialize::<Vec<(String, OwnedObjectPath)>>()
                .map_err(bus_error)?;
            Ok(object_refs(entries))
        }
        .boxed()
    }

    fn name<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<String, BusError>> {
        async move {
            let value = self
                .accessible_property(node, ATSPI_ACCESSIBLE, "Name")
                .await?;
            String::try_from(value).map_err(bus_error)
        }
        .boxed()
    }

    fn description<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<String, BusError>> {
        async move {
            let value = self
                .accessible_property(node, ATSPI_ACCESSIBLE, "Description")
                .await?;
            String::try_from(value).map_err(bus_error)
        }
        .boxed()
    }

    fn role<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<NodeRole, BusError>> {
        async move {
            let (role, name) = futures::join!(
                self.accessible_call(node, ATSPI_ACCESSIBLE, "GetRole", &()),
                self.accessible_call(node, ATSPI_ACCESSIBLE, "GetRoleName", &()),
            );
            // Both answers are required: an unknown role could be a password
            // field, so a missing role never allows reading text.
            let role = role?.body().deserialize::<u32>().map_err(bus_error)?;
            let name = name?.body().deserialize::<String>().map_err(bus_error)?;
            Ok(NodeRole {
                password: role == ROLE_PASSWORD_TEXT,
                name,
            })
        }
        .boxed()
    }

    fn state<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<NodeState, BusError>> {
        async move {
            let reply = self
                .accessible_call(node, ATSPI_ACCESSIBLE, "GetState", &())
                .await?;
            let words = reply.body().deserialize::<Vec<u32>>().map_err(bus_error)?;
            Ok(NodeState {
                active: state_bit(&words, STATE_ACTIVE),
                focused: state_bit(&words, STATE_FOCUSED),
            })
        }
        .boxed()
    }

    fn implements_text<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<bool, BusError>> {
        async move {
            let reply = self
                .accessible_call(node, ATSPI_ACCESSIBLE, "GetInterfaces", &())
                .await?;
            let interfaces = reply
                .body()
                .deserialize::<Vec<String>>()
                .map_err(bus_error)?;
            Ok(interfaces.iter().any(|interface| interface == ATSPI_TEXT))
        }
        .boxed()
    }

    fn character_count<'a>(
        &'a self,
        node: &'a ObjectRef,
    ) -> BoxFuture<'a, Result<usize, BusError>> {
        async move {
            let value = self
                .accessible_property(node, ATSPI_TEXT, "CharacterCount")
                .await?;
            let count = i32::try_from(value).map_err(bus_error)?;
            Ok(count.max(0) as usize)
        }
        .boxed()
    }

    fn text<'a>(
        &'a self,
        node: &'a ObjectRef,
        start: usize,
        end: usize,
    ) -> BoxFuture<'a, Result<String, BusError>> {
        async move {
            let start = i32::try_from(start).map_err(bus_error)?;
            let end = i32::try_from(end).map_err(bus_error)?;
            let reply = self
                .accessible_call(node, ATSPI_TEXT, "GetText", &(start, end))
                .await?;
            reply.body().deserialize::<String>().map_err(bus_error)
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::state_bit;

    #[test]
    fn state_bits_decode_active_and_focused() {
        assert!(state_bit(&[1 << 1], 1));
        assert!(!state_bit(&[1 << 1], 12));
        assert!(state_bit(&[1 << 12], 12));
        assert!(!state_bit(&[], 1));
        assert!(state_bit(&[0, 1 << 1], 33));
    }
}
