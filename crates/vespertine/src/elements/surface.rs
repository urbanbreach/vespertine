use crate::{
    App, Bounds, DevicePixels, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, ObjectFit, Pixels, Size, Style, StyleRefinement, Styled, Window,
};
#[cfg(target_os = "macos")]
use objc2_core_foundation::CFRetained;
#[cfg(target_os = "macos")]
use objc2_core_video::{CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth};
use std::{any::Any, fmt, sync::Arc};
use vespertine_refineable::Refineable;

/// A source of a surface's content.
#[derive(Clone)]
pub enum SurfaceSource {
    /// A CoreVideo image buffer
    #[cfg(target_os = "macos")]
    Surface(CFRetained<CVPixelBuffer>),
    /// A shared, type-erased GPU texture created on the window's own device (see
    /// [`Window::gpu_context`]) together with its size in device pixels.
    ///
    /// Cloning retains shared ownership of the same texture. The Linux renderer samples it as
    /// a single-sample filterable 2D RGBA texture (mip level zero) holding straight-alpha,
    /// sRGB-encoded values; other platforms draw nothing for this variant.
    GpuTexture {
        /// The erased texture handle. Its concrete type is the graphics library's texture
        /// type; any other type is skipped at render time.
        texture: Arc<dyn Any + Send + Sync>,
        /// Texture dimensions in device pixels.
        size: Size<DevicePixels>,
    },
}

impl SurfaceSource {
    /// Wraps a shared GPU texture of `size` device pixels. No pixel data is copied.
    pub fn gpu_texture(texture: Arc<dyn Any + Send + Sync>, size: Size<DevicePixels>) -> Self {
        SurfaceSource::GpuTexture { texture, size }
    }
}

impl PartialEq for SurfaceSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            #[cfg(target_os = "macos")]
            (SurfaceSource::Surface(a), SurfaceSource::Surface(b)) => a == b,
            (
                SurfaceSource::GpuTexture {
                    texture: a,
                    size: a_size,
                },
                SurfaceSource::GpuTexture {
                    texture: b,
                    size: b_size,
                },
            ) => Arc::as_ptr(a).cast::<()>() == Arc::as_ptr(b).cast::<()>() && a_size == b_size,
            #[cfg(target_os = "macos")]
            _ => false,
        }
    }
}

impl Eq for SurfaceSource {}

impl fmt::Debug for SurfaceSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceSource::Surface(buffer) => f.debug_tuple("Surface").field(buffer).finish(),
            SurfaceSource::GpuTexture { texture, size } => f
                .debug_struct("GpuTexture")
                .field("texture", &Arc::as_ptr(texture).cast::<()>())
                .field("size", size)
                .finish(),
        }
    }
}

#[cfg(target_os = "macos")]
impl From<CFRetained<CVPixelBuffer>> for SurfaceSource {
    fn from(value: CFRetained<CVPixelBuffer>) -> Self {
        SurfaceSource::Surface(value)
    }
}

/// A surface element.
///
/// Corner radii are not applied to the sampled texture; the visible area is the object-fit
/// rectangle intersected with the content mask, both axis-aligned.
pub struct Surface {
    source: SurfaceSource,
    object_fit: ObjectFit,
    style: StyleRefinement,
}

/// Create a new surface element.
pub fn surface(source: impl Into<SurfaceSource>) -> Surface {
    Surface {
        source: source.into(),
        object_fit: ObjectFit::Contain,
        style: Default::default(),
    }
}

impl Surface {
    /// Set the object fit for the image.
    pub fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        self.object_fit = object_fit;
        self
    }
}

