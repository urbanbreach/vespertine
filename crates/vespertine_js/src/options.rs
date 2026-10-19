//! Window options (APP-005 R4, R5, R7): the plain-object configuration a JavaScript caller
//! passes to `init`, validated and mapped onto Vespertine window options.

use serde::Deserialize;
use vespertine::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use vespertine::{
    App, Bounds, Pixels, Point, Size, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, point, px, size,
};

/// Default window size in logical pixels.
pub const DEFAULT_WIDTH: f32 = 800.0;
pub const DEFAULT_HEIGHT: f32 = 600.0;

/// A configuration error reported to the caller of `init`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct OptionsError(pub String);

/// Window options as received from JavaScript. Every field is optional; unknown fields
/// are ignored.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct WindowConfig {
    pub title: Option<String>,
    pub resizable: Option<bool>,
    pub fullscreen: Option<bool>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub min_width: Option<f64>,
    pub min_height: Option<f64>,
    pub titlebar_transparent: Option<bool>,
    /// Plain transparency; selects the transparent background when no explicit kind is set.
    pub transparent: Option<bool>,
    /// `opaque`, `transparent` or `blurred`; wins over `transparent`.
    pub window_background: Option<String>,
    pub focus: Option<bool>,
    pub show: Option<bool>,
    /// Application identity: the Wayland `app_id` and the X11 `WM_CLASS`.
    pub app_id: Option<String>,
    pub layer_shell: Option<LayerShellConfig>,
    /// Titlebar button position of other platforms. Accepted, no effect on Linux.
    pub traffic_light_x: Option<f64>,
    /// See `traffic_light_x`.
    pub traffic_light_y: Option<f64>,
}

/// Wayland layer-shell configuration (APP-005 R7).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LayerShellConfig {
    pub keyboard_interactivity: Option<String>,
    pub namespace: Option<String>,
    pub layer: Option<String>,
    pub anchor: Option<Vec<String>>,
    pub margin: Option<Vec<f64>>,
    pub exclusive_edge: Option<String>,
    pub exclusive_zone: Option<f64>,
}

/// Parses one edge name: case-insensitive after trimming; unknown names are `None`.
pub fn parse_edge(name: &str) -> Option<Anchor> {
    match name.trim().to_ascii_lowercase().as_str() {
        "top" => Some(Anchor::TOP),
        "bottom" => Some(Anchor::BOTTOM),
        "left" => Some(Anchor::LEFT),
        "right" => Some(Anchor::RIGHT),
        _ => None,
    }
}

impl LayerShellConfig {
    /// Applies the defaults and fallbacks of the specification.
    pub fn resolve(&self) -> LayerShellOptions {
        let layer = match self
            .layer
            .as_deref()
            .map(|layer| layer.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("background") => Layer::Background,
            Some("bottom") => Layer::Bottom,
            Some("overlay") => Layer::Overlay,
            _ => Layer::Top,
        };
        let anchor = match self.anchor.as_deref() {
            None | Some([]) => Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            Some(names) => names
                .iter()
                .filter_map(|name| parse_edge(name))
                .fold(Anchor::empty(), |all, edge| all | edge),
        };
        let keyboard_interactivity = match self
            .keyboard_interactivity
            .as_deref()
            .map(|mode| mode.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("on-demand") => KeyboardInteractivity::OnDemand,
            Some("exclusive") => KeyboardInteractivity::Exclusive,
            _ => KeyboardInteractivity::None,
        };
        let margin = match self.margin.as_deref() {
            Some(&[top, right, bottom, left]) => Some((
                px(top as f32),
                px(right as f32),
                px(bottom as f32),
                px(left as f32),
            )),
            _ => None,
        };
        LayerShellOptions {
            namespace: self.namespace.clone().unwrap_or_default(),
            layer,
            anchor,
            exclusive_zone: self.exclusive_zone.map(|zone| px(zone as f32)),
            exclusive_edge: self.exclusive_edge.as_deref().and_then(parse_edge),
            margin,
            keyboard_interactivity,
        }
    }
}

