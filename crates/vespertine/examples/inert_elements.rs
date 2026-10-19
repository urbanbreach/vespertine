//! Compact interaction storage (PRF-003): inert elements are nearly free.
//!
//! Builds thousands of inert elements and reports how many heap allocations they cost, how many
//! listener vectors and accessibility property blocks they own (none), then attaches listeners to
//! a few elements and checks that every callback still fires exactly once, in the order the
//! dispatcher defines, with capture and bubble order unchanged.
//!
//! Run with: `cargo run -p vespertine --example inert_elements --features test-support`

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::RefCell,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};
use vespertine::{
    App, Context, Div, FocusHandle, InteractiveElement, IntoElement, Modifiers, MouseButton,
    ParentElement, Render, Styled, TestAppContext, Window, actions, div, point, px,
};

actions!(inert_elements, [Ping]);

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// Counts every allocation made by the process.
struct CountingAllocator;

// SAFETY: forwards every call to the system allocator unchanged.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn allocations() -> usize {
    ALLOCATIONS.load(Ordering::Relaxed)
}

fn report_storage(elements: &mut [Div]) -> (usize, usize) {
    let mut listener_vectors = 0;
    let mut property_blocks = 0;
    for element in elements {
        let interactivity = element.interactivity();
        listener_vectors += interactivity.allocated_listener_collections();
        property_blocks += usize::from(interactivity.has_accessibility_property_block());
    }
    (listener_vectors, property_blocks)
}

struct Ordered {
    log: Rc<RefCell<Vec<&'static str>>>,
    focus: FocusHandle,
}

impl Render for Ordered {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mouse = |name: &'static str| {
            let log = self.log.clone();
            move |_: &_, _: &mut Window, _: &mut App| log.borrow_mut().push(name)
        };
        let action = |name: &'static str| {
            let log = self.log.clone();
            move |_: &Ping, _: &mut Window, cx: &mut App| {
                log.borrow_mut().push(name);
                // Action handlers stop propagation unless told otherwise.
                cx.propagate();
            }
        };
        let inner = div()
            .id("inner")
            .size(px(50.))
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, mouse("inner bubble 1"))
            .on_mouse_down(MouseButton::Left, mouse("inner bubble 2"))
            .capture_any_mouse_down(mouse("inner capture"))
            .on_action(action("inner action 1"))
            .on_action(action("inner action 2"))
            .capture_action(action("inner capture action"));
        div()
            .id("outer")
            .size(px(100.))
            .on_mouse_down(MouseButton::Left, mouse("outer bubble 1"))
            .on_mouse_down(MouseButton::Left, mouse("outer bubble 2"))
            .capture_any_mouse_down(mouse("outer capture"))
            .on_action(action("outer action 1"))
            .on_action(action("outer action 2"))
            .capture_action(action("outer capture action"))
            .children((0..1000).map(|_| div().size(px(0.))))
            .child(inner)
    }
}

fn main() {
    const ELEMENTS: usize = 5000;

    // One empty `div()` allocates only its base style; measure it as the reference.
    let reference_start = allocations();
    let reference = div();
    let allocations_per_element = allocations() - reference_start;
    drop(reference);

    let start = allocations();
    let mut elements: Vec<Div> = Vec::with_capacity(ELEMENTS);
    elements.extend((0..ELEMENTS).map(|_| div()));
    let construction_allocations = allocations() - start;
    let (listener_vectors, property_blocks) = report_storage(&mut elements);
    println!("inert elements:                  {ELEMENTS}");
    println!("allocations per inert element:   {allocations_per_element}");
    println!(
        "allocations for all elements:    {construction_allocations} (one vector of elements + {} base styles)",
        construction_allocations - 1
    );
    println!("listener-vector allocations:     {listener_vectors}");
    println!("accessibility property blocks:   {property_blocks}");
    assert_eq!(listener_vectors, 0);
    assert_eq!(property_blocks, 0);
    assert_eq!(
        construction_allocations,
        ELEMENTS * allocations_per_element + 1,
        "inert elements allocate nothing beyond their base style"
    );

    // Attach listeners to a few elements: each used collection allocates exactly one vector.
    let start = allocations();
    for element in elements.iter_mut().take(10) {
        element
            .interactivity()
            .on_mouse_down(MouseButton::Left, |_, _, _| {});
        element
            .interactivity()
            .on_mouse_down(MouseButton::Left, |_, _, _| {});
        element.interactivity().on_key_down(|_, _, _| {});
    }
    let (listener_vectors, property_blocks) = report_storage(&mut elements);
    println!(
        "after attaching listeners to 10: {listener_vectors} listener vectors, {property_blocks} property blocks, {} allocations",
        allocations() - start
    );
    assert_eq!(
        listener_vectors, 20,
        "two collections used by each of 10 elements"
    );
    assert_eq!(property_blocks, 0);

    // Dispatch order with listeners registered on nested elements, among many inert siblings.
    let mut cx = TestAppContext::single();
    let log = Rc::new(RefCell::new(Vec::new()));
    let (root, cx) = cx.add_window_view({
        let log = log.clone();
        |_, cx| Ordered {
            log,
            focus: cx.focus_handle(),
        }
    });
    cx.update(|window, cx| {
        let focus = root.read(cx).focus.clone();
        window.focus(&focus, cx);
        window.draw(cx).clear(cx);
    });
    cx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
    let mouse_order = log.borrow().clone();
    log.borrow_mut().clear();
    cx.dispatch_action(Ping);
    let action_order = log.borrow().clone();

    println!("mouse down order:  {mouse_order:?}");
    println!("action order:      {action_order:?}");
    assert_eq!(
        mouse_order,
        [
            "outer capture",
            "inner capture",
            "inner bubble 2",
            "inner bubble 1",
            "outer bubble 2",
            "outer bubble 1"
        ]
    );
    assert_eq!(
        action_order,
        [
            "outer capture action",
            "inner capture action",
            "inner action 1",
            "inner action 2",
            "outer action 1",
            "outer action 2"
        ]
    );
    println!("every callback fired exactly once");
}