impl Element for Surface {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.refine(&self.style);
        let layout_id = window.request_layout(style, [], cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        match &self.source {
            #[cfg(target_os = "macos")]
            SurfaceSource::Surface(surface) => {
                let size = crate::size(
                    CVPixelBufferGetWidth(surface).into(),
                    CVPixelBufferGetHeight(surface).into(),
                );
                let new_bounds = self.object_fit.get_bounds(bounds, size);
                // Corner radii are not applied to surfaces.
                window.paint_surface(new_bounds, surface.clone());
            }
            #[cfg(not(target_os = "macos"))]
            SurfaceSource::GpuTexture { size, .. } => {
                if size.width.0 <= 0 || size.height.0 <= 0 {
                    return;
                }
                let new_bounds = self.object_fit.get_bounds(bounds, *size);
                // Fits larger than the element (cover, none) are clipped by the element's own
                // bounds, as for images. Corner radii are not applied to surfaces.
                window.with_content_mask(Some(crate::ContentMask { bounds }), |window| {
                    window.paint_surface(new_bounds, self.source.clone());
                });
            }
            // The macOS renderer composites only video buffers; textures from a shared GPU
            // device do not exist there.
            #[cfg(target_os = "macos")]
            SurfaceSource::GpuTexture { .. } => {}
        }
    }
}

impl IntoElement for Surface {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for Surface {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use crate::{
        AnyElement, Bounds, ContentMask, DevicePixels, ParentElement as _, ScaledPixels,
        TestAppContext, VisualTestContext, div, point, px, size,
    };
    use std::sync::Weak;

    struct Texture;

    fn source(width: i32, height: i32) -> SurfaceSource {
        SurfaceSource::gpu_texture(
            Arc::new(Texture),
            size(DevicePixels(width), DevicePixels(height)),
        )
    }