/// The window background kind (APP-005 R4).
fn resolve_background(
    kind: Option<&str>,
    transparent: Option<bool>,
) -> Result<WindowBackgroundAppearance, OptionsError> {
    match kind {
        Some(kind) => match kind.trim().to_ascii_lowercase().as_str() {
            "opaque" => Ok(WindowBackgroundAppearance::Opaque),
            "transparent" => Ok(WindowBackgroundAppearance::Transparent),
            "blurred" => Ok(WindowBackgroundAppearance::Blurred),
            other => Err(OptionsError(format!(
                "windowBackground must be opaque, transparent or blurred, got {other:?}"
            ))),
        },
        None if transparent == Some(true) => Ok(WindowBackgroundAppearance::Transparent),
        None => Ok(WindowBackgroundAppearance::Opaque),
    }
}

fn dimension(name: &str, value: Option<f64>, default: f32) -> Result<f32, OptionsError> {
    match value {
        None => Ok(default),
        Some(v) if v.is_finite() && v > 0.0 => Ok(v as f32),
        Some(v) => Err(OptionsError(format!(
            "{name} must be a positive finite number, got {v}"
        ))),
    }
}

/// Validated window options, ready to be turned into Vespertine window options.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWindow {
    pub title: Option<String>,
    pub size: (f32, f32),
    /// Applied only when both minimum dimensions were given.
    pub min_size: Option<(f32, f32)>,
    pub resizable: bool,
    pub fullscreen: bool,
    pub titlebar_transparent: bool,
    pub background: WindowBackgroundAppearance,
    pub app_id: Option<String>,
    pub focus: bool,
    pub show: bool,
    pub layer_shell: Option<LayerShellOptions>,
}

impl WindowConfig {
    pub fn resolve(&self) -> Result<ResolvedWindow, OptionsError> {
        #[cfg(not(target_os = "linux"))]
        if self.layer_shell.is_some() {
            return Err(OptionsError(
                "layerShell is only supported on Wayland (Linux)".into(),
            ));
        }
        let width = dimension("width", self.width, DEFAULT_WIDTH)?;
        let height = dimension("height", self.height, DEFAULT_HEIGHT)?;
        let min_size = match (self.min_width, self.min_height) {
            (Some(w), Some(h)) => Some((
                dimension("minWidth", Some(w), 0.0)?,
                dimension("minHeight", Some(h), 0.0)?,
            )),
            _ => None,
        };
        Ok(ResolvedWindow {
            title: self.title.clone(),
            size: (width, height),
            min_size,
            resizable: self.resizable.unwrap_or(true),
            fullscreen: self.fullscreen.unwrap_or(false),
            titlebar_transparent: self.titlebar_transparent.unwrap_or(false),
            background: resolve_background(self.window_background.as_deref(), self.transparent)?,
            app_id: self.app_id.clone(),
            focus: self.focus.unwrap_or(true),
            show: self.show.unwrap_or(true),
            layer_shell: self.layer_shell.as_ref().map(LayerShellConfig::resolve),
        })
    }
}

impl ResolvedWindow {
    /// The bounds of the window: a layer-shell surface opens at the origin with the requested
    /// size (the compositor places it from its anchors); a normal window is centered on the
    /// primary display.
    pub fn bounds(&self, cx: &App) -> Bounds<Pixels> {
        let size: Size<Pixels> = size(px(self.size.0), px(self.size.1));
        if self.layer_shell.is_some() {
            Bounds {
                origin: point(px(0.), px(0.)) as Point<Pixels>,
                size,
            }
        } else {
            Bounds::centered(None, size, cx)
        }
    }

