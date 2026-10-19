//! Floating surfaces, anchored menus, popovers and dismissal layers (CMP-002).
//!
//! The foundation under every popup control. The pure, reusable rules live in their own
//! modules and carry no framework state:
//!
//! * [`stepping`]: keyboard stepping over lists with unavailable rows (R23).
//! * [`keys`]: key classification shared by menus, selects, comboboxes and pickers (R22).
//! * [`rank`]: search match rank and filter-and-rank returning original indices (R25).
//! * [`hover_intent`]: submenu hover intent with the pointer corridor (R26, R27).
//! * [`placement`]: contained placement inside caller-supplied bounds (R18, R19).
//! * [`controlled`]: the controlled-state helper (R12).
//!
//! and the parts that touch the framework:
//!
//! * [`lifecycle`]: the unmounted/open/closing popup value, press note and reap (R13, R15).
//! * [`motion`]: entrance and exit motion, reduced motion (R16).
//! * [`dismiss`]: the window-scoped dismissal-layer registry and Escape ownership (R6-R8).
//! * [`focus`]: opening and closing focus, the focus-target contract (R9-R11).
//! * [`trigger`]: trigger bounds and the outside-press probe (R13, R14).
//! * [`menu`]: anchored and contained menu surfaces (R1, R2, R5, R16, R18-R21).

pub mod controlled;
pub mod dismiss;
pub mod focus;
pub mod hover_intent;
pub mod keys;
pub mod lifecycle;
pub mod menu;
pub mod motion;
pub mod placement;
pub mod popup_menu;
pub mod rank;
pub mod stepping;
pub mod trigger;

pub use controlled::ControlledState;
pub use hover_intent::{
    CORRIDOR_TOLERANCE, HOVER_GRACE, HoverIntent, IntentAnswer, OpenSubmenu, RENEW_THRESHOLD,
    TURN_AWAY_THRESHOLD, in_corridor,
};
pub use keys::{ListKey, classify_key};
pub use placement::{
    CONTAINED_GAP, CONTAINMENT_INSET, ContainedAlign, ContainedPlacement, MAX_CONTAINED_HEIGHT,
    contained_max_size, containment_limits, place_contained, rows_max_height,
};
pub use rank::{filter_and_rank, match_rank};
pub use stepping::{Edge, StepDirection, edge_available, step_available, step_slice};

pub use dismiss::{
    EscapeHandler, EscapeOutcome, LayerId, is_top_layer, layers, press_is_inside, register_layer,
    set_layer_bounds, top_layer, unregister_layer,
};
pub use focus::{FocusPick, FocusTarget, LayerHandle, LayerOptions, open_layer};
pub use lifecycle::{Popup, close_popup, schedule_reap};
pub use menu::{
    AnchoredMenu, BESIDE_OFFSET, MENU_PADDING, NESTED_PRIORITY, Placement, ROW_GAP, TOP_PRIORITY,
    TRIGGER_CLEARANCE, WINDOW_MARGIN, anchored_menu, anchored_menu_above, anchored_menu_at,
    anchored_menu_below, anchored_menu_below_end, anchored_menu_below_with, anchored_menu_beside,
    anchored_menu_nested, contained_menu,
};
pub use motion::{
    ENTER_MS, EXIT_MS, REAP_SLACK_MS, SETTLE_TRAVEL, SIDE_TRAVEL, Side, SurfaceMotion,
    enter_progress, exit_progress, reap_delay,
};
pub use popup_menu::{BoundsObserver, MenuPart, PopupMenu, PopupMenuItem, menu_item_row};
pub use trigger::{OutsidePress, TriggerBounds};
