//! Startup-notification ids for activating X11 windows (PLT-008 R7).

/// The server timestamp encoded in a startup-notification id, which has the form
/// `<unique>_TIME<timestamp>`. The window manager compares it with the timestamp of the user's
/// last input to decide whether the activated window may take focus.
pub(crate) fn startup_id_timestamp(startup_id: &str) -> Option<u32> {
    let (_, timestamp) = startup_id.rsplit_once("_TIME")?;
    timestamp.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_follows_the_last_time_marker() {
        assert_eq!(
            startup_id_timestamp("myapp-1234-host_TIME987654"),
            Some(987654)
        );
        assert_eq!(startup_id_timestamp("a_TIME1_TIME22"), Some(22));
    }

    #[test]
    fn ids_without_a_valid_timestamp_have_none() {
        assert_eq!(startup_id_timestamp("no-marker"), None);
        assert_eq!(startup_id_timestamp("id_TIME"), None);
        assert_eq!(startup_id_timestamp("id_TIMEabc"), None);
        assert_eq!(startup_id_timestamp("id_TIME99999999999"), None);
    }
}