    fn scaled(origin: (i32, i32), extent: (i32, i32)) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(origin.0 as f32), ScaledPixels(origin.1 as f32)),
            size: size(ScaledPixels(extent.0 as f32), ScaledPixels(extent.1 as f32)),
        }
    }

    fn draw(window: &mut VisualTestContext, element: impl FnOnce() -> AnyElement) {
        window.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
            element()
        });
    }

    fn painted(window: &mut VisualTestContext) -> Vec<crate::PaintSurface> {
        window.update(|window, _| window.rendered_frame.scene.surfaces.clone())
    }

    #[vespertine::test]
    fn surface_bounds_are_scaled_and_snapped_to_device_pixels(cx: &mut TestAppContext) {
        // Layout rounds elements to whole logical pixels, so fractional logical bounds come
        // from the object fit: a 4:3 source contained in a 31 x 10 element is 13.33 wide and
        // starts at x = 8.83, so its edges are 8.83 and 22.17 logical pixels.
        for (scale, expected, mask) in [
            (1.0, scaled((9, 0), (13, 10)), scaled((0, 0), (31, 10))),
            (2.0, scaled((18, 0), (26, 20)), scaled((0, 0), (62, 20))),
        ] {
            let window = cx.add_empty_window();
            window.simulate_scale_factor_change(scale);
            draw(window, || {
                div()
                    .w(px(31.))
                    .h(px(10.))
                    .child(surface(source(4, 3)).size_full())
                    .into_any_element()
            });
            let surfaces = painted(window);
            assert_eq!(surfaces.len(), 1, "scale {scale}");
            assert_eq!(surfaces[0].bounds, expected, "scale {scale}");
            // The element's own bounds clip the surface, covering every device pixel it touches.
            assert_eq!(
                surfaces[0].content_mask,
                ContentMask { bounds: mask },
                "scale {scale}"
            );
        }
    }

    #[vespertine::test]
    fn surface_defaults_to_contain_and_other_fits_follow_images(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.simulate_scale_factor_change(1.0);
        let fits = [
            ("default", None, scaled((0, 25), (100, 50))),
            (
                "contain",
                Some(ObjectFit::Contain),
                scaled((0, 25), (100, 50)),
            ),
            ("fill", Some(ObjectFit::Fill), scaled((0, 0), (100, 100))),
            (
                "cover",
                Some(ObjectFit::Cover),
                scaled((-50, 0), (200, 100)),
            ),
            // The source is smaller than the element, so scale-down keeps its size.
            (
                "scale-down",
                Some(ObjectFit::ScaleDown),
                scaled((30, 40), (40, 20)),
            ),
            ("none", Some(ObjectFit::None), scaled((0, 0), (40, 20))),
        ];
        for (label, fit, expected) in fits {
            draw(window, || {
                let mut element = surface(source(40, 20)).size_full();
                if let Some(fit) = fit {
                    element = element.object_fit(fit);
                }
                element.into_any_element()
            });
            let surfaces = painted(window);
            assert_eq!(surfaces.len(), 1, "{label}");
            assert_eq!(surfaces[0].bounds, expected, "{label}");
        }
    }

    #[vespertine::test]
    fn surface_is_clipped_by_ancestor_mask_and_own_bounds(cx: &mut TestAppContext) {
        for scale in [1.0_f32, 2.0] {
            let window = cx.add_empty_window();
            window.simulate_scale_factor_change(scale);
            // An ancestor with hidden overflow clips a surface that is larger than it.
            draw(window, || {
                div()
                    .size_full()
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .left(px(20.))
                            .top(px(30.))
                            .size(px(40.))
                            .overflow_hidden()
                            .child(
                                surface(source(8, 8))
                                    .object_fit(ObjectFit::Fill)
                                    .absolute()
                                    .left(px(-10.))
                                    .top(px(-10.))
                                    .size(px(80.)),
                            ),
                    )
                    .into_any_element()
            });
            let surfaces = painted(window);
            assert_eq!(surfaces.len(), 1);
            let s = scale as i32;
            assert_eq!(
                surfaces[0].content_mask.bounds,
                scaled((20 * s, 30 * s), (40 * s, 40 * s)),
                "ancestor mask at scale {scale}"
            );
            assert_eq!(
                surfaces[0].bounds,
                scaled((10 * s, 20 * s), (80 * s, 80 * s)),
                "full destination at scale {scale}"
            );

            // Cover overflows the element, which clips it to its own bounds.
            draw(window, || {
                div()
                    .relative()
                    .size_full()
                    .child(
                        surface(source(200, 100))
                            .object_fit(ObjectFit::Cover)
                            .absolute()
                            .left(px(10.))
                            .top(px(10.))
                            .size(px(50.)),
                    )
                    .into_any_element()
            });
            let surfaces = painted(window);
            assert_eq!(surfaces.len(), 1);
            assert_eq!(
                surfaces[0].content_mask.bounds,
                scaled((10 * s, 10 * s), (50 * s, 50 * s)),
                "own-bounds mask at scale {scale}"
            );
            assert_eq!(
                surfaces[0].bounds,
                scaled((-15 * s, 10 * s), (100 * s, 50 * s)),
                "cover destination at scale {scale}"
            );
        }
    }

    #[vespertine::test]
    fn scene_retains_texture_until_it_is_dropped(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let texture = Arc::new(Texture);
        let weak: Weak<Texture> = Arc::downgrade(&texture);
        let erased: Arc<dyn Any + Send + Sync> = texture;
        let source = SurfaceSource::gpu_texture(erased, size(DevicePixels(4), DevicePixels(4)));
        assert_eq!(source.clone(), source, "clones share the texture");
        draw(window, || surface(source).size_full().into_any_element());
        assert_eq!(painted(window).len(), 1);
        assert!(
            weak.upgrade().is_some(),
            "the texture outlives its creator's handle while the scene holds it"
        );

        for _ in 0..3 {
            draw(window, || div().into_any_element());
        }
        assert!(painted(window).is_empty());
        assert!(
            weak.upgrade().is_none(),
            "the texture is released once no scene references it"
        );
    }

    #[vespertine::test]
    fn element_opacity_is_recorded_on_the_primitive(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        draw(window, || {
            div()
                .size_full()
                .opacity(0.25)
                .child(surface(source(2, 2)).size_full())
                .into_any_element()
        });
        let surfaces = painted(window);
        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].opacity, 0.25);
    }

    #[vespertine::test]
    fn empty_sources_and_destinations_paint_nothing(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        draw(window, || {
            div()
                .size_full()
                .child(surface(source(0, 4)).size_full())
                .child(surface(source(4, 0)).size_full())
                .child(surface(source(4, 4)).w(px(0.)).h(px(10.)))
                .into_any_element()
        });
        assert!(painted(window).is_empty());
    }

    #[vespertine::test]
    fn test_platform_has_no_gpu_device(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _| {
            assert!(window.gpu_context().is_none());
            assert_eq!(window.gpu_device_lost(), None);
        });
    }
}