    pub fn to_window_options(&self, cx: &App) -> WindowOptions {
        let bounds = self.bounds(cx);
        let layer_shell = self.layer_shell.is_some();
        let kind = match &self.layer_shell {
            #[cfg(target_os = "linux")]
            Some(options) => WindowKind::LayerShell(options.clone()),
            _ => WindowKind::Normal,
        };
        WindowOptions {
            window_bounds: Some(if self.fullscreen && !layer_shell {
                WindowBounds::Fullscreen(bounds)
            } else {
                WindowBounds::Windowed(bounds)
            }),
            titlebar: if layer_shell {
                None
            } else {
                Some(TitlebarOptions {
                    title: self.title.clone().map(Into::into),
                    appears_transparent: self.titlebar_transparent,
                    traffic_light_position: None,
                })
            },
            focus: self.focus,
            show: self.show,
            kind,
            is_resizable: self.resizable,
            window_background: self.background,
            app_id: self.app_id.clone(),
            window_min_size: self.min_size.map(|(w, h)| size(px(w), px(h))),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: &str) -> WindowConfig {
        serde_json::from_str(json).expect("valid options")
    }

    #[test]
    fn defaults_are_800_by_600_resizable_opaque_focused_and_shown() {
        let window = WindowConfig::default().resolve().unwrap();
        assert_eq!(window.size, (800.0, 600.0));
        assert!(window.resizable && window.focus && window.show);
        assert!(!window.fullscreen && !window.titlebar_transparent);
        assert_eq!(window.background, WindowBackgroundAppearance::Opaque);
        assert_eq!(window.min_size, None);
        assert_eq!(window.layer_shell, None);
    }

    #[test]
    fn transparent_without_a_kind_selects_the_transparent_background() {
        let window = config(r#"{"transparent": true}"#).resolve().unwrap();
        assert_eq!(window.background, WindowBackgroundAppearance::Transparent);
        let window = config(r#"{"transparent": false}"#).resolve().unwrap();
        assert_eq!(window.background, WindowBackgroundAppearance::Opaque);
    }

    #[test]
    fn an_explicit_kind_wins_over_plain_transparency() {
        let window = config(r#"{"transparent": true, "windowBackground": "opaque"}"#)
            .resolve()
            .unwrap();
        assert_eq!(window.background, WindowBackgroundAppearance::Opaque);
        let window = config(r#"{"transparent": false, "windowBackground": "blurred"}"#)
            .resolve()
            .unwrap();
        assert_eq!(window.background, WindowBackgroundAppearance::Blurred);
        assert!(
            config(r#"{"windowBackground": "glass"}"#)
                .resolve()
                .is_err()
        );
    }

    #[test]
    fn minimum_size_applies_only_when_both_values_are_given() {
        assert_eq!(
            config(r#"{"minWidth": 100}"#).resolve().unwrap().min_size,
            None
        );
        assert_eq!(
            config(r#"{"minHeight": 100}"#).resolve().unwrap().min_size,
            None
        );
        assert_eq!(
            config(r#"{"minWidth": 100, "minHeight": 50}"#)
                .resolve()
                .unwrap()
                .min_size,
            Some((100.0, 50.0))
        );
    }

    #[test]
    fn invalid_sizes_are_rejected() {
        for json in [
            r#"{"width": 0}"#,
            r#"{"height": -3}"#,
            r#"{"minWidth": 0, "minHeight": 1}"#,
        ] {
            assert!(config(json).resolve().is_err(), "{json}");
        }
    }

    #[test]
    fn titlebar_button_fields_and_unknown_fields_are_accepted() {
        let window = config(r#"{"trafficLightX": 3, "trafficLightY": 4, "extra": 1}"#)
            .resolve()
            .unwrap();
        assert_eq!(window, WindowConfig::default().resolve().unwrap());
    }

    #[test]
    fn layer_shell_defaults() {
        let options = LayerShellConfig::default().resolve();
        assert_eq!(options.namespace, "");
        assert_eq!(options.layer, Layer::Top);
        assert_eq!(options.anchor, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
        assert_eq!(options.exclusive_zone, None);
        assert_eq!(options.exclusive_edge, None);
        assert_eq!(options.margin, None);
        assert_eq!(options.keyboard_interactivity, KeyboardInteractivity::None);
    }

    fn layer(json: &str) -> LayerShellOptions {
        serde_json::from_str::<LayerShellConfig>(json)
            .unwrap()
            .resolve()
    }

    #[test]
    fn layers_parse_and_unknown_values_fall_back_to_top() {
        assert_eq!(layer(r#"{"layer":"background"}"#).layer, Layer::Background);
        assert_eq!(layer(r#"{"layer":"bottom"}"#).layer, Layer::Bottom);
        assert_eq!(layer(r#"{"layer":"top"}"#).layer, Layer::Top);
        assert_eq!(layer(r#"{"layer":"overlay"}"#).layer, Layer::Overlay);
        assert_eq!(layer(r#"{"layer":"sideways"}"#).layer, Layer::Top);
        assert_eq!(layer(r#"{"layer":""}"#).layer, Layer::Top);
    }

    #[test]
    fn edge_names_ignore_case_and_whitespace_and_unknown_names_add_nothing() {
        assert_eq!(
            layer(r#"{"anchor":[" TOP ","Left"]}"#).anchor,
            Anchor::TOP | Anchor::LEFT
        );
        assert_eq!(
            layer(r#"{"anchor":["bottom","diagonal","right"]}"#).anchor,
            Anchor::BOTTOM | Anchor::RIGHT
        );
        assert_eq!(layer(r#"{"anchor":["nowhere"]}"#).anchor, Anchor::empty());
        assert_eq!(
            layer(r#"{"anchor":[]}"#).anchor,
            Anchor::TOP | Anchor::LEFT | Anchor::RIGHT
        );
        assert_eq!(
            layer(r#"{"anchor":["left","right"]}"#).anchor,
            Anchor::LEFT | Anchor::RIGHT
        );
        assert_eq!(
            layer(r#"{"exclusiveEdge":" Bottom"}"#).exclusive_edge,
            Some(Anchor::BOTTOM)
        );
        assert_eq!(layer(r#"{"exclusiveEdge":"nope"}"#).exclusive_edge, None);
    }

    #[test]
    fn exclusive_zone_keeps_zero_negative_and_absent_distinct() {
        assert_eq!(layer(r#"{}"#).exclusive_zone, None);
        assert_eq!(layer(r#"{"exclusiveZone":0}"#).exclusive_zone, Some(px(0.)));
        assert_eq!(
            layer(r#"{"exclusiveZone":-1}"#).exclusive_zone,
            Some(px(-1.))
        );
        assert_eq!(
            layer(r#"{"exclusiveZone":32}"#).exclusive_zone,
            Some(px(32.))
        );
    }

    #[test]
    fn margins_of_other_lengths_are_ignored() {
        assert_eq!(
            layer(r#"{"margin":[1,2,3,4]}"#).margin,
            Some((px(1.), px(2.), px(3.), px(4.)))
        );
        for json in [
            r#"{"margin":[1,2,3]}"#,
            r#"{"margin":[1,2,3,4,5]}"#,
            r#"{"margin":[]}"#,
        ] {
            assert_eq!(layer(json).margin, None, "{json}");
        }
    }

    #[test]
    fn keyboard_interactivity_parses_and_unknown_values_are_none() {
        assert_eq!(
            layer(r#"{"keyboardInteractivity":"on-demand"}"#).keyboard_interactivity,
            KeyboardInteractivity::OnDemand
        );
        assert_eq!(
            layer(r#"{"keyboardInteractivity":"exclusive"}"#).keyboard_interactivity,
            KeyboardInteractivity::Exclusive
        );
        assert_eq!(
            layer(r#"{"keyboardInteractivity":"none"}"#).keyboard_interactivity,
            KeyboardInteractivity::None
        );
        assert_eq!(
            layer(r#"{"keyboardInteractivity":"sometimes"}"#).keyboard_interactivity,
            KeyboardInteractivity::None
        );
    }
}
