use objc2::runtime::ProtocolObject;
use objc2::{rc::Retained, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSScreen, NSStatusWindowLevel, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect};
use objc2_metal_kit::{MTKView, MTKViewDelegate};

use crate::renderer::MetalRenderer;

pub struct OverlaySet {
    pub windows: Vec<Retained<NSWindow>>,
    pub views: Vec<Retained<MTKView>>,
    pub renderers: Vec<MetalRenderer>,
    pub screen_frames: Vec<NSRect>,
}

pub fn create_for_connected_displays(
    mtm: MainThreadMarker,
    delegate: &ProtocolObject<dyn MTKViewDelegate>,
) -> OverlaySet {
    let screens = NSScreen::screens(mtm);
    let mut windows = Vec::with_capacity(screens.count());
    let mut views = Vec::with_capacity(screens.count());
    let mut renderers = Vec::with_capacity(screens.count());
    let mut screen_frames = Vec::with_capacity(screens.count());

    for index in 0..screens.count() {
        let screen = screens.objectAtIndex(index);
        let screen_frame = screen.frame();
        let content_frame = NSRect::new(NSPoint::new(0.0, 0.0), screen_frame.size);
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer_screen(
                NSWindow::alloc(mtm),
                screen_frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
                Some(&screen),
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setLevel(NSStatusWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        match MetalRenderer::new((index as u32).wrapping_add(0x5749_4e54)) {
            Ok(renderer) => {
                let view = MTKView::initWithFrame_device(
                    MTKView::alloc(mtm),
                    content_frame,
                    Some(renderer.device()),
                );
                renderer.configure_view(&view, content_frame);
                view.setDelegate(Some(delegate));
                window.setContentView(Some(&view));
                views.push(view);
                renderers.push(renderer);
                screen_frames.push(screen_frame);
            }
            Err(error) => {
                eprintln!("Snowfall renderer unavailable for display {index}: {error}");
            }
        }
        windows.push(window);
    }

    if windows.is_empty() {
        eprintln!("No connected displays were returned by AppKit");
    }
    OverlaySet {
        windows,
        views,
        renderers,
        screen_frames,
    }
}
