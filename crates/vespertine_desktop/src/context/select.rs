//! Target window selection (PLT-013 R1).

use super::{AccessibilityConnection, ContextUnavailable, ObjectRef};
use futures::future::join_all;

/// The identity of a selected window: bus name and path (inside `object`),
/// owning process id and exact title. Selection is repeated after traversal
/// and the result is kept only when the identity is unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WindowIdentity {
    pub object: ObjectRef,
    pub process_id: u32,
    pub title: String,
}

pub(crate) struct Selection {
    pub window: WindowIdentity,
    pub application_name: String,
}

/// Selects the one top-level window of `process_id`'s applications whose
/// accessible name equals `title` exactly and that is active or focused.
///
/// Process identity comes from the accessibility bus daemon's connection
/// credentials, never from names. Zero candidates, several candidates, or an
/// inactive candidate yield no selection.
pub(crate) async fn select(
    connection: &dyn AccessibilityConnection,
    process_id: u32,
    title: &str,
) -> Result<Selection, ContextUnavailable> {
    let applications = connection
        .applications()
        .await
        .map_err(|_| ContextUnavailable::UnavailableBus)?;

    // An application that vanished or cannot report its process cannot be the
    // target and is skipped.
    let pids = join_all(
        applications
            .iter()
            .map(|app| connection.process_id(&app.bus_name)),
    )
    .await;
    let matching_apps = applications
        .into_iter()
        .zip(pids)
        .filter_map(|(app, pid)| (pid.ok() == Some(process_id)).then_some(app));

    let mut candidates: Vec<(ObjectRef, ObjectRef)> = Vec::new();
    for app in matching_apps {
        // Windows of a matching application that cannot be listed could hide
        // a duplicate, so this is a bus failure rather than a skip.
        let windows = connection
            .children(&app)
            .await
            .map_err(|_| ContextUnavailable::UnavailableBus)?;
        let names = join_all(windows.iter().map(|window| connection.name(window))).await;
        for (window, name) in windows.into_iter().zip(names) {
            if matches!(&name, Ok(name) if name == title) {
                candidates.push((app.clone(), window));
            }
        }
        if candidates.len() > 1 {
            return Err(ContextUnavailable::Ambiguous);
        }
    }

    let (app, window) = match candidates.pop() {
        Some(candidate) => candidate,
        None => return Err(ContextUnavailable::NoMatch),
    };
    let state = connection
        .state(&window)
        .await
        .map_err(|_| ContextUnavailable::NoMatch)?;
    if !(state.active || state.focused) {
        return Err(ContextUnavailable::NoMatch);
    }
    let application_name = connection.name(&app).await.unwrap_or_default();
    Ok(Selection {
        window: WindowIdentity {
            object: window,
            process_id,
            title: title.to_owned(),
        },
        application_name,
    })
}
