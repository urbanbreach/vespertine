use crate::{Bounds, DisplayId, Pixels, PlatformDisplay, Point, px};
use anyhow::{Ok, Result};
use std::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct TestDisplay {
    id: DisplayId,
    uuid: uuid::Uuid,
    bounds: Bounds<Pixels>,
    /// The work area; the full bounds when unset.
    visible_bounds: Option<Bounds<Pixels>>,
    pub(crate) refresh_interval: Option<Duration>,
}

impl TestDisplay {
    pub fn new() -> Self {
        Self::with_id(DisplayId(1))
    }

    pub fn with_id(id: DisplayId) -> Self {
        TestDisplay {
            id,
            uuid: uuid::Uuid::new_v4(),
            bounds: Bounds::from_corners(Point::default(), Point::new(px(1920.), px(1080.))),
            visible_bounds: None,
            refresh_interval: Some(Duration::from_secs(1) / 60),
        }
    }

    /// A display with the given identity, bounds and work area.
    pub fn with_geometry(
        id: DisplayId,
        uuid: uuid::Uuid,
        bounds: Bounds<Pixels>,
        visible_bounds: Bounds<Pixels>,
    ) -> Self {
        TestDisplay {
            uuid,
            bounds,
            visible_bounds: Some(visible_bounds),
            ..Self::with_id(id)
        }
    }
}

impl PlatformDisplay for TestDisplay {
    fn id(&self) -> crate::DisplayId {
        self.id
    }

    fn uuid(&self) -> Result<uuid::Uuid> {
        Ok(self.uuid)
    }

    fn bounds(&self) -> crate::Bounds<crate::Pixels> {
        self.bounds
    }

    fn visible_bounds(&self) -> Bounds<Pixels> {
        self.visible_bounds.unwrap_or(self.bounds)
    }

    fn refresh_interval(&self) -> Option<Duration> {
        self.refresh_interval
    }
}
