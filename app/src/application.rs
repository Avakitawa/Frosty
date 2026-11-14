use std::cell::{Cell, OnceCell, RefCell};
use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{define_class, msg_send, sel, AllocAnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSBezierPath, NSButton, NSButtonType, NSCellImagePosition, NSColor,
    NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventMask, NSEventModifierFlags,
    NSEventType, NSFocusRingType, NSFont, NSGradient, NSImage, NSImageScaling,
    NSImageSymbolConfiguration, NSImageView, NSMenu, NSMenuItem, NSPopover, NSPopoverBehavior,
    NSScreen, NSSlider, NSStatusBar, NSStatusBarButton, NSStatusItem, NSStatusWindowLevel,
    NSTextAlignment, NSTextField, NSView, NSViewController, NSWindow, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSArray, NSData, NSLocale, NSNotification, NSObject,
    NSObjectProtocol, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSUserDefaults, NSURL,
};
use objc2_metal_kit::{MTKView, MTKViewDelegate};
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use winter_core::SnowfallState;
use winter_physics::{BlockedRegion, CollisionSurface};

use crate::overlay;
use crate::renderer::MetalRenderer;
use crate::settings::AppSettings;
use crate::windows::VisibleWindow;

// Settings panel geometry. Every `*_TOP` value is the distance from the top edge
// of the panel; `panel_frame` converts it to AppKit's bottom-left coordinates.
// Rows are derived from one another, so the layout cannot drift or overlap.
const PANEL_WIDTH: f64 = 440.0;
const PANEL_PADDING: f64 = 20.0;
const PANEL_CONTENT_WIDTH: f64 = PANEL_WIDTH - PANEL_PADDING * 2.0;
const SECTION_TITLE_HEIGHT: f64 = 15.0;
const TITLE_TO_CONTROL: f64 = 8.0;
const HEADER_TOP: f64 = 14.0;
const HEADER_HEIGHT: f64 = 44.0;
const WEATHER_TITLE_TOP: f64 = HEADER_TOP + HEADER_HEIGHT + 16.0;
const WEATHER_TOP: f64 = WEATHER_TITLE_TOP + SECTION_TITLE_HEIGHT + TITLE_TO_CONTROL;
const WEATHER_HEIGHT: f64 = 50.0;
const WEATHER_META_TOP: f64 = WEATHER_TOP + WEATHER_HEIGHT + 4.0;
const WEATHER_META_HEIGHT: f64 = 20.0;
const WEATHER_HINT_TOP: f64 = WEATHER_META_TOP + 22.0;
const WEATHER_HINT_HEIGHT: f64 = 14.0;
const TOOL_TITLE_TOP: f64 = WEATHER_HINT_TOP + WEATHER_HINT_HEIGHT + 10.0;
const TOOL_TOP: f64 = TOOL_TITLE_TOP + SECTION_TITLE_HEIGHT + TITLE_TO_CONTROL;
const TOOL_HEIGHT: f64 = 58.0;
const TOOL_HINT_TOP: f64 = TOOL_TOP + TOOL_HEIGHT + 6.0;
const TOOL_HINT_HEIGHT: f64 = 14.0;
const WIND_TITLE_TOP: f64 = TOOL_HINT_TOP + TOOL_HINT_HEIGHT + 14.0;
const COMPASS_CARD_TOP: f64 = WIND_TITLE_TOP + SECTION_TITLE_HEIGHT + TITLE_TO_CONTROL;
const COMPASS_CARD_HEIGHT: f64 = 140.0;
const WIND_CARD_LEFT_WIDTH: f64 = 156.0;
const COMPASS_CENTER_X: f64 = PANEL_PADDING + WIND_CARD_LEFT_WIDTH / 2.0;
const WIND_SLIDER_X: f64 = PANEL_PADDING + WIND_CARD_LEFT_WIDTH + 10.0;
const WIND_SLIDER_WIDTH: f64 = PANEL_WIDTH - PANEL_PADDING - WIND_SLIDER_X - 14.0;
const SLIDER_ROW_HEIGHT: f64 = 42.0;
const WIND_SLIDER_FIRST_TOP: f64 = COMPASS_CARD_TOP + 28.0;
const WIND_SLIDER_SECOND_TOP: f64 = WIND_SLIDER_FIRST_TOP + SLIDER_ROW_HEIGHT;
const TOGGLES_TOP: f64 = COMPASS_CARD_TOP + COMPASS_CARD_HEIGHT + 12.0;
const TOGGLE_HEIGHT: f64 = 46.0;
const CLEAR_TOP: f64 = TOGGLES_TOP + TOGGLE_HEIGHT + 12.0;
const CLEAR_HEIGHT: f64 = 34.0;
const ADDITIONAL_TOP: f64 = CLEAR_TOP + CLEAR_HEIGHT + 12.0;
const ADDITIONAL_HEIGHT: f64 = 34.0;
const PHYSICS_TOP: f64 = ADDITIONAL_TOP + ADDITIONAL_HEIGHT + 8.0;
const PHYSICS_SECOND_ROW_TOP: f64 = PHYSICS_TOP + SLIDER_ROW_HEIGHT;
const PHYSICS_TOGGLE_TOP: f64 = PHYSICS_SECOND_ROW_TOP + SLIDER_ROW_HEIGHT + 6.0;
const PHYSICS_HEIGHT: f64 = PHYSICS_TOGGLE_TOP + 40.0 - PHYSICS_TOP;
const ABOUT_TOP_COLLAPSED: f64 = ADDITIONAL_TOP + ADDITIONAL_HEIGHT + 8.0;
const ABOUT_TOP_EXPANDED: f64 = PHYSICS_TOP + PHYSICS_HEIGHT + 8.0;
const ABOUT_HEIGHT: f64 = 56.0;
const PANEL_HEIGHT_COLLAPSED: f64 = ABOUT_TOP_COLLAPSED + ABOUT_HEIGHT + 14.0;
const PANEL_HEIGHT_EXPANDED: f64 = ABOUT_TOP_EXPANDED + ABOUT_HEIGHT + 14.0;
// Kept for layout tests and the initial panel size calculations.
const WINDOW_SNOW_MARGIN_RATIO: f64 = 0.025;
const MIN_WINDOW_SNOW_MARGIN_POINTS: f64 = 8.0;
const MAX_WINDOW_SNOW_MARGIN_POINTS: f64 = 24.0;
const DOCK_SNOW_DEPTH_POINTS: f64 = 28.0;
const DOCK_TOP_SURFACE_ID: u64 = u64::MAX - 21;
const DOCK_LEFT_SURFACE_ID: u64 = u64::MAX - 22;
const DOCK_RIGHT_SURFACE_ID: u64 = u64::MAX - 23;

struct SettingSliderConfig {
    title: &'static str,
    tag: isize,
    value: f64,
    minimum: f64,
    maximum: f64,
    top: f64,
    x: f64,
    width: f64,
}

struct SliderControl {
    tag: isize,
    slider: Retained<NSSlider>,
    value_label: Retained<NSTextField>,
}

struct ToggleControl {
    tag: isize,
    button: Retained<NSButton>,
    container: Retained<NSView>,
    knob: Retained<NSView>,
    switch_width: f64,
    switch_height: f64,
    knob_size: f64,
}

struct SettingsCard {
    container: Retained<NSView>,
    icon: Retained<NSImageView>,
    title: Retained<NSTextField>,
    subtitle: Option<Retained<NSTextField>>,
    button: Retained<NSButton>,
    is_tool: bool,
}

const WIND_CALM_TAG: isize = 48;
const COMPASS_RADIUS: f64 = 44.0;
const COMPASS_INNER_RADIUS: f64 = 38.0;
const COMPASS_STEP_X: f64 = 32.0;
const COMPASS_STEP_Y: f64 = 32.0;
const CUSTOM_WEATHER_MODE: u8 = 3;
const MAX_WIND_STRENGTH: f32 = 0.08;
const MAX_WIND_TURBULENCE: f32 = 0.05;

// UI palette. The panel is mostly #091540 and #3D518C; #1B2CC1 and #7692FF are
// reserved for selected states, borders and interactive accents.
const HEX_DARK: u32 = 0x091540;
const HEX_SURFACE: u32 = 0x3D518C;
const HEX_PRIMARY: u32 = 0x1B2CC1;
const HEX_ACCENT: u32 = 0x7692FF;
const HEX_TEXT: u32 = 0xABD2FA;

const APP_AUTHOR: &str = "Avakitawa";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const DONATION_LINKS: [(&str, &str); 2] = [
    ("♥ Ko-fi", "https://ko-fi.com/avakitawa"),
    ("♥ Boosty", "https://boosty.to/avakitawa"),
];

#[derive(Clone, Copy)]
enum DragTarget {
    Snowball(u64),
    Snowman(u64),
}

#[derive(Clone, Copy)]
struct OverlayDrag {
    screen_index: usize,
    target: DragTarget,
    last_time: Instant,
}

#[derive(Default)]
struct AppDelegateIvars {
    state: Cell<SnowfallState>,
    first_frame_submitted: Cell<bool>,
    windows: RefCell<Vec<Retained<objc2_app_kit::NSWindow>>>,
    views: RefCell<Vec<Retained<MTKView>>>,
    renderers: RefCell<Vec<MetalRenderer>>,
    screen_frames: RefCell<Vec<objc2_foundation::NSRect>>,
    last_window_refresh: Cell<Option<Instant>>,
    settings: RefCell<AppSettings>,
    cursor_tool: Cell<u8>,
    language: Cell<Language>,
    last_preset: Cell<u8>,
    weather_info: RefCell<Option<Retained<NSTextField>>>,
    weather_badge: RefCell<Option<Retained<NSTextField>>>,
    settings_popover: OnceCell<Retained<NSPopover>>,
    settings_controller: OnceCell<Retained<NSViewController>>,
    settings_buttons: RefCell<Vec<Retained<NSButton>>>,
    cards: RefCell<Vec<SettingsCard>>,
    sliders: RefCell<Vec<SliderControl>>,
    toggle_controls: RefCell<Vec<ToggleControl>>,
    wind_status: OnceCell<Retained<NSTextField>>,
    tool_hint: OnceCell<Retained<NSTextField>>,
    status_item: OnceCell<Retained<NSStatusItem>>,
    overlay_drag: Cell<Option<OverlayDrag>>,
    roll_origin: Cell<Option<(usize, f32, f32)>>,
    rolled_with_option: Cell<bool>,
    physics_expanded: Cell<bool>,
    physics_section: RefCell<Option<Retained<FlippedSectionView>>>,
    about_section: RefCell<Option<Retained<FlippedSectionView>>>,
    expand_button: RefCell<Option<Retained<NSButton>>>,
    settings_content: RefCell<Option<Retained<PanelBackgroundView>>>,
    onboarding_window: RefCell<Option<Retained<NSWindow>>>,
    hint_window: RefCell<Option<Retained<NSWindow>>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = self.mtm();
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            match crate::settings::load() {
                Ok(Some(settings)) => *self.ivars().settings.borrow_mut() = settings,
                Ok(None) => {}
                Err(error) => {
                    eprintln!("Cannot load settings; keeping defaults without replacing the file: {error}");
                }
            }
            {
                // A preset mode is the source of truth: normalize old settings
                // files so the selected preset and the sliders always agree.
                let mut settings = self.ivars().settings.borrow_mut();
                let mode = settings.weather_mode;
                if mode <= 2 {
                    apply_weather_preset(&mut settings, mode);
                    *settings = settings.validate();
                }
            }
            let last_preset = {
                let settings = self.ivars().settings.borrow();
                if settings.weather_mode <= 2 {
                    settings.weather_mode
                } else {
                    1
                }
            };
            self.ivars().last_preset.set(last_preset);
            if !self.ivars().settings.borrow().enabled {
                let mut state = self.ivars().state.get();
                state.toggle();
                self.ivars().state.set(state);
            }

            let status_item = NSStatusBar::systemStatusBar()
                .statusItemWithLength(objc2_app_kit::NSVariableStatusItemLength);
            let button = status_item
                .button(mtm)
                .expect("new status item must have a button");
            if let Some(image) = symbol_image("snowflake", 16.0, SymbolWeight::Medium) {
                button.setImage(Some(&image));
                button.setImagePosition(NSCellImagePosition::ImageOnly);
            } else {
                button.setTitle(ns_string!("❄"));
            }
            button.setToolTip(Some(&NSString::from_str(tr(
                self.ivars().language.get(),
                "Frosty — снегопад",
                "Frosty — Snowfall",
            ))));
            unsafe {
                button.setTarget(Some(self));
                button.setAction(Some(sel!(statusItemClicked:)));
            }
            let _ = button.sendActionOn(
                NSEventMask::LeftMouseUp
                    | NSEventMask::RightMouseUp
                    | NSEventMask::RightMouseDown,
            );
            self.ivars().status_item.set(status_item).unwrap();
            self.refresh_overlays();
            eprintln!(
                "Frosty started; {} display overlay(s) created with up to 120 FPS",
                self.ivars().windows.borrow().len()
            );
            if !onboarding_seen() {
                self.show_onboarding(mtm);
            }
        }

        #[unsafe(method(applicationDidChangeScreenParameters:))]
        fn did_change_screen_parameters(&self, _notification: &NSNotification) {
            self.refresh_overlays();
            eprintln!(
                "Display configuration changed; {} display overlay(s) active",
                self.ivars().windows.borrow().len()
            );
        }
    }

    unsafe impl MTKViewDelegate for AppDelegate {
        #[unsafe(method(drawInMTKView:))]
        fn draw_in_mtk_view(&self, view: &MTKView) {
            self.refresh_window_surfaces();
            let renderer_index = self
                .ivars()
                .views
                .borrow()
                .iter()
                .position(|candidate| std::ptr::eq(&**candidate, view));
            let Some(renderer_index) = renderer_index else {
                return;
            };
            let Some(frame) = self
                .ivars()
                .screen_frames
                .borrow()
                .get(renderer_index)
                .copied()
            else {
                return;
            };
            let mouse = NSEvent::mouseLocation();
            let simulation_space = DisplaySimulationSpace::new(frame);
            let position = (mouse.x >= simulation_space.left
                && mouse.x <= simulation_space.left + simulation_space.width
                && mouse.y >= simulation_space.bottom
                && mouse.y <= simulation_space.top())
                .then_some((
                    simulation_space.x(mouse.x),
                    simulation_space.y_from_top(mouse.y),
                ));
            let mut show_roll_hint = false;
            let mut show_limit_hint = false;
            let interactive = {
                let mut renderers = self.ivars().renderers.borrow_mut();
                if let Some(renderer) = renderers.get_mut(renderer_index) {
                    renderer.set_cursor_tool(self.ivars().cursor_tool.get());
                    renderer.set_cursor_position(
                        position,
                        frame.size.width as f32,
                        frame.size.height as f32,
                    );

                    if !roll_hint_seen()
                        && self.ivars().cursor_tool.get() == 30
                        && self.ivars().overlay_drag.get().is_none()
                        && self.ivars().onboarding_window.borrow().is_none()
                    {
                        if let Some((x, y)) = position {
                            if renderer.can_roll_snowball_at(x, y) {
                                show_roll_hint = true;
                            }
                        }
                    }

                    // Holding Option turns a mitten sweep into an explicit
                    // "roll one snowball" gesture. Plain sweeps only push snow.
                    let option_down =
                        NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Option);
                    if !option_down || self.ivars().cursor_tool.get() != 30 {
                        self.ivars().rolled_with_option.set(false);
                        self.ivars().roll_origin.set(None);
                    } else if let Some((x, y)) = position {
                        let mut origin = self.ivars().roll_origin.get();
                        if origin.map(|(screen, _, _)| screen) != Some(renderer_index) {
                            origin = Some((renderer_index, x, y));
                            self.ivars().roll_origin.set(origin);
                            self.ivars().rolled_with_option.set(false);
                        }
                        if !self.ivars().rolled_with_option.get()
                            && self.ivars().overlay_drag.get().is_none()
                        {
                            if let Some((_, origin_x, origin_y)) = origin {
                                let travel = (x - origin_x).hypot(y - origin_y);
                                if travel > 0.006 && renderer.can_roll_snowball_at(x, y) {
                                    let direction = if (x - origin_x).abs() > 0.0005 {
                                        (x - origin_x).signum()
                                    } else {
                                        1.0
                                    };
                                    if renderer.roll_snowball_at(x, y, direction) {
                                        self.ivars().rolled_with_option.set(true);
                                    } else if renderer.snowball_limit_reached() {
                                        self.ivars().rolled_with_option.set(true);
                                        show_limit_hint = true;
                                    }
                                }
                            }
                        }
                    }

                    let drag = self
                        .ivars()
                        .overlay_drag
                        .get()
                        .filter(|drag| drag.screen_index == renderer_index);
                    if let (Some(drag), Some((x, y))) = (drag, position) {
                        // Keep the held object on the pointer even between mouse events.
                        match drag.target {
                            DragTarget::Snowball(id) => {
                                renderer.drag_snowball(id, x, y, 1.0 / 120.0);
                            }
                            DragTarget::Snowman(id) => {
                                renderer.drag_snowman(id, x, y, 1.0 / 120.0);
                            }
                        }
                    }
                    let hovered_ball = position.and_then(|(x, y)| renderer.snowball_at(x, y));
                    let hovered_snowman = if hovered_ball.is_none() {
                        position.and_then(|(x, y)| renderer.snowman_at(x, y))
                    } else {
                        None
                    };
                    let (dragged_ball, dragged_snowman) = match drag.map(|drag| drag.target) {
                        Some(DragTarget::Snowball(id)) => (Some(id), None),
                        Some(DragTarget::Snowman(id)) => (None, Some(id)),
                        None => (None, None),
                    };
                    renderer.set_hovered_snowball(dragged_ball.or(hovered_ball));
                    renderer.set_hovered_snowman(dragged_snowman.or(hovered_snowman));
                    if renderer.draw(view) && !self.ivars().first_frame_submitted.replace(true) {
                        eprintln!("First Metal physics frame submitted at up to 120 FPS");
                    }
                    drag.is_some() || hovered_ball.is_some() || hovered_snowman.is_some()
                } else {
                    false
                }
            };
            if show_roll_hint {
                self.show_roll_hint();
            } else if show_limit_hint {
                self.show_too_many_snowballs_hint();
            }
            self.set_overlay_interactive(renderer_index, interactive);
        }

        #[unsafe(method(mtkView:drawableSizeWillChange:))]
        fn drawable_size_will_change(
            &self,
            _view: &MTKView,
            _size: objc2_core_foundation::CGSize,
        ) {
        }
    }

    impl AppDelegate {
        #[unsafe(method(openSettings:))]
        fn open_settings(&self, sender: &NSStatusBarButton) {
            self.show_settings_popover(sender);
        }

        #[unsafe(method(hideHint:))]
        fn hide_hint(&self, _argument: &NSObject) {
            if let Some(window) = self.ivars().hint_window.borrow_mut().take() {
                window.close();
            }
        }

        #[unsafe(method(statusItemClicked:))]
        fn status_item_clicked(&self, sender: &NSStatusBarButton) {
            let app = NSApplication::sharedApplication(self.mtm());
            let right_click = app.currentEvent().as_ref().is_some_and(|event| {
                event.r#type() == NSEventType::RightMouseDown
                    || event.r#type() == NSEventType::RightMouseUp
                    || event
                        .modifierFlags()
                        .contains(NSEventModifierFlags::Control)
            });
            if right_click {
                self.show_status_menu(sender);
            } else {
                self.show_settings_popover(sender);
            }
        }

        #[unsafe(method(menuOpenSettings:))]
        fn menu_open_settings(&self, _sender: &NSMenuItem) {
            if let Some(button) = self
                .ivars()
                .status_item
                .get()
                .and_then(|item| item.button(self.mtm()))
            {
                self.show_settings_popover(&button);
            }
        }

        #[unsafe(method(menuTogglePause:))]
        fn menu_toggle_pause(&self, _sender: &NSMenuItem) {
            let enabled = !self.ivars().settings.borrow().enabled;
            self.set_snowfall_enabled(enabled);
        }

        #[unsafe(method(menuWeatherPreset:))]
        fn menu_weather_preset(&self, sender: &NSMenuItem) {
            let mode = sender.tag().clamp(0, 2) as u8;
            self.apply_weather_mode(mode);
        }

        #[unsafe(method(menuToggleLaunchAtLogin:))]
        fn menu_toggle_launch_at_login(&self, _sender: &NSMenuItem) {
            self.toggle_launch_at_login();
        }

        #[unsafe(method(menuQuit:))]
        fn menu_quit(&self, _sender: &NSMenuItem) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }

        #[unsafe(method(resetWeatherPreset:))]
        fn reset_weather_preset(&self, _sender: &NSButton) {
            let mode = {
                let settings = self.ivars().settings.borrow();
                if settings.weather_mode <= 2 {
                    settings.weather_mode
                } else {
                    self.ivars().last_preset.get().min(2)
                }
            };
            self.apply_weather_mode(mode);
        }

        #[unsafe(method(resetAllSettings:))]
        fn reset_all_settings(&self, _sender: &NSButton) {
            let enabled = self.ivars().settings.borrow().enabled;
            *self.ivars().settings.borrow_mut() = AppSettings {
                enabled,
                ..AppSettings::default()
            }
            .validate();
            self.ivars().last_preset.set(1);
            self.sync_weather_buttons();
            self.sync_wind_controls();
            self.sync_slider_controls();
            self.sync_toggle_controls();
            self.save_settings();
            self.apply_settings();
        }

        #[unsafe(method(dismissOnboarding:))]
        fn dismiss_onboarding(&self, _sender: &NSButton) {
            if let Some(window) = self.ivars().onboarding_window.borrow_mut().take() {
                window.close();
            }
        }

        #[unsafe(method(togglePhysics:))]
        fn toggle_physics(&self, _sender: &NSButton) {
            let expanded = !self.ivars().physics_expanded.get();
            if expanded && !settings_expanded_fits(self.mtm()) {
                let language = self.ivars().language.get();
                self.show_transient_hint(tr(
                    language,
                    "Для физики не хватает высоты экрана. Сверни её на этом Mac.",
                    "There is not enough screen height for physics on this Mac.",
                ));
                return;
            }
            self.set_physics_expanded(expanded);
        }

        #[unsafe(method(quitApp:))]
        fn quit_app(&self, _sender: &NSButton) {
            let app = NSApplication::sharedApplication(self.mtm());
            app.terminate(None);
        }

        #[unsafe(method(openDonation:))]
        fn open_donation(&self, sender: &NSButton) {
            let index = sender.tag().saturating_sub(70) as usize;
            let Some((label, url)) = DONATION_LINKS.get(index) else {
                return;
            };
            if url.is_empty() {
                eprintln!("Donation link for {label} is not configured");
                return;
            }
            let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
                eprintln!("Donation link for {label} is invalid");
                return;
            };
            if !NSWorkspace::sharedWorkspace().openURL(&url) {
                eprintln!("Cannot open the donation link for {label}");
            }
        }

        #[unsafe(method(selectWeather:))]
        fn select_weather(&self, sender: &NSButton) {
            let mode = sender.tag().saturating_sub(20).clamp(0, 2) as u8;
            {
                let mut settings = self.ivars().settings.borrow_mut();
                apply_weather_preset(&mut settings, mode);
                *settings = settings.validate();
            }
            self.ivars().last_preset.set(mode);
            // Presets rewrite the physical sliders and the wind compass.
            self.sync_weather_buttons();
            self.sync_wind_controls();
            self.sync_slider_controls();
            self.sync_toggle_controls();
            self.save_settings();
            self.apply_settings();
        }

        #[unsafe(method(selectTool:))]
        fn select_tool(&self, sender: &NSButton) {
            let tool = sender.tag().clamp(30, 31);
            self.select_button_group(30, 31, tool);
            self.ivars().cursor_tool.set(tool as u8);
            if let Some(hint) = self.ivars().tool_hint.get() {
                hint.setStringValue(&NSString::from_str(tool_hint_text(tool as u8, self.ivars().language.get())));
            }
        }

        #[unsafe(method(selectWind:))]
        fn select_wind(&self, sender: &NSButton) {
            {
                let mut settings = self.ivars().settings.borrow_mut();
                if sender.tag() == WIND_CALM_TAG {
                    settings.wind_strength = 0.0;
                } else {
                    let angle = direction_angle(sender.tag());
                    if direction_tag(settings.wind_direction) == sender.tag()
                        && settings.wind_strength > 0.0
                    {
                        settings.wind_strength = (settings.wind_strength + 0.018).min(0.08);
                    } else {
                        settings.wind_direction = angle;
                        settings.wind_strength = settings.wind_strength.max(0.018);
                    }
                }
                // Any manual wind edit leaves the weather preset behind.
                settings.weather_mode = CUSTOM_WEATHER_MODE;
                *settings = settings.validate();
            }
            self.sync_weather_buttons();
            self.sync_wind_controls();
            self.sync_slider_controls();
            self.save_settings();
            self.apply_settings();
        }

        #[unsafe(method(clearSnow:))]
        fn clear_snow(&self, _sender: &NSButton) {
            self.clear_accumulated_snow();
        }

        #[unsafe(method(toggleSnowOption:))]
        fn toggle_snow_option(&self, sender: &NSButton) {
            let enabled = sender.state() == NSControlStateValueOn;
            let tag = sender.tag();
            {
                let mut settings = self.ivars().settings.borrow_mut();
                match tag {
                    60 => settings.particles_in_windows = enabled,
                    61 => settings.window_wind_draft = enabled,
                    62 => settings.window_snow = enabled,
                    63 => settings.accumulation = enabled,
                    64 => settings.enabled = enabled,
                    _ => return,
                }
                *settings = settings.validate();
            }
            sender.setState(if enabled {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            if let Some(control) = self
                .ivars()
                .toggle_controls
                .borrow()
                .iter()
                .find(|control| control.tag == tag)
            {
                style_toggle_control(control, enabled);
            }
            if tag == 60 {
                self.ivars().settings.borrow_mut().weather_mode = CUSTOM_WEATHER_MODE;
                self.sync_weather_buttons();
            }
            self.save_settings();
            self.apply_settings();
        }

        #[unsafe(method(changeSnowSetting:))]
        fn change_snow_setting(&self, sender: &NSSlider) {
            let tag = sender.tag();
            let manual_weather = matches!(tag, 90..=95);
            {
                let mut settings = self.ivars().settings.borrow_mut();
                match tag {
                    90 => settings.accumulation_speed = sender.doubleValue() as f32,
                    91 => {
                        settings.particle_count = ((sender.doubleValue() / 500.0).round() as u32
                            * 500)
                            .clamp(500, 20_000);
                    }
                    92 => settings.intensity = sender.doubleValue() as f32,
                    93 => settings.falling_speed = sender.doubleValue() as f32,
                    94 => settings.wind_strength = sender.doubleValue() as f32,
                    95 => settings.wind_turbulence = sender.doubleValue() as f32,
                    _ => return,
                }
                if manual_weather {
                    settings.weather_mode = CUSTOM_WEATHER_MODE;
                }
                *settings = settings.validate();
            }
            self.sync_slider_controls();
            if manual_weather {
                self.sync_weather_buttons();
            }
            if tag == 94 {
                self.sync_wind_controls();
            }
            self.save_settings();
            self.apply_settings();
        }

    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars::default());
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.ivars().language.set(detect_language());
        this.ivars().cursor_tool.set(30);
        this
    }

    fn show_onboarding(&self, mtm: MainThreadMarker) {
        let language = self.ivars().language.get();
        mark_onboarding_seen();

        let size = NSSize::new(440.0, 370.0);
        let content = PanelBackgroundView::new(mtm, NSRect::new(NSPoint::new(0.0, 0.0), size));

        let icon = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            panel_frame(PANEL_PADDING, 16.0, 44.0, 44.0),
        );
        icon.setImage(app_icon_image().as_deref());
        icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        content.addSubview(&icon);

        let title = NSTextField::labelWithString(
            &NSString::from_str(tr(
                language,
                "Добро пожаловать в Frosty",
                "Welcome to Frosty",
            )),
            mtm,
        );
        title.setFrame(panel_frame(76.0, 13.0, 340.0, 22.0));
        style_label(&title, color_text(1.0));
        title.setFont(Some(&NSFont::boldSystemFontOfSize(17.0)));
        content.addSubview(&title);

        let subtitle = NSTextField::labelWithString(
            &NSString::from_str(tr(
                language,
                "Три короткие подсказки для начала",
                "Three quick tips to get started",
            )),
            mtm,
        );
        subtitle.setFrame(panel_frame(76.0, 37.0, 340.0, 16.0));
        style_label(&subtitle, color_accent(0.85));
        subtitle.setFont(Some(&NSFont::systemFontOfSize(10.5)));
        content.addSubview(&subtitle);

        let divider = NSView::initWithFrame(
            NSView::alloc(mtm),
            panel_frame(PANEL_PADDING, 70.0, PANEL_CONTENT_WIDTH, 1.0),
        );
        style_layer(&divider, color_surface(0.55), 0.5, 0.0, color_clear());
        content.addSubview(&divider);

        let steps = [
            (
                tr(language, "Иконка в строке меню", "The menu bar icon"),
                tr(
                    language,
                    "Frosty живёт рядом с часами. Снежинка — это приложение.",
                    "Frosty lives next to the clock. The snowflake is the app.",
                ),
                "snowflake",
            ),
            (
                tr(language, "Открыть настройки", "Open settings"),
                tr(
                    language,
                    "Нажми на снежинку — откроется панель с погодой и инструментами.",
                    "Click the snowflake to open weather and tool settings.",
                ),
                "gearshape.fill",
            ),
            (
                tr(language, "Слепить снежок", "Pack a snowball"),
                tr(
                    language,
                    "Зажми Option и проведи варежкой по сугробу.",
                    "Hold Option and sweep the mitten across a drift.",
                ),
                "hand.raised.fill",
            ),
        ];
        for (index, (heading, body, symbol)) in steps.into_iter().enumerate() {
            let top = 82.0 + index as f64 * 62.0;
            let card = NSView::initWithFrame(
                NSView::alloc(mtm),
                panel_frame(PANEL_PADDING, top, PANEL_CONTENT_WIDTH, 56.0),
            );
            style_layer(&card, color_surface(0.30), 12.0, 1.0, color_accent(0.18));
            content.addSubview(&card);

            let step_icon = NSImageView::initWithFrame(
                NSImageView::alloc(mtm),
                top_frame(56.0, 14.0, 16.0, 24.0, 24.0),
            );
            step_icon.setImage(symbol_image(symbol, 18.0, SymbolWeight::Medium).as_deref());
            step_icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
            step_icon.setContentTintColor(Some(&color_accent(0.95)));
            card.addSubview(&step_icon);

            let heading_label = NSTextField::labelWithString(&NSString::from_str(heading), mtm);
            heading_label.setFrame(top_frame(56.0, 52.0, 8.0, 330.0, 16.0));
            style_label(&heading_label, color_text(1.0));
            heading_label.setFont(Some(&NSFont::boldSystemFontOfSize(11.5)));
            card.addSubview(&heading_label);

            let body_label = NSTextField::labelWithString(&NSString::from_str(body), mtm);
            body_label.setFrame(top_frame(56.0, 52.0, 25.0, 330.0, 15.0));
            style_label(&body_label, color_accent(0.85));
            body_label.setFont(Some(&NSFont::systemFontOfSize(9.5)));
            card.addSubview(&body_label);
        }

        let button = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            panel_frame(PANEL_PADDING, 292.0, PANEL_CONTENT_WIDTH, 36.0),
        );
        button.setBordered(false);
        button.setButtonType(NSButtonType::MomentaryPushIn);
        button.setTitle(&NSString::from_str(tr(language, "Понятно", "Got it")));
        button.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
        button.setContentTintColor(Some(&color_text(1.0)));
        button.setFocusRingType(NSFocusRingType::None);
        style_layer(&button, color_primary(0.96), 10.0, 1.0, color_accent(0.55));
        unsafe {
            button.setTarget(Some(self));
            button.setAction(Some(sel!(dismissOnboarding:)));
        }
        content.addSubview(&button);

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), size),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Frosty"));
        window.setBackgroundColor(Some(&color_dark(1.0)));
        window.setContentSize(size);
        window.setContentView(Some(&content));
        window.setLevel(NSStatusWindowLevel + 1);
        window.center();
        let dark_appearance: Option<Retained<AnyObject>> = unsafe {
            msg_send![objc2::class!(NSAppearance), appearanceNamed: ns_string!("NSAppearanceNameDarkAqua")]
        };
        let _: () = unsafe { msg_send![&*window, setAppearance: dark_appearance.as_deref()] };
        self.ivars().onboarding_window.replace(Some(window.clone()));
        NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
    }

    fn show_roll_hint(&self) {
        if roll_hint_seen() {
            return;
        }
        mark_roll_hint_seen();
        let language = self.ivars().language.get();
        self.show_transient_hint(tr(
            language,
            "Зажми Option и проведи варежкой по сугробу — получится снежок.",
            "Hold Option and sweep the mitten across a drift to pack a snowball.",
        ));
    }

    fn show_too_many_snowballs_hint(&self) {
        let language = self.ivars().language.get();
        self.show_transient_hint(tr(
            language,
            "Слишком много снежков — сначала убери или соедини лишние.",
            "Too many snowballs — clear or combine some first.",
        ));
    }

    fn show_transient_hint(&self, text: &str) {
        let mtm = self.mtm();
        if let Some(window) = self.ivars().hint_window.borrow_mut().take() {
            window.close();
        }
        let width = 330.0;
        let height = 46.0;
        let mouse = NSEvent::mouseLocation();
        let mut origin_x = mouse.x + 16.0;
        let mut origin_y = mouse.y - height - 14.0;
        let screens = NSScreen::screens(mtm);
        for index in 0..screens.count() {
            let screen = screens.objectAtIndex(index);
            let frame = screen.frame();
            if mouse.x < frame.origin.x
                || mouse.x > frame.origin.x + frame.size.width
                || mouse.y < frame.origin.y
                || mouse.y > frame.origin.y + frame.size.height
            {
                continue;
            }
            let visible = screen.visibleFrame();
            origin_x = origin_x.clamp(
                visible.origin.x + 8.0,
                (visible.origin.x + visible.size.width - width - 8.0).max(visible.origin.x + 8.0),
            );
            origin_y = origin_y.clamp(
                visible.origin.y + 8.0,
                (visible.origin.y + visible.size.height - height - 8.0).max(visible.origin.y + 8.0),
            );
            break;
        }

        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height)),
        );
        style_layer(&content, color_surface(0.97), 12.0, 1.0, color_accent(0.65));
        let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        label.setFrame(NSRect::new(
            NSPoint::new(14.0, 12.0),
            NSSize::new(width - 28.0, height - 20.0),
        ));
        label.setFont(Some(&NSFont::systemFontOfSize(10.5)));
        style_label(&label, color_text(1.0));
        label.setAlignment(NSTextAlignment::Center);
        content.addSubview(&label);

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(origin_x, origin_y), NSSize::new(width, height)),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        window.setIgnoresMouseEvents(true);
        window.setLevel(NSStatusWindowLevel + 1);
        window.setContentView(Some(&content));
        window.orderFrontRegardless();
        self.ivars().hint_window.replace(Some(window));
        let _: () = unsafe {
            msg_send![
                self,
                performSelector: sel!(hideHint:),
                withObject: self,
                afterDelay: 4.5_f64
            ]
        };
    }

    fn show_settings_popover(&self, button: &NSStatusBarButton) {
        if self.ivars().physics_expanded.get() && !settings_expanded_fits(self.mtm()) {
            self.set_physics_expanded(false);
        }
        if self.ivars().settings_popover.get().is_none() {
            self.build_settings_popover(self.mtm());
        }
        if let Some(popover) = self.ivars().settings_popover.get() {
            popover.showRelativeToRect_ofView_preferredEdge(
                button.bounds(),
                button,
                NSRectEdge::MinY,
            );
        }
    }

    fn set_snowfall_enabled(&self, enabled: bool) {
        self.ivars().settings.borrow_mut().enabled = enabled;
        self.save_settings();
        self.apply_settings();
        self.sync_toggle_controls();
    }

    fn apply_weather_mode(&self, mode: u8) {
        self.ivars().last_preset.set(mode.min(2));
        {
            let mut settings = self.ivars().settings.borrow_mut();
            apply_weather_preset(&mut settings, mode);
            *settings = settings.validate();
        }
        self.sync_weather_buttons();
        self.sync_wind_controls();
        self.sync_slider_controls();
        self.sync_toggle_controls();
        self.save_settings();
        self.apply_settings();
    }

    fn toggle_launch_at_login(&self) {
        let service = unsafe { SMAppService::mainAppService() };
        let status = unsafe { service.status() };
        if status == SMAppServiceStatus::RequiresApproval {
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(
                "x-apple.systempreferences:com.apple.LoginItems-Settings.extension",
            )) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
            return;
        }
        let result = if status == SMAppServiceStatus::Enabled {
            unsafe { service.unregisterAndReturnError() }
        } else {
            unsafe { service.registerAndReturnError() }
        };
        if let Err(error) = result {
            eprintln!("Cannot update login item: {}", error.localizedDescription());
        }
    }

    fn show_status_menu(&self, button: &NSStatusBarButton) {
        let mtm = self.mtm();
        let language = self.ivars().language.get();
        let settings = *self.ivars().settings.borrow();
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Frosty"));
        menu.setAutoenablesItems(false);

        let add_item =
            |title: &str, action: Option<Sel>, tag: isize, checked: bool| -> Retained<NSMenuItem> {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(title),
                        action,
                        &NSString::from_str(""),
                    )
                };
                unsafe { item.setTarget(Some(self)) };
                item.setTag(tag);
                if checked {
                    item.setState(NSControlStateValueOn);
                }
                menu.addItem(&item);
                item
            };

        add_item(
            tr(language, "Настройки", "Settings"),
            Some(sel!(menuOpenSettings:)),
            0,
            false,
        );
        add_item(
            if settings.enabled {
                tr(language, "Пауза", "Pause")
            } else {
                tr(language, "Продолжить", "Resume")
            },
            Some(sel!(menuTogglePause:)),
            1,
            false,
        );

        let snowfall_item = add_item(tr(language, "Снегопад", "Snowfall"), None, 0, false);
        let snowfall_menu = NSMenu::initWithTitle(
            NSMenu::alloc(mtm),
            &NSString::from_str(tr(language, "Снегопад", "Snowfall")),
        );
        snowfall_menu.setAutoenablesItems(false);
        for (mode, title) in [
            (0_u8, tr(language, "Лёгкий", "Light")),
            (1, tr(language, "Спокойный", "Calm")),
            (2, tr(language, "Метель", "Blizzard")),
        ] {
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    Some(sel!(menuWeatherPreset:)),
                    &NSString::from_str(""),
                )
            };
            unsafe { item.setTarget(Some(self)) };
            item.setTag(mode as isize);
            if settings.weather_mode == mode {
                item.setState(NSControlStateValueOn);
            }
            snowfall_menu.addItem(&item);
        }
        snowfall_item.setSubmenu(Some(&snowfall_menu));

        let launch_enabled = launch_at_login_enabled();
        add_item(
            tr(language, "Запускать при входе", "Launch at login"),
            Some(sel!(menuToggleLaunchAtLogin:)),
            2,
            launch_enabled,
        );

        menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_item(
            tr(language, "Выход", "Quit"),
            Some(sel!(menuQuit:)),
            3,
            false,
        );

        menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(0.0, 0.0), Some(button));
    }

    fn build_settings_popover(&self, mtm: MainThreadMarker) {
        let settings = *self.ivars().settings.borrow();
        let expanded = self.ivars().physics_expanded.get();
        let panel_height = if expanded {
            PANEL_HEIGHT_EXPANDED
        } else {
            PANEL_HEIGHT_COLLAPSED
        };
        let size = NSSize::new(PANEL_WIDTH, panel_height);
        let content = PanelBackgroundView::new(mtm, NSRect::new(NSPoint::new(0.0, 0.0), size));
        *self.ivars().settings_content.borrow_mut() = Some(content.clone());
        let language = self.ivars().language.get();

        // Header: the real app icon, wordmark, master switch and quit.
        let logo = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            panel_frame(PANEL_PADDING, HEADER_TOP + 3.0, 38.0, 38.0),
        );
        logo.setImage(app_icon_image().as_deref());
        logo.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        content.addSubview(&logo);

        let title = NSTextField::labelWithString(ns_string!("Frosty"), mtm);
        title.setFrame(panel_frame(
            PANEL_PADDING + 50.0,
            HEADER_TOP + 9.0,
            220.0,
            24.0,
        ));
        style_label(&title, color_text(1.0));
        title.setFont(Some(&NSFont::boldSystemFontOfSize(19.0)));
        content.addSubview(&title);

        self.add_icon_button(
            mtm,
            &content,
            symbol_image("xmark", 12.0, SymbolWeight::Semibold),
            2,
            panel_frame(
                PANEL_WIDTH - PANEL_PADDING - 34.0,
                HEADER_TOP + 5.0,
                34.0,
                34.0,
            ),
            sel!(quitApp:),
            tr(language, "Выйти", "Quit"),
        );

        let header_divider = NSView::initWithFrame(
            NSView::alloc(mtm),
            panel_frame(
                PANEL_PADDING,
                HEADER_TOP + HEADER_HEIGHT + 4.0,
                PANEL_CONTENT_WIDTH,
                1.0,
            ),
        );
        style_layer(
            &header_divider,
            color_surface(0.55),
            0.5,
            0.0,
            color_clear(),
        );
        content.addSubview(&header_divider);

        // Master on/off switch lives in the header, next to quit, with no labels.
        let switch_width = 38.0;
        let switch_height = 20.0;
        let switch_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            panel_frame(
                PANEL_WIDTH - PANEL_PADDING - 34.0 - 10.0 - switch_width,
                HEADER_TOP + 12.0,
                switch_width,
                switch_height,
            ),
        );
        let knob = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(3.0, 2.0), NSSize::new(16.0, 16.0)),
        );
        switch_view.addSubview(&knob);
        let toggle = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(switch_width, switch_height),
            ),
        );
        toggle.setBordered(false);
        toggle.setButtonType(NSButtonType::PushOnPushOff);
        toggle.setTitle(ns_string!(""));
        toggle.setState(if settings.enabled {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        toggle.setTag(64);
        toggle.setToolTip(Some(&NSString::from_str(tr(
            language,
            "Включить или выключить снегопад",
            "Turn snowfall on or off",
        ))));
        toggle.setFocusRingType(NSFocusRingType::None);
        unsafe {
            toggle.setTarget(Some(self));
            toggle.setAction(Some(sel!(toggleSnowOption:)));
        }
        switch_view.addSubview(&toggle);
        content.addSubview(&switch_view);
        let master = ToggleControl {
            tag: 64,
            button: toggle,
            container: switch_view,
            knob,
            switch_width,
            switch_height,
            knob_size: 16.0,
        };
        style_toggle_control(&master, settings.enabled);
        self.ivars().toggle_controls.borrow_mut().push(master);

        // Weather presets.
        add_section_title(
            mtm,
            &content,
            tr(language, "СНЕГОПАД", "SNOWFALL"),
            WEATHER_TITLE_TOP,
        );
        let weather_gap = 10.0;
        let weather_width = (PANEL_CONTENT_WIDTH - weather_gap * 2.0) / 3.0;
        for (index, (label, icon, tag)) in [
            (tr(language, "Лёгкий", "Light"), "snowflake", 20),
            (tr(language, "Спокойный", "Calm"), "cloud.snow.fill", 21),
            (tr(language, "Метель", "Blizzard"), "wind", 22),
        ]
        .into_iter()
        .enumerate()
        {
            self.add_weather_card(
                mtm,
                &content,
                label,
                icon,
                tag,
                panel_frame(
                    PANEL_PADDING + index as f64 * (weather_width + weather_gap),
                    WEATHER_TOP,
                    weather_width,
                    WEATHER_HEIGHT,
                ),
                isize::from(settings.weather_mode) == tag - 20,
            );
        }

        // Preset summary, custom indicator and reset actions.
        let badge = NSTextField::labelWithString(
            &NSString::from_str(tr(language, "ИЗМЕНЁННЫЙ ПРЕСЕТ", "CUSTOM PRESET")),
            mtm,
        );
        badge.setFrame(panel_frame(
            PANEL_WIDTH - PANEL_PADDING - 116.0,
            WEATHER_TITLE_TOP - 1.0,
            116.0,
            16.0,
        ));
        style_label(&badge, color_accent(1.0));
        badge.setFont(Some(&NSFont::boldSystemFontOfSize(8.0)));
        badge.setAlignment(NSTextAlignment::Right);
        badge.setHidden(settings.weather_mode <= 2);
        content.addSubview(&badge);
        *self.ivars().weather_badge.borrow_mut() = Some(badge);

        let weather_info = NSTextField::labelWithString(
            &NSString::from_str(&weather_summary(&settings, language)),
            mtm,
        );
        weather_info.setFrame(panel_frame(
            PANEL_PADDING,
            WEATHER_META_TOP,
            PANEL_CONTENT_WIDTH - 92.0,
            WEATHER_META_HEIGHT,
        ));
        style_label(&weather_info, color_accent(0.90));
        weather_info.setFont(Some(&NSFont::systemFontOfSize(9.0)));
        content.addSubview(&weather_info);
        *self.ivars().weather_info.borrow_mut() = Some(weather_info);

        let reset = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            panel_frame(
                PANEL_WIDTH - PANEL_PADDING - 78.0,
                WEATHER_META_TOP - 2.0,
                78.0,
                22.0,
            ),
        );
        reset.setBordered(false);
        reset.setButtonType(NSButtonType::MomentaryPushIn);
        reset.setTitle(&NSString::from_str(tr(language, "Сбросить", "Reset")));
        reset.setFocusRingType(NSFocusRingType::None);
        style_small_action(&reset);
        reset.setToolTip(Some(&NSString::from_str(tr(
            language,
            "Вернуть значения выбранного пресета",
            "Restore the selected preset",
        ))));
        unsafe {
            reset.setTarget(Some(self));
            reset.setAction(Some(sel!(resetWeatherPreset:)));
        }
        content.addSubview(&reset);

        let hint = NSTextField::labelWithString(
            &NSString::from_str(tr(
                language,
                "Ручная правка физики включает режим «Свой».",
                "Manual physics changes switch to Custom.",
            )),
            mtm,
        );
        hint.setFrame(panel_frame(
            PANEL_PADDING,
            WEATHER_HINT_TOP,
            PANEL_CONTENT_WIDTH - 140.0,
            WEATHER_HINT_HEIGHT,
        ));
        style_label(&hint, color_accent(0.70));
        hint.setFont(Some(&NSFont::systemFontOfSize(8.5)));
        content.addSubview(&hint);

        let defaults = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            panel_frame(
                PANEL_WIDTH - PANEL_PADDING - 110.0,
                WEATHER_HINT_TOP - 3.0,
                110.0,
                20.0,
            ),
        );
        defaults.setBordered(false);
        defaults.setButtonType(NSButtonType::MomentaryPushIn);
        defaults.setTitle(&NSString::from_str(tr(
            language,
            "По умолчанию",
            "Defaults",
        )));
        defaults.setFocusRingType(NSFocusRingType::None);
        style_small_action(&defaults);
        defaults.setToolTip(Some(&NSString::from_str(tr(
            language,
            "Сбросить все настройки к значениям по умолчанию",
            "Reset all settings to their defaults",
        ))));
        unsafe {
            defaults.setTarget(Some(self));
            defaults.setAction(Some(sel!(resetAllSettings:)));
        }
        content.addSubview(&defaults);

        // Cursor tools.
        add_section_title(
            mtm,
            &content,
            tr(language, "ИНСТРУМЕНТ", "TOOL"),
            TOOL_TITLE_TOP,
        );
        let tool_gap = 10.0;
        let tool_width = (PANEL_CONTENT_WIDTH - tool_gap) / 2.0;
        let current_tool = self.ivars().cursor_tool.get();
        self.add_tool_card(
            mtm,
            &content,
            tr(language, "Варежка", "Mitten"),
            tr(language, "Игра со снегом", "Play with snow"),
            "hand.raised.fill",
            30,
            panel_frame(PANEL_PADDING, TOOL_TOP, tool_width, TOOL_HEIGHT),
            current_tool == 30,
        );
        self.add_tool_card(
            mtm,
            &content,
            tr(language, "Лопата", "Shovel"),
            tr(language, "Уборка снега", "Clear snow"),
            "paintbrush.pointed.fill",
            31,
            panel_frame(
                PANEL_PADDING + tool_width + tool_gap,
                TOOL_TOP,
                tool_width,
                TOOL_HEIGHT,
            ),
            current_tool == 31,
        );

        let tool_hint = NSTextField::labelWithString(
            &NSString::from_str(tool_hint_text(current_tool, language)),
            mtm,
        );
        tool_hint.setFrame(panel_frame(
            PANEL_PADDING,
            TOOL_HINT_TOP,
            PANEL_CONTENT_WIDTH,
            TOOL_HINT_HEIGHT,
        ));
        style_label(&tool_hint, color_accent(0.85));
        tool_hint.setFont(Some(&NSFont::systemFontOfSize(10.0)));
        tool_hint.setAlignment(NSTextAlignment::Center);
        content.addSubview(&tool_hint);
        self.ivars().tool_hint.set(tool_hint).unwrap();

        // Wind card: compact compass on the left, strength/gust sliders right.
        add_section_title(mtm, &content, tr(language, "ВЕТЕР", "WIND"), WIND_TITLE_TOP);
        let wind_status = NSTextField::labelWithString(
            &NSString::from_str(wind_description(settings.wind_strength, language)),
            mtm,
        );
        style_label(&wind_status, color_accent(1.0));
        wind_status.setFont(Some(&NSFont::systemFontOfSize(10.5)));
        wind_status.setAlignment(NSTextAlignment::Right);
        wind_status.setFrame(panel_frame(
            PANEL_WIDTH - PANEL_PADDING - 150.0,
            WIND_TITLE_TOP,
            150.0,
            SECTION_TITLE_HEIGHT,
        ));
        content.addSubview(&wind_status);

        let compass_card = NSView::initWithFrame(
            NSView::alloc(mtm),
            panel_frame(
                PANEL_PADDING,
                COMPASS_CARD_TOP,
                PANEL_CONTENT_WIDTH,
                COMPASS_CARD_HEIGHT,
            ),
        );
        style_layer(
            &compass_card,
            color_surface(0.34),
            14.0,
            1.0,
            color_surface(0.65),
        );
        content.addSubview(&compass_card);

        let center_x = COMPASS_CENTER_X;
        let center_top = COMPASS_CARD_TOP + COMPASS_CARD_HEIGHT / 2.0;
        let ring = NSView::initWithFrame(
            NSView::alloc(mtm),
            panel_frame(
                center_x - COMPASS_RADIUS,
                center_top - COMPASS_RADIUS,
                COMPASS_RADIUS * 2.0,
                COMPASS_RADIUS * 2.0,
            ),
        );
        style_ring(&ring, color_accent(0.38), 1.0, COMPASS_RADIUS);
        content.addSubview(&ring);

        let dot_count = 32;
        for index in 0..dot_count {
            let angle = index as f64 / dot_count as f64 * std::f64::consts::TAU;
            let dot = NSView::initWithFrame(
                NSView::alloc(mtm),
                panel_frame(
                    center_x + angle.cos() * COMPASS_INNER_RADIUS - 1.25,
                    center_top + angle.sin() * COMPASS_INNER_RADIUS - 1.25,
                    2.5,
                    2.5,
                ),
            );
            style_layer(&dot, color_accent(0.45), 1.25, 0.0, color_clear());
            content.addSubview(&dot);
        }
        for (x, top, width, height) in [
            (center_x - 0.5, center_top - 32.0, 1.0, 64.0),
            (center_x - 32.0, center_top - 0.5, 64.0, 1.0),
        ] {
            let guide =
                NSView::initWithFrame(NSView::alloc(mtm), panel_frame(x, top, width, height));
            style_layer(&guide, color_accent(0.12), 0.5, 0.0, color_clear());
            content.addSubview(&guide);
        }
        for (label, x, top) in [
            (tr(language, "С", "N"), center_x - 8.0, center_top - 58.0),
            (tr(language, "Ю", "S"), center_x - 8.0, center_top + 46.0),
            (tr(language, "З", "W"), center_x - 60.0, center_top - 7.0),
            (tr(language, "В", "E"), center_x + 48.0, center_top - 7.0),
        ] {
            let cardinal = NSTextField::labelWithString(&NSString::from_str(label), mtm);
            cardinal.setFrame(panel_frame(x, top, 16.0, 14.0));
            style_label(&cardinal, color_accent(0.95));
            cardinal.setFont(Some(&NSFont::systemFontOfSize(9.5)));
            cardinal.setAlignment(NSTextAlignment::Center);
            content.addSubview(&cardinal);
        }

        let calm = settings.wind_strength <= 0.0;
        let selected_direction = direction_tag(settings.wind_direction);
        for (icon, tag, column, row) in [
            ("arrow.up.left", 40, -1.0, -1.0),
            ("arrow.up", 41, 0.0, -1.0),
            ("arrow.up.right", 42, 1.0, -1.0),
            ("arrow.left", 43, -1.0, 0.0),
            ("arrow.right", 44, 1.0, 0.0),
            ("arrow.down.left", 45, -1.0, 1.0),
            ("arrow.down", 46, 0.0, 1.0),
            ("arrow.down.right", 47, 1.0, 1.0),
        ] {
            let selected = !calm && tag == selected_direction;
            self.add_compass_button(
                mtm,
                &content,
                icon,
                tag,
                11.0,
                panel_frame(
                    center_x + column * COMPASS_STEP_X - 13.0,
                    center_top + row * COMPASS_STEP_Y - 13.0,
                    26.0,
                    26.0,
                ),
                selected,
            );
        }
        self.add_compass_button(
            mtm,
            &content,
            "circle.fill",
            WIND_CALM_TAG,
            8.0,
            panel_frame(center_x - 10.0, center_top - 10.0, 20.0, 20.0),
            calm,
        );

        let wind_divider = NSView::initWithFrame(
            NSView::alloc(mtm),
            panel_frame(
                PANEL_PADDING + WIND_CARD_LEFT_WIDTH + 2.0,
                COMPASS_CARD_TOP + 18.0,
                1.0,
                COMPASS_CARD_HEIGHT - 36.0,
            ),
        );
        style_layer(&wind_divider, color_surface(0.55), 0.5, 0.0, color_clear());
        content.addSubview(&wind_divider);
        self.add_slider(
            mtm,
            &content,
            SettingSliderConfig {
                title: tr(language, "Сила ветра", "Wind strength"),
                tag: 94,
                value: settings.wind_strength as f64,
                minimum: 0.0,
                maximum: MAX_WIND_STRENGTH as f64,
                top: WIND_SLIDER_FIRST_TOP,
                x: WIND_SLIDER_X,
                width: WIND_SLIDER_WIDTH,
            },
        );
        self.add_slider(
            mtm,
            &content,
            SettingSliderConfig {
                title: tr(language, "Порывы", "Gusts"),
                tag: 95,
                value: settings.wind_turbulence as f64,
                minimum: 0.0,
                maximum: MAX_WIND_TURBULENCE as f64,
                top: WIND_SLIDER_SECOND_TOP,
                x: WIND_SLIDER_X,
                width: WIND_SLIDER_WIDTH,
            },
        );

        // Two visible window toggles, as in the concept.
        let toggle_width = (PANEL_CONTENT_WIDTH - 10.0) / 2.0;
        self.add_toggle_row(
            mtm,
            &content,
            62,
            tr(language, "Снег на окнах", "Snow on windows"),
            toggle_tooltip(62, language),
            panel_frame(PANEL_PADDING, TOGGLES_TOP, toggle_width, TOGGLE_HEIGHT),
            settings.window_snow,
        );
        self.add_toggle_row(
            mtm,
            &content,
            60,
            tr(language, "Частицы в окнах", "Particles in windows"),
            toggle_tooltip(60, language),
            panel_frame(
                PANEL_PADDING + toggle_width + 10.0,
                TOGGLES_TOP,
                toggle_width,
                TOGGLE_HEIGHT,
            ),
            settings.particles_in_windows,
        );

        // Clear action.
        let clear = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            panel_frame(PANEL_PADDING, CLEAR_TOP, PANEL_CONTENT_WIDTH, CLEAR_HEIGHT),
        );
        clear.setBordered(false);
        clear.setButtonType(NSButtonType::MomentaryPushIn);
        clear.setTitle(&NSString::from_str(tr(language, "Очистить", "Clear")));
        clear.setImage(symbol_image("sparkles", 12.0, SymbolWeight::Medium).as_deref());
        clear.setImagePosition(NSCellImagePosition::ImageLeft);
        clear.setImageHugsTitle(true);
        clear.setContentTintColor(Some(&color_text(1.0)));
        clear.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        clear.setToolTip(Some(&NSString::from_str(tr(
            language,
            "Убрать сугробы, шарики и снеговиков",
            "Remove drifts, snowballs and snowmen",
        ))));
        clear.setTag(50);
        clear.setFocusRingType(NSFocusRingType::None);
        style_settings_button(&clear, false);
        unsafe {
            clear.setTarget(Some(self));
            clear.setAction(Some(sel!(clearSnow:)));
        }
        content.addSubview(&clear);
        self.ivars().settings_buttons.borrow_mut().push(clear);

        // Disclosure row for physics.
        let expand = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            panel_frame(
                PANEL_PADDING,
                ADDITIONAL_TOP,
                PANEL_CONTENT_WIDTH,
                ADDITIONAL_HEIGHT,
            ),
        );
        expand.setBordered(false);
        expand.setButtonType(NSButtonType::MomentaryPushIn);
        expand.setTitle(&NSString::from_str(tr(
            language,
            "Дополнительно",
            "Advanced",
        )));
        expand.setImage(
            symbol_image(
                if expanded {
                    "chevron.up"
                } else {
                    "chevron.down"
                },
                10.0,
                SymbolWeight::Semibold,
            )
            .as_deref(),
        );
        expand.setImagePosition(NSCellImagePosition::ImageLeft);
        expand.setImageHugsTitle(true);
        expand.setAlignment(NSTextAlignment::Center);
        expand.setContentTintColor(Some(&color_accent(1.0)));
        expand.setFont(Some(&NSFont::systemFontOfSize(11.5)));
        expand.setTag(80);
        expand.setToolTip(Some(&NSString::from_str(tr(
            language,
            "Показать или скрыть физику",
            "Show or hide physics",
        ))));
        expand.setFocusRingType(NSFocusRingType::None);
        style_layer(&expand, color_surface(0.20), 10.0, 1.0, color_accent(0.18));
        unsafe {
            expand.setTarget(Some(self));
            expand.setAction(Some(sel!(togglePhysics:)));
        }
        content.addSubview(&expand);
        *self.ivars().expand_button.borrow_mut() = Some(expand);

        // Physics section inside the disclosure, hidden when collapsed.
        let physics = FlippedSectionView::new(
            mtm,
            NSRect::new(
                NSPoint::new(0.0, PHYSICS_TOP),
                NSSize::new(PANEL_WIDTH, PHYSICS_HEIGHT),
            ),
        );
        physics.setHidden(!expanded);
        content.addSubview(&physics);
        let column_gap = 10.0;
        let column_width = (PANEL_CONTENT_WIDTH - column_gap) / 2.0;
        let left_x = PANEL_PADDING;
        let right_x = PANEL_PADDING + column_width + column_gap;
        self.add_slider(
            mtm,
            &physics,
            SettingSliderConfig {
                title: tr(language, "Интенсивность", "Intensity"),
                tag: 92,
                value: settings.intensity as f64,
                minimum: 0.2,
                maximum: 1.0,
                top: 0.0,
                x: left_x,
                width: column_width,
            },
        );
        self.add_slider(
            mtm,
            &physics,
            SettingSliderConfig {
                title: tr(language, "Скорость падения", "Fall speed"),
                tag: 93,
                value: settings.falling_speed as f64,
                minimum: 0.25,
                maximum: 2.0,
                top: 0.0,
                x: right_x,
                width: column_width,
            },
        );
        self.add_slider(
            mtm,
            &physics,
            SettingSliderConfig {
                title: tr(language, "Скорость накопления", "Accumulation speed"),
                tag: 90,
                value: settings.accumulation_speed as f64,
                minimum: 0.0,
                maximum: 1.0,
                top: SLIDER_ROW_HEIGHT,
                x: left_x,
                width: column_width,
            },
        );
        self.add_slider(
            mtm,
            &physics,
            SettingSliderConfig {
                title: tr(language, "Количество частиц", "Particles"),
                tag: 91,
                value: settings.particle_count as f64,
                minimum: 500.0,
                maximum: 20_000.0,
                top: SLIDER_ROW_HEIGHT,
                x: right_x,
                width: column_width,
            },
        );
        let secondary_top = PHYSICS_TOGGLE_TOP - PHYSICS_TOP;
        self.add_toggle_row(
            mtm,
            &physics,
            63,
            tr(language, "Накопление", "Accumulation"),
            toggle_tooltip(63, language),
            panel_frame(left_x, secondary_top, column_width, 40.0),
            settings.accumulation,
        );
        self.add_toggle_row(
            mtm,
            &physics,
            61,
            tr(language, "Порывы у окон", "Window drafts"),
            toggle_tooltip(61, language),
            panel_frame(right_x, secondary_top, column_width, 40.0),
            settings.window_wind_draft,
        );
        *self.ivars().physics_section.borrow_mut() = Some(physics);

        // About footer inside its own movable flipped section.
        let about_top = if expanded {
            ABOUT_TOP_EXPANDED
        } else {
            ABOUT_TOP_COLLAPSED
        };
        let about = FlippedSectionView::new(
            mtm,
            NSRect::new(
                NSPoint::new(0.0, about_top),
                NSSize::new(PANEL_WIDTH, ABOUT_HEIGHT),
            ),
        );
        content.addSubview(&about);
        let about_divider = NSView::initWithFrame(
            NSView::alloc(mtm),
            panel_frame(PANEL_PADDING, 0.0, PANEL_CONTENT_WIDTH, 1.0),
        );
        style_layer(&about_divider, color_surface(0.65), 0.5, 0.0, color_clear());
        about.addSubview(&about_divider);
        let about_icon = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            panel_frame(PANEL_PADDING, 9.0, 30.0, 30.0),
        );
        about_icon.setImage(app_icon_image().as_deref());
        about_icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        about.addSubview(&about_icon);

        let about_name = NSTextField::labelWithString(ns_string!("Frosty"), mtm);
        about_name.setFrame(panel_frame(60.0, 9.0, 180.0, 15.0));
        style_label(&about_name, color_text(1.0));
        about_name.setFont(Some(&NSFont::boldSystemFontOfSize(11.5)));
        about.addSubview(&about_name);
        let about_meta = NSTextField::labelWithString(
            &NSString::from_str(&format!("v{APP_VERSION} · {APP_AUTHOR}")),
            mtm,
        );
        about_meta.setFrame(panel_frame(60.0, 25.0, 220.0, 13.0));
        style_label(&about_meta, color_accent(0.85));
        about_meta.setFont(Some(&NSFont::systemFontOfSize(9.0)));
        about.addSubview(&about_meta);

        for (index, (label, url)) in DONATION_LINKS.iter().enumerate() {
            let link = NSButton::initWithFrame(
                NSButton::alloc(mtm),
                panel_frame(
                    PANEL_WIDTH - PANEL_PADDING - 120.0 + index as f64 * 60.0,
                    17.0,
                    60.0,
                    16.0,
                ),
            );
            link.setBordered(false);
            link.setButtonType(NSButtonType::MomentaryPushIn);
            link.setTitle(&NSString::from_str(label));
            link.setContentTintColor(Some(&color_accent(1.0)));
            link.setFont(Some(&NSFont::systemFontOfSize(9.5)));
            link.setAlignment(NSTextAlignment::Center);
            link.setTag(70 + index as isize);
            link.setFocusRingType(NSFocusRingType::None);
            link.setToolTip(Some(&NSString::from_str(if url.is_empty() {
                tr(language, "Ссылка доната будет добавлена", "Donation link")
            } else {
                tr(language, "Поддержать проект", "Support the project")
            })));
            unsafe {
                link.setTarget(Some(self));
                link.setAction(Some(sel!(openDonation:)));
            }
            about.addSubview(&link);
        }
        *self.ivars().about_section.borrow_mut() = Some(about);

        self.ivars().wind_status.set(wind_status).unwrap();

        let controller: Retained<NSViewController> =
            unsafe { msg_send![NSViewController::alloc(mtm), init] };
        controller.setView(&content);
        let popover = NSPopover::init(NSPopover::alloc(mtm));
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentSize(size);
        popover.setContentViewController(Some(&controller));
        let dark_appearance: Option<Retained<AnyObject>> = unsafe {
            msg_send![objc2::class!(NSAppearance), appearanceNamed: ns_string!("NSAppearanceNameDarkAqua")]
        };
        let _: () = unsafe { msg_send![&*popover, setAppearance: dark_appearance.as_deref()] };
        self.ivars().settings_controller.set(controller).unwrap();
        self.ivars().settings_popover.set(popover).unwrap();
    }

    fn set_physics_expanded(&self, expanded: bool) {
        if expanded && !settings_expanded_fits(self.mtm()) {
            return;
        }
        self.ivars().physics_expanded.set(expanded);
        if let Some(section) = self.ivars().physics_section.borrow().as_ref() {
            section.setHidden(!expanded);
        }
        let about_top = if expanded {
            ABOUT_TOP_EXPANDED
        } else {
            ABOUT_TOP_COLLAPSED
        };
        if let Some(about) = self.ivars().about_section.borrow().as_ref() {
            about.setFrameOrigin(NSPoint::new(0.0, about_top));
        }
        let height = if expanded {
            PANEL_HEIGHT_EXPANDED
        } else {
            PANEL_HEIGHT_COLLAPSED
        };
        if let Some(content) = self.ivars().settings_content.borrow().as_ref() {
            content.setFrameSize(NSSize::new(PANEL_WIDTH, height));
            content.setNeedsDisplay(true);
        }
        if let Some(popover) = self.ivars().settings_popover.get() {
            popover.setContentSize(NSSize::new(PANEL_WIDTH, height));
        }
        if let Some(button) = self.ivars().expand_button.borrow().as_ref() {
            button.setImage(
                symbol_image(
                    if expanded {
                        "chevron.up"
                    } else {
                        "chevron.down"
                    },
                    10.0,
                    SymbolWeight::Semibold,
                )
                .as_deref(),
            );
        }
    }

    fn sync_weather_buttons(&self) {
        let settings = *self.ivars().settings.borrow();
        let selected = if settings.weather_mode <= 2 {
            20 + isize::from(settings.weather_mode)
        } else {
            -1
        };
        self.select_button_group(20, 22, selected);
    }

    fn sync_slider_controls(&self) {
        let settings = *self.ivars().settings.borrow();
        for control in self.ivars().sliders.borrow().iter() {
            let value = match control.tag {
                90 => settings.accumulation_speed as f64,
                91 => settings.particle_count as f64,
                92 => settings.intensity as f64,
                93 => settings.falling_speed as f64,
                94 => settings.wind_strength as f64,
                95 => settings.wind_turbulence as f64,
                _ => continue,
            };
            control.slider.setDoubleValue(value);
            control
                .value_label
                .setStringValue(&NSString::from_str(&slider_label_value(control.tag, value)));
        }
    }

    fn sync_toggle_controls(&self) {
        let settings = *self.ivars().settings.borrow();
        for control in self.ivars().toggle_controls.borrow().iter() {
            let enabled = match control.tag {
                60 => settings.particles_in_windows,
                61 => settings.window_wind_draft,
                62 => settings.window_snow,
                63 => settings.accumulation,
                64 => settings.enabled,
                _ => continue,
            };
            style_toggle_control(control, enabled);
        }
    }

    fn style_cards(&self) {
        for card in self.ivars().cards.borrow().iter() {
            style_card(card, card.button.state() == NSControlStateValueOn);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_icon_button(
        &self,
        mtm: MainThreadMarker,
        content: &NSView,
        image: Option<Retained<NSImage>>,
        tag: isize,
        frame: NSRect,
        action: objc2::runtime::Sel,
        tooltip: &str,
    ) -> Retained<NSButton> {
        let button = NSButton::initWithFrame(NSButton::alloc(mtm), frame);
        button.setBordered(false);
        button.setButtonType(if matches!(tag, 1..=2) {
            NSButtonType::MomentaryPushIn
        } else {
            NSButtonType::PushOnPushOff
        });
        button.setImage(image.as_deref());
        button.setImagePosition(NSCellImagePosition::ImageOnly);
        button.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        button.setContentTintColor(Some(&color_text(1.0)));
        button.setToolTip(Some(&NSString::from_str(tooltip)));
        button.setTag(tag);
        button.setFocusRingType(NSFocusRingType::None);
        style_settings_button(&button, false);
        unsafe {
            button.setTarget(Some(self));
            button.setAction(Some(action));
        }
        content.addSubview(&button);
        self.ivars()
            .settings_buttons
            .borrow_mut()
            .push(button.clone());
        button
    }

    #[allow(clippy::too_many_arguments)]
    fn add_compass_button(
        &self,
        mtm: MainThreadMarker,
        content: &NSView,
        symbol: &str,
        tag: isize,
        symbol_size: f64,
        frame: NSRect,
        selected: bool,
    ) -> Retained<NSButton> {
        let button = NSButton::initWithFrame(NSButton::alloc(mtm), frame);
        button.setBordered(false);
        button.setButtonType(NSButtonType::PushOnPushOff);
        button.setImage(symbol_image(symbol, symbol_size, SymbolWeight::Medium).as_deref());
        button.setImagePosition(NSCellImagePosition::ImageOnly);
        button.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        button.setContentTintColor(Some(&color_text(1.0)));
        button.setTag(tag);
        button.setState(if selected {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        button.setFocusRingType(NSFocusRingType::None);
        style_settings_button(&button, selected);
        unsafe {
            button.setTarget(Some(self));
            button.setAction(Some(sel!(selectWind:)));
        }
        content.addSubview(&button);
        self.ivars()
            .settings_buttons
            .borrow_mut()
            .push(button.clone());
        button
    }

    #[allow(clippy::too_many_arguments)]
    fn add_weather_card(
        &self,
        mtm: MainThreadMarker,
        content: &NSView,
        label: &str,
        symbol: &str,
        tag: isize,
        frame: NSRect,
        selected: bool,
    ) {
        let width = frame.size.width;
        let height = frame.size.height;
        let container = NSView::initWithFrame(NSView::alloc(mtm), frame);
        content.addSubview(&container);

        let icon = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            top_frame(height, (width - 22.0) / 2.0, 7.0, 22.0, 22.0),
        );
        icon.setImage(symbol_image(symbol, 19.0, SymbolWeight::Medium).as_deref());
        icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        container.addSubview(&icon);

        let title = NSTextField::labelWithString(&NSString::from_str(label), mtm);
        title.setFrame(top_frame(height, 4.0, 30.0, width - 8.0, 16.0));
        title.setAlignment(NSTextAlignment::Center);
        title.setFont(Some(&NSFont::systemFontOfSize(10.5)));
        container.addSubview(&title);

        let button = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height)),
        );
        button.setBordered(false);
        button.setButtonType(NSButtonType::PushOnPushOff);
        button.setTitle(ns_string!(""));
        button.setTag(tag);
        button.setState(if selected {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        button.setToolTip(Some(&NSString::from_str(weather_tooltip(
            tag,
            self.ivars().language.get(),
        ))));
        button.setFocusRingType(NSFocusRingType::None);
        unsafe {
            button.setTarget(Some(self));
            button.setAction(Some(sel!(selectWeather:)));
        }
        container.addSubview(&button);

        let card = SettingsCard {
            container,
            icon,
            title,
            subtitle: None,
            button: button.clone(),
            is_tool: false,
        };
        style_card(&card, selected);
        self.ivars().cards.borrow_mut().push(card);
        self.ivars().settings_buttons.borrow_mut().push(button);
    }

    #[allow(clippy::too_many_arguments)]
    fn add_tool_card(
        &self,
        mtm: MainThreadMarker,
        content: &NSView,
        label: &str,
        detail: &str,
        symbol: &str,
        tag: isize,
        frame: NSRect,
        selected: bool,
    ) {
        let width = frame.size.width;
        let height = frame.size.height;
        let container = NSView::initWithFrame(NSView::alloc(mtm), frame);
        content.addSubview(&container);

        let icon = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            top_frame(height, 10.0, (height - 34.0) / 2.0, 34.0, 34.0),
        );
        icon.setImage(symbol_image(symbol, 26.0, SymbolWeight::Medium).as_deref());
        icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        container.addSubview(&icon);

        let title = NSTextField::labelWithString(&NSString::from_str(label), mtm);
        title.setFrame(top_frame(height, 52.0, 9.0, width - 62.0, 16.0));
        title.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
        container.addSubview(&title);

        let subtitle = NSTextField::labelWithString(&NSString::from_str(detail), mtm);
        subtitle.setFrame(top_frame(height, 52.0, 26.0, width - 62.0, 14.0));
        subtitle.setFont(Some(&NSFont::systemFontOfSize(9.5)));
        container.addSubview(&subtitle);

        let button = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height)),
        );
        button.setBordered(false);
        button.setButtonType(NSButtonType::PushOnPushOff);
        button.setTitle(ns_string!(""));
        button.setTag(tag);
        button.setState(if selected {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        button.setToolTip(Some(&NSString::from_str(tool_tooltip(
            tag,
            self.ivars().language.get(),
        ))));
        button.setFocusRingType(NSFocusRingType::None);
        unsafe {
            button.setTarget(Some(self));
            button.setAction(Some(sel!(selectTool:)));
        }
        container.addSubview(&button);

        let card = SettingsCard {
            container,
            icon,
            title,
            subtitle: Some(subtitle),
            button: button.clone(),
            is_tool: true,
        };
        style_card(&card, selected);
        self.ivars().cards.borrow_mut().push(card);
        self.ivars().settings_buttons.borrow_mut().push(button);
    }

    fn add_slider(&self, mtm: MainThreadMarker, content: &NSView, config: SettingSliderConfig) {
        let title_label = NSTextField::labelWithString(&NSString::from_str(config.title), mtm);
        title_label.setFrame(panel_frame(config.x, config.top, config.width - 58.0, 18.0));
        style_label(&title_label, color_accent(0.95));
        title_label.setFont(Some(&NSFont::systemFontOfSize(11.5)));
        content.addSubview(&title_label);

        let value_label = NSTextField::labelWithString(
            &NSString::from_str(&slider_label_value(config.tag, config.value)),
            mtm,
        );
        value_label.setFrame(panel_frame(
            config.x + config.width - 52.0,
            config.top,
            52.0,
            15.0,
        ));
        value_label.setAlignment(NSTextAlignment::Right);
        style_label(&value_label, color_text(1.0));
        value_label.setFont(Some(&NSFont::systemFontOfSize(10.0)));
        content.addSubview(&value_label);

        let slider = NSSlider::initWithFrame(
            NSSlider::alloc(mtm),
            panel_frame(config.x, config.top + 17.0, config.width, 19.0),
        );
        slider.setMinValue(config.minimum);
        slider.setMaxValue(config.maximum);
        slider.setDoubleValue(config.value);
        slider.setTag(config.tag);
        slider.setContinuous(false);
        slider.setTrackFillColor(Some(&color_accent(1.0)));
        slider.setFocusRingType(NSFocusRingType::None);
        slider.setToolTip(Some(&NSString::from_str(slider_tooltip(
            config.tag,
            self.ivars().language.get(),
        ))));
        unsafe {
            slider.setTarget(Some(self));
            slider.setAction(Some(sel!(changeSnowSetting:)));
        }
        content.addSubview(&slider);
        self.ivars().sliders.borrow_mut().push(SliderControl {
            tag: config.tag,
            slider,
            value_label,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn add_toggle_row(
        &self,
        mtm: MainThreadMarker,
        content: &NSView,
        tag: isize,
        label: &str,
        tooltip: &str,
        frame: NSRect,
        value: bool,
    ) {
        let width = frame.size.width;
        let height = frame.size.height;
        let container = NSView::initWithFrame(NSView::alloc(mtm), frame);
        style_layer(
            &container,
            color_surface(0.34),
            10.0,
            0.8,
            color_surface(0.60),
        );
        container.setToolTip(Some(&NSString::from_str(tooltip)));
        content.addSubview(&container);

        let icon_name = match tag {
            60 => "snowflake",
            61 => "wind",
            62 => "macwindow",
            63 => "square.stack.3d.up",
            _ => "circle",
        };
        let icon = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            top_frame(height, 12.0, (height - 13.0) / 2.0, 13.0, 13.0),
        );
        icon.setImage(symbol_image(icon_name, 11.0, SymbolWeight::Medium).as_deref());
        icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        icon.setContentTintColor(Some(&color_accent(0.90)));
        container.addSubview(&icon);

        let title = NSTextField::labelWithString(&NSString::from_str(label), mtm);
        title.setFrame(top_frame(
            height,
            30.0,
            14.0,
            width - 30.0 - 14.0 - 38.0 - 8.0,
            16.0,
        ));
        style_label(&title, color_text(1.0));
        title.setFont(Some(&NSFont::systemFontOfSize(10.5)));
        container.addSubview(&title);

        let switch_height = 20.0;
        let switch_width = 38.0;
        let switch_frame = top_frame(
            height,
            width - 14.0 - switch_width,
            (height - switch_height) / 2.0,
            switch_width,
            switch_height,
        );
        let switch_view = NSView::initWithFrame(NSView::alloc(mtm), switch_frame);
        switch_view.setToolTip(Some(&NSString::from_str(tooltip)));
        let knob = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(3.0, 2.0), NSSize::new(16.0, 16.0)),
        );
        switch_view.addSubview(&knob);
        let toggle = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(switch_width, switch_height),
            ),
        );
        toggle.setBordered(false);
        toggle.setButtonType(NSButtonType::PushOnPushOff);
        toggle.setTitle(ns_string!(""));
        toggle.setState(if value {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        toggle.setTag(tag);
        toggle.setToolTip(Some(&NSString::from_str(tooltip)));
        toggle.setFocusRingType(NSFocusRingType::None);
        unsafe {
            toggle.setTarget(Some(self));
            toggle.setAction(Some(sel!(toggleSnowOption:)));
        }
        switch_view.addSubview(&toggle);
        container.addSubview(&switch_view);
        let control = ToggleControl {
            tag,
            button: toggle,
            container: switch_view,
            knob,
            switch_width,
            switch_height,
            knob_size: 16.0,
        };
        style_toggle_control(&control, value);
        self.ivars().toggle_controls.borrow_mut().push(control);
    }

    fn update_weather_summary(&self) {
        let settings = *self.ivars().settings.borrow();
        let language = self.ivars().language.get();
        if let Some(label) = self.ivars().weather_info.borrow().as_ref() {
            label.setStringValue(&NSString::from_str(&weather_summary(&settings, language)));
        }
        if let Some(badge) = self.ivars().weather_badge.borrow().as_ref() {
            badge.setHidden(settings.weather_mode <= 2);
        }
    }

    fn sync_wind_controls(&self) {
        let settings = *self.ivars().settings.borrow();
        let selected = if settings.wind_strength <= 0.0 {
            WIND_CALM_TAG
        } else {
            direction_tag(settings.wind_direction)
        };
        self.select_button_group(40, WIND_CALM_TAG, selected);
        self.update_wind_status();
    }

    fn refresh_overlays(&self) {
        let mtm = self.mtm();
        self.ivars().overlay_drag.set(None);
        for window in self.ivars().windows.borrow().iter() {
            window.orderOut(None);
        }
        self.ivars().views.borrow_mut().clear();
        self.ivars().renderers.borrow_mut().clear();
        self.ivars().screen_frames.borrow_mut().clear();
        self.ivars().last_window_refresh.set(None);
        let delegate = objc2::runtime::ProtocolObject::from_ref(self);
        let overlays = overlay::create_for_connected_displays(mtm, delegate);
        let enabled = self.ivars().state.get().is_enabled();
        for window in &overlays.windows {
            if enabled {
                window.orderFrontRegardless();
            }
        }
        *self.ivars().windows.borrow_mut() = overlays.windows;
        *self.ivars().views.borrow_mut() = overlays.views;
        *self.ivars().renderers.borrow_mut() = overlays.renderers;
        *self.ivars().screen_frames.borrow_mut() = overlays.screen_frames;
        for (screen_index, view) in self.ivars().views.borrow().iter().enumerate() {
            // Click-through by default; the draw loop enables this view only while
            // the pointer is over a snowball (or a snowball is being dragged).
            let interaction = SnowballInteractionView::new(mtm, view.bounds(), self, screen_index);
            interaction.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            view.addSubview(&interaction);
        }
        self.apply_settings();
    }

    fn refresh_window_surfaces(&self) {
        let now = Instant::now();
        let last_refresh = self.ivars().last_window_refresh.get();
        if last_refresh.is_some_and(|last| now.duration_since(last) < Duration::from_millis(16)) {
            return;
        }
        self.ivars().last_window_refresh.set(Some(now));

        // Core Graphics window bounds are measured from the top of the primary display
        // (the first entry of NSScreen.screens), not from NSScreen.mainScreen, which
        // follows keyboard focus and breaks the conversion on multi-display setups.
        let screens = NSScreen::screens(self.mtm());
        if screens.count() == 0 {
            eprintln!("Cannot refresh window surfaces: AppKit reported no screens");
            return;
        }
        let primary_frame = screens.objectAtIndex(0).frame();
        let appkit_top = primary_frame.origin.y + primary_frame.size.height;
        let display_area = (0..screens.count())
            .map(|index| {
                let size = screens.objectAtIndex(index).frame().size;
                size.width * size.height
            })
            .fold(0.0_f64, f64::max);
        let snapshot = match crate::windows::visible_windows(appkit_top, display_area) {
            Ok(windows) => windows,
            Err(error) => {
                eprintln!("Cannot refresh window surfaces: {error}");
                return;
            }
        };
        if last_refresh.is_none() {
            eprintln!(
                "Window discovery found {} visible window-edge segment(s)",
                snapshot.top_edges.len()
            );
        }

        let frames = self.ivars().screen_frames.borrow();
        let mut renderers = self.ivars().renderers.borrow_mut();
        for (screen_index, (frame, renderer)) in frames.iter().zip(renderers.iter_mut()).enumerate()
        {
            let simulation_space = DisplaySimulationSpace::new(*frame);
            let screen_bottom = simulation_space.bottom;
            let visible_frame = screens.objectAtIndex(screen_index).visibleFrame();
            let settings = *self.ivars().settings.borrow();
            let fullscreen = snapshot
                .blocked_regions
                .iter()
                .any(|window| is_fullscreen_window(window, *frame));
            let mut surfaces = dock_surfaces_for_display(
                *frame,
                visible_frame,
                &snapshot.dock_windows,
                fullscreen,
            );
            if settings.window_snow || snapshot.top_edges.iter().any(|window| window.is_launchpad) {
                surfaces.extend(
                    snapshot
                        .top_edges
                        .iter()
                        .filter(|window| {
                            window.is_launchpad || (settings.window_snow && !window.is_dock)
                        })
                        .filter_map(|window| {
                            let window_top = window.y + window.height;
                            let left = window.x.max(simulation_space.left);
                            let right = (window.x + window.width)
                                .min(simulation_space.left + simulation_space.width);
                            if right <= left
                                || window_top < screen_bottom
                                || window_top > simulation_space.top()
                            {
                                return None;
                            }
                            let edge_width = right - left;
                            let margin_points = window_snow_edge_margin_points(edge_width);
                            let geometry = simulation_space.normalize_rect(
                                left,
                                window.y.max(screen_bottom),
                                right,
                                window_top,
                            )?;
                            Some(CollisionSurface {
                                id: window.id,
                                x: geometry.x,
                                y: geometry.y,
                                width: geometry.width,
                                edge_margin: (margin_points / simulation_space.width) as f32,
                                orientation: winter_physics::SurfaceOrientation::Horizontal,
                                persistent: false,
                                maximum_depth: simulation_space.height(28.0),
                            })
                        }),
                );
            }
            let blocked_regions: Vec<_> = snapshot
                .blocked_regions
                .iter()
                .filter_map(|window| {
                    let left = window.x.max(simulation_space.left);
                    let right = (window.x + window.width)
                        .min(simulation_space.left + simulation_space.width);
                    let bottom = window.y.max(screen_bottom);
                    let top = (window.y + window.height).min(simulation_space.top());
                    if right <= left || top <= bottom {
                        return None;
                    }
                    let geometry = simulation_space.normalize_rect(left, bottom, right, top)?;
                    Some(BlockedRegion {
                        id: window.id,
                        x: geometry.x,
                        y: geometry.y,
                        width: geometry.width,
                        height: geometry.height,
                    })
                })
                .collect();
            renderer.set_surfaces(&surfaces);
            renderer.set_fullscreen(fullscreen);
            renderer.set_settings(settings);
            if let Err(error) = renderer.set_blocked_regions(&blocked_regions) {
                eprintln!("Cannot update window mask: {error}");
            }
        }
    }

    fn snowball_pointer_position(&self, screen_index: usize) -> Option<(f32, f32)> {
        let frames = self.ivars().screen_frames.borrow();
        let frame = *frames.get(screen_index)?;
        let space = DisplaySimulationSpace::new(frame);
        let mouse = NSEvent::mouseLocation();
        (mouse.x >= space.left
            && mouse.x <= space.left + space.width
            && mouse.y >= space.bottom
            && mouse.y <= space.top())
        .then(|| (space.x(mouse.x), space.y_from_top(mouse.y)))
    }

    fn snowball_pointer_position_clamped(&self, screen_index: usize) -> Option<(f32, f32)> {
        let frames = self.ivars().screen_frames.borrow();
        let frame = *frames.get(screen_index)?;
        let space = DisplaySimulationSpace::new(frame);
        let mouse = NSEvent::mouseLocation();
        Some((
            ((mouse.x - space.left) / space.width).clamp(0.0, 1.0) as f32,
            ((space.top() - mouse.y) / space.height).clamp(0.0, 1.0) as f32,
        ))
    }

    fn set_overlay_interactive(&self, screen_index: usize, interactive: bool) {
        if let Some(window) = self.ivars().windows.borrow().get(screen_index) {
            window.setIgnoresMouseEvents(!interactive);
        }
    }

    fn overlay_mouse_down(&self, screen_index: usize) {
        let Some((x, y)) = self.snowball_pointer_position(screen_index) else {
            return;
        };
        let target = {
            let mut renderers = self.ivars().renderers.borrow_mut();
            renderers.get_mut(screen_index).and_then(|renderer| {
                if let Some(id) = renderer.grab_snowball(x, y) {
                    Some(DragTarget::Snowball(id))
                } else {
                    renderer.grab_snowman(x, y).map(DragTarget::Snowman)
                }
            })
        };
        if let Some(target) = target {
            self.ivars().overlay_drag.set(Some(OverlayDrag {
                screen_index,
                target,
                last_time: Instant::now(),
            }));
            self.set_overlay_interactive(screen_index, true);
        } else {
            self.set_overlay_interactive(screen_index, false);
        }
    }

    fn overlay_mouse_dragged(&self, screen_index: usize) {
        let Some(mut drag) = self.ivars().overlay_drag.get() else {
            return;
        };
        if drag.screen_index != screen_index {
            return;
        }
        let Some((x, y)) = self.snowball_pointer_position_clamped(screen_index) else {
            return;
        };
        let now = Instant::now();
        let delta_seconds = now
            .duration_since(drag.last_time)
            .as_secs_f32()
            .clamp(1.0 / 240.0, 0.05);
        drag.last_time = now;
        {
            let mut renderers = self.ivars().renderers.borrow_mut();
            if let Some(renderer) = renderers.get_mut(screen_index) {
                match drag.target {
                    DragTarget::Snowball(id) => {
                        renderer.drag_snowball(id, x, y, delta_seconds);
                    }
                    DragTarget::Snowman(id) => {
                        renderer.drag_snowman(id, x, y, delta_seconds);
                    }
                }
            }
        }
        self.ivars().overlay_drag.set(Some(drag));
    }

    fn overlay_mouse_up(&self, screen_index: usize) {
        let Some(drag) = self.ivars().overlay_drag.take() else {
            return;
        };
        if drag.screen_index != screen_index {
            self.ivars().overlay_drag.set(Some(drag));
            return;
        }
        {
            let mut renderers = self.ivars().renderers.borrow_mut();
            if let Some(renderer) = renderers.get_mut(screen_index) {
                match drag.target {
                    DragTarget::Snowball(id) => {
                        renderer.release_snowball(id);
                    }
                    DragTarget::Snowman(id) => {
                        renderer.release_snowman(id);
                    }
                }
            }
        }
        self.set_overlay_interactive(screen_index, false);
    }

    fn save_settings(&self) {
        let settings = *self.ivars().settings.borrow();
        if let Err(error) = crate::settings::save(settings) {
            eprintln!("Cannot save settings: {error}");
        }
    }

    fn select_button_group(&self, first: isize, last: isize, selected: isize) {
        for button in self.ivars().settings_buttons.borrow().iter() {
            if (first..=last).contains(&button.tag()) {
                let is_selected = button.tag() == selected;
                button.setState(if is_selected {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                style_settings_button(button, is_selected);
            }
        }
        self.style_cards();
    }

    fn clear_accumulated_snow(&self) {
        for renderer in self.ivars().renderers.borrow_mut().iter_mut() {
            renderer.clear_snow();
        }
    }

    fn update_wind_status(&self) {
        if let Some(label) = self.ivars().wind_status.get() {
            let settings = *self.ivars().settings.borrow();
            label.setStringValue(&objc2_foundation::NSString::from_str(wind_description(
                settings.wind_strength,
                self.ivars().language.get(),
            )));
        }
    }

    fn apply_settings(&self) {
        let settings = *self.ivars().settings.borrow();
        for window in self.ivars().windows.borrow().iter() {
            if settings.enabled {
                window.orderFrontRegardless();
            } else {
                window.orderOut(None);
            }
        }
        for view in self.ivars().views.borrow().iter() {
            view.setPaused(!settings.enabled);
        }
        if let Some(button) = self
            .ivars()
            .status_item
            .get()
            .and_then(|item| item.button(self.mtm()))
        {
            button.setAlphaValue(if settings.enabled { 1.0 } else { 0.45 });
        }
        let current = self.ivars().state.get();
        if current.is_enabled() != settings.enabled {
            let mut updated = current;
            updated.toggle();
            self.ivars().state.set(updated);
        }
        for renderer in self.ivars().renderers.borrow_mut().iter_mut() {
            renderer.set_settings(settings);
            if !settings.enabled {
                renderer.cancel_overlay_drag();
            }
        }
        if !settings.enabled {
            self.ivars().overlay_drag.set(None);
        }
        self.ivars().last_window_refresh.set(None);
        if settings.enabled {
            self.refresh_window_surfaces();
        }
        self.update_weather_summary();
    }
}

struct SnowballInteractionIvars {
    target: Cell<*const AppDelegate>,
    screen_index: Cell<usize>,
}

impl Default for SnowballInteractionIvars {
    fn default() -> Self {
        Self {
            target: Cell::new(std::ptr::null()),
            screen_index: Cell::new(0),
        }
    }
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = SnowballInteractionIvars]
    struct SnowballInteractionView;

    unsafe impl NSObjectProtocol for SnowballInteractionView {}

    impl SnowballInteractionView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            let target = self.ivars().target.get();
            if !target.is_null() {
                unsafe { &*target }.overlay_mouse_down(self.ivars().screen_index.get());
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {
            let target = self.ivars().target.get();
            if !target.is_null() {
                unsafe { &*target }.overlay_mouse_dragged(self.ivars().screen_index.get());
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            let target = self.ivars().target.get();
            if !target.is_null() {
                unsafe { &*target }.overlay_mouse_up(self.ivars().screen_index.get());
            }
        }
    }
);

impl SnowballInteractionView {
    fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        target: &AppDelegate,
        screen_index: usize,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SnowballInteractionIvars {
            target: Cell::new(target as *const AppDelegate),
            screen_index: Cell::new(screen_index),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

fn apply_weather_preset(settings: &mut AppSettings, mode: u8) {
    settings.weather_mode = mode;
    match mode {
        0 => {
            // Light: near-minimum particles, no wind, no new snow.
            settings.intensity = 0.35;
            settings.falling_speed = 0.55;
            settings.wind_strength = 0.0;
            settings.wind_turbulence = 0.0;
            settings.wind_direction = 0.0;
            settings.accumulation_speed = 0.0;
            settings.particle_count = 800;
            settings.particles_in_windows = false;
        }
        1 => {
            // Calm: half accumulation, a few more particles and a weak
            // rightward wind.
            settings.intensity = 0.8;
            settings.falling_speed = 1.0;
            settings.wind_strength = 0.012;
            settings.wind_turbulence = 0.008;
            settings.wind_direction = 0.0;
            settings.accumulation_speed = 0.5;
            settings.particle_count = 1_500;
            settings.particles_in_windows = false;
        }
        _ => {
            // Blizzard: maximum particles, full accumulation, snow inside
            // windows and a strong alternating wind.
            settings.intensity = 1.0;
            settings.falling_speed = 1.8;
            settings.wind_strength = 0.08;
            settings.wind_turbulence = 0.03;
            settings.wind_direction = 0.0;
            settings.accumulation_speed = 1.0;
            settings.particle_count = 20_000;
            settings.particles_in_windows = true;
        }
    }
}

fn panel_frame(x: f64, top: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, top), NSSize::new(width, height))
}

/// Converts a top-down offset inside a card of `container_height` into the
/// bottom-left coordinates AppKit expects for its subviews.
fn top_frame(container_height: f64, x: f64, top: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(x, container_height - top - height),
        NSSize::new(width, height),
    )
}

fn add_section_title(
    mtm: MainThreadMarker,
    content: &NSView,
    title: &str,
    top: f64,
) -> Retained<NSTextField> {
    let accent = NSView::initWithFrame(
        NSView::alloc(mtm),
        panel_frame(PANEL_PADDING, top + 2.0, 3.0, SECTION_TITLE_HEIGHT - 4.0),
    );
    style_layer(&accent, color_primary(1.0), 1.5, 0.0, color_clear());
    content.addSubview(&accent);

    let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    label.setFrame(panel_frame(
        PANEL_PADDING + 10.0,
        top,
        190.0,
        SECTION_TITLE_HEIGHT,
    ));
    style_label(&label, color_accent(0.95));
    label.setFont(Some(&NSFont::boldSystemFontOfSize(10.5)));
    content.addSubview(&label);
    label
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Language {
    Russian,
    #[default]
    English,
}

fn language_from_code(code: &str) -> Language {
    if code.to_ascii_lowercase().starts_with("ru") {
        Language::Russian
    } else {
        Language::English
    }
}

fn detect_language() -> Language {
    NSLocale::preferredLanguages()
        .firstObject()
        .map(|language| language_from_code(&language.to_string()))
        .unwrap_or_default()
}

fn tr(language: Language, russian: &'static str, english: &'static str) -> &'static str {
    match language {
        Language::Russian => russian,
        Language::English => english,
    }
}

fn tool_hint_text(tool: u8, language: Language) -> &'static str {
    match tool {
        30 => tr(
            language,
            "Варежка: ⌥ + движение — скатать шарик · 3 шарика = снеговик",
            "Mitten: ⌥ + move — pack a snowball · 3 balls = snowman",
        ),
        _ => tr(
            language,
            "Лопата убирает снег под курсором",
            "Shovel clears snow under the cursor",
        ),
    }
}

fn toggle_tooltip(tag: isize, language: Language) -> &'static str {
    match tag {
        60 => tr(
            language,
            "Снежинки могут залетать внутрь окон",
            "Snowflakes can drift inside windows",
        ),
        61 => tr(
            language,
            "Редкие слабые порывы у краёв окон",
            "Occasional weak drafts at window edges",
        ),
        62 => tr(
            language,
            "Снег скапливается на верхних кромках окон",
            "Snow collects on window tops",
        ),
        63 => tr(
            language,
            "Новый снег скапливается на поверхностях",
            "New snow builds up on surfaces",
        ),
        _ => "",
    }
}

#[derive(Clone, Copy)]
enum SymbolWeight {
    Medium,
    Semibold,
}

fn weather_tooltip(tag: isize, language: Language) -> &'static str {
    match tag {
        20 => tr(
            language,
            "Редкий спокойный снегопад",
            "Light, calm snowfall",
        ),
        21 => tr(
            language,
            "Спокойный снегопад с лёгким ветром",
            "Calm snowfall with a light breeze",
        ),
        _ => tr(
            language,
            "Быстрый снег и сильный ветер",
            "Fast snow and strong wind",
        ),
    }
}

fn tool_tooltip(tag: isize, language: Language) -> &'static str {
    match tag {
        30 => tr(
            language,
            "Двигает сугробы; ⌥ + движение скатывает шарик",
            "Moves drifts; ⌥ + move packs a snowball",
        ),
        _ => tr(
            language,
            "Убирает снег под курсором",
            "Clears snow under the cursor",
        ),
    }
}

fn slider_tooltip(tag: isize, language: Language) -> &'static str {
    match tag {
        90 => tr(
            language,
            "Как быстро растут сугробы",
            "How quickly drifts grow",
        ),
        91 => tr(
            language,
            "Сколько снежинок участвует в симуляции",
            "How many snowflakes are simulated",
        ),
        92 => tr(
            language,
            "Насколько густым выглядит снегопад",
            "How dense the snowfall looks",
        ),
        93 => tr(
            language,
            "Скорость падения снежинок",
            "Snowflake fall speed",
        ),
        94 => tr(language, "Сила бокового ветра", "Sideways wind strength"),
        95 => tr(
            language,
            "Насколько неспокойным выглядит ветер",
            "How gusty the wind looks",
        ),
        _ => "",
    }
}

fn settings_expanded_fits(mtm: MainThreadMarker) -> bool {
    NSScreen::mainScreen(mtm)
        .is_some_and(|screen| screen.visibleFrame().size.height >= PANEL_HEIGHT_EXPANDED + 8.0)
}

fn launch_at_login_enabled() -> bool {
    unsafe { SMAppService::mainAppService().status() == SMAppServiceStatus::Enabled }
}

const ONBOARDING_SEEN_KEY: &str = "FrostyOnboardingSeenV1";
const ROLL_HINT_SEEN_KEY: &str = "FrostyRollHintSeenV1";

fn roll_hint_seen() -> bool {
    NSUserDefaults::standardUserDefaults().boolForKey(&NSString::from_str(ROLL_HINT_SEEN_KEY))
}

fn mark_roll_hint_seen() {
    NSUserDefaults::standardUserDefaults()
        .setBool_forKey(true, &NSString::from_str(ROLL_HINT_SEEN_KEY));
}

fn onboarding_seen() -> bool {
    NSUserDefaults::standardUserDefaults().boolForKey(&NSString::from_str(ONBOARDING_SEEN_KEY))
}

fn mark_onboarding_seen() {
    NSUserDefaults::standardUserDefaults()
        .setBool_forKey(true, &NSString::from_str(ONBOARDING_SEEN_KEY));
}

const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon.png");

fn app_icon_image() -> Option<Retained<NSImage>> {
    let data =
        unsafe { NSData::dataWithBytes_length(APP_ICON_PNG.as_ptr().cast(), APP_ICON_PNG.len()) };
    NSImage::initWithData(NSImage::alloc(), &data)
}

fn symbol_image(name: &str, point_size: f64, weight: SymbolWeight) -> Option<Retained<NSImage>> {
    let weight = unsafe {
        match weight {
            SymbolWeight::Medium => objc2_app_kit::NSFontWeightMedium,
            SymbolWeight::Semibold => objc2_app_kit::NSFontWeightSemibold,
        }
    };
    let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let configuration =
        NSImageSymbolConfiguration::configurationWithPointSize_weight(point_size, weight);
    let image = symbol
        .imageWithSymbolConfiguration(&configuration)
        .unwrap_or(symbol);
    image.setTemplate(true);
    Some(image)
}

fn window_snow_edge_margin_points(edge_width_points: f64) -> f64 {
    (edge_width_points * WINDOW_SNOW_MARGIN_RATIO)
        .clamp(MIN_WINDOW_SNOW_MARGIN_POINTS, MAX_WINDOW_SNOW_MARGIN_POINTS)
        .min(edge_width_points * 0.24)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DockEdge {
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Copy)]
// AppKit/Core Graphics geometry enters in screen points; simulation geometry
// leaves this boundary normalized to a top-left origin and a 0..1 display.
struct DisplaySimulationSpace {
    left: f64,
    bottom: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, Copy)]
struct NormalizedRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl DisplaySimulationSpace {
    fn new(frame: NSRect) -> Self {
        Self {
            left: frame.origin.x,
            bottom: frame.origin.y,
            width: frame.size.width,
            height: frame.size.height,
        }
    }

    fn top(self) -> f64 {
        self.bottom + self.height
    }

    fn x(self, screen_x: f64) -> f32 {
        ((screen_x - self.left) / self.width) as f32
    }

    fn y_from_top(self, screen_y: f64) -> f32 {
        ((self.top() - screen_y) / self.height) as f32
    }

    fn width(self, points: f64) -> f32 {
        (points / self.width) as f32
    }

    fn height(self, points: f64) -> f32 {
        (points / self.height) as f32
    }

    fn normalize_rect(
        self,
        left: f64,
        bottom: f64,
        right: f64,
        top: f64,
    ) -> Option<NormalizedRect> {
        if !self.width.is_finite()
            || !self.height.is_finite()
            || self.width <= 0.0
            || self.height <= 0.0
            || right <= left
            || top <= bottom
        {
            return None;
        }
        Some(NormalizedRect {
            x: self.x(left),
            y: self.y_from_top(top),
            width: self.width(right - left),
            height: self.height(top - bottom),
        })
    }
}

fn is_fullscreen_window(window: &VisibleWindow, frame: NSRect) -> bool {
    window.x <= frame.origin.x + 8.0
        && window.y <= frame.origin.y + 8.0
        && window.x + window.width >= frame.origin.x + frame.size.width - 8.0
        && window.y + window.height >= frame.origin.y + frame.size.height - 8.0
}

fn dock_surfaces_for_display(
    frame: NSRect,
    visible_frame: NSRect,
    dock_windows: &[VisibleWindow],
    fullscreen: bool,
) -> Vec<CollisionSurface> {
    use winter_physics::SurfaceOrientation;

    if fullscreen || frame.size.width <= 0.0 || frame.size.height <= 0.0 {
        return Vec::new();
    }

    let space = DisplaySimulationSpace::new(frame);
    let screen_left = space.left;
    let screen_bottom = space.bottom;
    let screen_right = space.left + space.width;
    let screen_top = space.top();
    let mut dock_bounds: Option<(f64, f64, f64, f64)> = None;
    for dock in dock_windows {
        let left = dock.x.max(screen_left);
        let right = (dock.x + dock.width).min(screen_right);
        let bottom = dock.y.max(screen_bottom);
        let top = (dock.y + dock.height).min(screen_top);
        if right <= left || top <= bottom {
            continue;
        }
        dock_bounds = Some(match dock_bounds {
            Some((old_left, old_bottom, old_right, old_top)) => (
                old_left.min(left),
                old_bottom.min(bottom),
                old_right.max(right),
                old_top.max(top),
            ),
            None => (left, bottom, right, top),
        });
    }

    let bottom_inset = (visible_frame.origin.y - screen_bottom).max(0.0);
    let left_inset = (visible_frame.origin.x - screen_left).max(0.0);
    let right_inset = (screen_right - visible_frame.origin.x - visible_frame.size.width).max(0.0);
    let edge = if let Some((left, bottom, right, _)) = dock_bounds {
        let gaps = [
            ((bottom - screen_bottom).abs(), DockEdge::Bottom),
            ((left - screen_left).abs(), DockEdge::Left),
            ((screen_right - right).abs(), DockEdge::Right),
        ];
        gaps.into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, edge)| edge)
            .unwrap_or(DockEdge::Bottom)
    } else if left_inset >= right_inset && left_inset >= bottom_inset && left_inset > 20.0 {
        DockEdge::Left
    } else if right_inset > bottom_inset && right_inset > 20.0 {
        DockEdge::Right
    } else {
        DockEdge::Bottom
    };

    let mut surfaces = Vec::new();
    match edge {
        DockEdge::Bottom => {
            let bounds = dock_bounds.or_else(|| {
                (bottom_inset > 20.0).then(|| {
                    let width = crate::windows::dock_preferred_length()
                        .map(|width| width.min(visible_frame.size.width))
                        .unwrap_or_else(|| (frame.size.width * 0.64).min(visible_frame.size.width));
                    let left = (screen_left + (frame.size.width - width) / 2.0)
                        .max(visible_frame.origin.x);
                    (
                        left,
                        visible_frame.origin.y,
                        left + width,
                        visible_frame.origin.y,
                    )
                })
            });
            if let Some((left, bottom, right, top)) = bounds {
                let surface_y = if dock_bounds.is_some() { top } else { bottom };
                let margin_points = window_snow_edge_margin_points(right - left);
                surfaces.push(CollisionSurface {
                    id: DOCK_TOP_SURFACE_ID,
                    x: space.x(left),
                    y: space.y_from_top(surface_y),
                    width: space.width(right - left),
                    edge_margin: space.width(margin_points),
                    orientation: SurfaceOrientation::Horizontal,
                    persistent: true,
                    maximum_depth: space.height(DOCK_SNOW_DEPTH_POINTS),
                });
                for (id, segment_left, segment_right) in [
                    (DOCK_LEFT_SURFACE_ID, screen_left, left),
                    (DOCK_RIGHT_SURFACE_ID, right, screen_right),
                ] {
                    if segment_right > segment_left {
                        surfaces.push(CollisionSurface {
                            id,
                            x: space.x(segment_left),
                            y: 1.0,
                            width: space.width(segment_right - segment_left),
                            edge_margin: space.width(window_snow_edge_margin_points(
                                segment_right - segment_left,
                            )),
                            orientation: SurfaceOrientation::Horizontal,
                            persistent: true,
                            maximum_depth: space.height(DOCK_SNOW_DEPTH_POINTS),
                        });
                    }
                }
            } else {
                surfaces.push(CollisionSurface {
                    id: DOCK_TOP_SURFACE_ID,
                    x: 0.0,
                    y: 1.0,
                    width: 1.0,
                    edge_margin: space.width(window_snow_edge_margin_points(frame.size.width)),
                    orientation: SurfaceOrientation::Horizontal,
                    persistent: false,
                    maximum_depth: space.height(DOCK_SNOW_DEPTH_POINTS),
                });
            }
        }
        DockEdge::Left | DockEdge::Right => {
            let bounds = dock_bounds.unwrap_or_else(|| {
                let thickness = if edge == DockEdge::Left {
                    left_inset.max(64.0)
                } else {
                    right_inset.max(64.0)
                };
                let length = crate::windows::dock_preferred_length()
                    .map(|length| length.min(visible_frame.size.height))
                    .unwrap_or(visible_frame.size.height);
                let y = visible_frame.origin.y + (visible_frame.size.height - length) / 2.0;
                if edge == DockEdge::Left {
                    (
                        screen_left,
                        y,
                        (screen_left + thickness).min(screen_right),
                        y + length,
                    )
                } else {
                    (
                        (screen_right - thickness).max(screen_left),
                        y,
                        screen_right,
                        y + length,
                    )
                }
            });
            let (left, bottom, right, top) = bounds;
            let face_x = if edge == DockEdge::Left { right } else { left };
            let orientation = if edge == DockEdge::Left {
                SurfaceOrientation::VerticalToRight
            } else {
                SurfaceOrientation::VerticalToLeft
            };
            surfaces.push(CollisionSurface {
                id: DOCK_TOP_SURFACE_ID,
                x: space.x(face_x),
                y: space.y_from_top(top),
                width: space.height(top - bottom),
                edge_margin: space.height(window_snow_edge_margin_points(top - bottom)),
                orientation,
                persistent: true,
                maximum_depth: space.width(DOCK_SNOW_DEPTH_POINTS),
            });
        }
    }
    surfaces
}

fn fill_polygon(points: &[(f64, f64)], color: Retained<NSColor>) {
    if points.len() < 3 {
        return;
    }
    let path = NSBezierPath::bezierPath();
    path.moveToPoint(NSPoint::new(points[0].0, points[0].1));
    for point in &points[1..] {
        path.lineToPoint(NSPoint::new(point.0, point.1));
    }
    path.closePath();
    color.setFill();
    path.fill();
}

fn stroke_polyline(points: &[(f64, f64)], color: Retained<NSColor>, width: f64) {
    if points.len() < 2 {
        return;
    }
    let path = NSBezierPath::bezierPath();
    path.moveToPoint(NSPoint::new(points[0].0, points[0].1));
    for point in &points[1..] {
        path.lineToPoint(NSPoint::new(point.0, point.1));
    }
    path.setLineWidth(width);
    color.setStroke();
    path.stroke();
}

fn fill_oval(rect: NSRect, color: Retained<NSColor>) {
    let path = NSBezierPath::bezierPathWithOvalInRect(rect);
    color.setFill();
    path.fill();
}

fn snow_band_path(width: f64, height: f64, top: f64, amplitude: f64) -> Retained<NSBezierPath> {
    let path = NSBezierPath::bezierPath();
    path.moveToPoint(NSPoint::new(0.0, height));
    path.lineToPoint(NSPoint::new(0.0, height * top));
    path.curveToPoint_controlPoint1_controlPoint2(
        NSPoint::new(width * 0.52, height * (top - 0.008)),
        NSPoint::new(width * 0.18, height * (top - amplitude)),
        NSPoint::new(width * 0.34, height * (top + amplitude * 0.55)),
    );
    path.curveToPoint_controlPoint1_controlPoint2(
        NSPoint::new(width, height * top),
        NSPoint::new(width * 0.68, height * (top - amplitude * 0.6)),
        NSPoint::new(width * 0.84, height * (top + amplitude * 0.4)),
    );
    path.lineToPoint(NSPoint::new(width, height));
    path.closePath();
    path
}

fn draw_snow_scene(bounds: NSRect) {
    let width = bounds.size.width;
    let height = bounds.size.height;
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    // Deep blue night sky with scattered stars.
    let star = color_text(0.35);
    for (x, y, radius) in [
        (0.06, 0.08, 1.1),
        (0.11, 0.15, 0.8),
        (0.17, 0.06, 1.3),
        (0.22, 0.19, 0.9),
        (0.28, 0.10, 1.1),
        (0.34, 0.05, 0.8),
        (0.39, 0.16, 1.2),
        (0.45, 0.08, 0.9),
        (0.51, 0.18, 1.0),
        (0.57, 0.06, 1.2),
        (0.62, 0.14, 0.8),
        (0.68, 0.09, 1.1),
        (0.73, 0.20, 0.9),
        (0.79, 0.07, 1.0),
        (0.91, 0.11, 1.2),
        (0.96, 0.20, 0.8),
    ] {
        fill_oval(
            NSRect::new(
                NSPoint::new(width * x, height * y),
                NSSize::new(radius, radius),
            ),
            star.clone(),
        );
    }

    // Constellations: thin connecting lines with a brighter star at each node.
    let constellation_line = color_text(0.16);
    let constellation_star = color_text(0.45);
    for constellation in [
        &[(0.06, 0.13), (0.12, 0.09), (0.18, 0.16), (0.15, 0.24)][..],
        &[(0.27, 0.07), (0.33, 0.13), (0.40, 0.10), (0.46, 0.17)][..],
        &[(0.57, 0.12), (0.62, 0.07), (0.68, 0.13), (0.72, 0.21)][..],
        &[(0.76, 0.27), (0.82, 0.32), (0.88, 0.27)][..],
    ] {
        let path = NSBezierPath::bezierPath();
        let Some((first, rest)) = constellation.split_first() else {
            continue;
        };
        path.moveToPoint(NSPoint::new(width * first.0, height * first.1));
        for point in rest {
            path.lineToPoint(NSPoint::new(width * point.0, height * point.1));
        }
        path.setLineWidth(0.7);
        constellation_line.setStroke();
        path.stroke();
        for point in constellation {
            fill_oval(
                NSRect::new(
                    NSPoint::new(width * point.0 - 1.0, height * point.1 - 1.0),
                    NSSize::new(2.0, 2.0),
                ),
                constellation_star.clone(),
            );
        }
    }

    // Far mountain range, peaks reaching roughly the middle of the panel.
    fill_polygon(
        &[
            (0.0, height),
            (0.0, height * 0.70),
            (width * 0.08, height * 0.58),
            (width * 0.17, height * 0.66),
            (width * 0.27, height * 0.52),
            (width * 0.38, height * 0.63),
            (width * 0.49, height * 0.55),
            (width * 0.60, height * 0.67),
            (width * 0.72, height * 0.56),
            (width * 0.84, height * 0.65),
            (width, height * 0.58),
            (width, height),
        ],
        color_surface(0.20),
    );
    for (x, y, half, drop) in [
        (0.08, 0.58, 0.050, 0.045),
        (0.27, 0.52, 0.055, 0.052),
        (0.49, 0.55, 0.048, 0.045),
        (0.72, 0.56, 0.058, 0.050),
        (0.84, 0.65, 0.045, 0.040),
    ] {
        fill_polygon(
            &[
                (width * x, height * y),
                (width * (x + half), height * (y + drop)),
                (width * (x + half * 0.25), height * (y + drop * 0.5)),
                (width * (x - half * 0.2), height * (y + drop * 0.45)),
                (width * (x - half), height * (y + drop)),
            ],
            color_text(0.08),
        );
    }
    stroke_polyline(
        &[
            (0.0, height * 0.70),
            (width * 0.08, height * 0.58),
            (width * 0.17, height * 0.66),
            (width * 0.27, height * 0.52),
            (width * 0.38, height * 0.63),
            (width * 0.49, height * 0.55),
            (width * 0.60, height * 0.67),
            (width * 0.72, height * 0.56),
            (width * 0.84, height * 0.65),
            (width, height * 0.58),
        ],
        color_text(0.14),
        1.2,
    );

    // Near mountain range.
    fill_polygon(
        &[
            (0.0, height),
            (0.0, height * 0.82),
            (width * 0.12, height * 0.73),
            (width * 0.24, height * 0.80),
            (width * 0.36, height * 0.69),
            (width * 0.50, height * 0.78),
            (width * 0.64, height * 0.71),
            (width * 0.78, height * 0.80),
            (width, height * 0.74),
            (width, height),
        ],
        color_surface(0.28),
    );
    for (x, y, half, drop) in [
        (0.12, 0.73, 0.040, 0.035),
        (0.36, 0.69, 0.050, 0.040),
        (0.64, 0.71, 0.045, 0.038),
        (0.78, 0.80, 0.038, 0.030),
    ] {
        fill_polygon(
            &[
                (width * x, height * y),
                (width * (x + half), height * (y + drop)),
                (width * (x + half * 0.25), height * (y + drop * 0.5)),
                (width * (x - half * 0.2), height * (y + drop * 0.45)),
                (width * (x - half), height * (y + drop)),
            ],
            color_text(0.11),
        );
    }
    stroke_polyline(
        &[
            (0.0, height * 0.82),
            (width * 0.12, height * 0.73),
            (width * 0.24, height * 0.80),
            (width * 0.36, height * 0.69),
            (width * 0.50, height * 0.78),
            (width * 0.64, height * 0.71),
            (width * 0.78, height * 0.80),
            (width, height * 0.74),
        ],
        color_text(0.18),
        1.4,
    );

    // Layered snow at the bottom, drawn as softly striped drifts.
    for (top, amplitude, color) in [
        (0.86, 0.018, color_surface(0.22)),
        (0.90, 0.014, color_text(0.05)),
        (0.94, 0.012, color_surface(0.28)),
        (0.975, 0.008, color_text(0.08)),
    ] {
        let band = snow_band_path(width, height, top, amplitude);
        color.setFill();
        band.fill();
        let outline = snow_band_path(width, height, top, amplitude);
        outline.setLineWidth(0.8);
        color_text(0.10).setStroke();
        outline.stroke();
    }

    // A light dark veil keeps the landscape atmospheric behind glass cards.
    let veil = NSBezierPath::bezierPathWithRect(bounds);
    color_dark(0.30).setFill();
    veil.fill();
}
fn palette(hex: u32, alpha: f64) -> Retained<NSColor> {
    let red = ((hex >> 16) & 0xff) as f64 / 255.0;
    let green = ((hex >> 8) & 0xff) as f64 / 255.0;
    let blue = (hex & 0xff) as f64 / 255.0;
    NSColor::colorWithSRGBRed_green_blue_alpha(red, green, blue, alpha.clamp(0.0, 1.0))
}

fn color_dark(alpha: f64) -> Retained<NSColor> {
    palette(HEX_DARK, alpha)
}

fn color_surface(alpha: f64) -> Retained<NSColor> {
    palette(HEX_SURFACE, alpha)
}

fn color_primary(alpha: f64) -> Retained<NSColor> {
    palette(HEX_PRIMARY, alpha)
}

fn color_accent(alpha: f64) -> Retained<NSColor> {
    palette(HEX_ACCENT, alpha)
}

fn color_text(alpha: f64) -> Retained<NSColor> {
    palette(HEX_TEXT, alpha)
}

fn color_clear() -> Retained<NSColor> {
    palette(HEX_DARK, 0.0)
}

fn style_layer(
    view: &NSView,
    background: Retained<NSColor>,
    radius: f64,
    border_width: f64,
    border_color: Retained<NSColor>,
) {
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        let background = background.CGColor();
        layer.setBackgroundColor(Some(&background));
        layer.setCornerRadius(radius);
        layer.setBorderWidth(border_width);
        let border = border_color.CGColor();
        layer.setBorderColor(Some(&border));
    }
}

fn style_ring(view: &NSView, border_color: Retained<NSColor>, border_width: f64, radius: f64) {
    style_layer(view, color_clear(), radius, border_width, border_color);
}

fn style_label(label: &NSTextField, color: Retained<NSColor>) {
    label.setTextColor(Some(&color));
    label.setFont(Some(&NSFont::systemFontOfSize(14.0)));
}

fn style_card(card: &SettingsCard, selected: bool) {
    let (background, border_color, border_width) = if selected {
        if card.is_tool {
            (color_primary(0.38), color_accent(0.92), 1.6)
        } else {
            (color_primary(0.96), color_accent(0.80), 1.2)
        }
    } else {
        (color_surface(0.22), color_surface(0.85), 0.8)
    };
    style_layer(
        &card.container,
        background,
        if card.is_tool { 12.0 } else { 14.0 },
        border_width,
        border_color,
    );
    let title_color = if selected {
        color_text(1.0)
    } else {
        color_text(0.85)
    };
    card.title.setTextColor(Some(&title_color));
    if let Some(subtitle) = &card.subtitle {
        let subtitle_color = if selected {
            color_text(0.88)
        } else {
            color_accent(0.85)
        };
        subtitle.setTextColor(Some(&subtitle_color));
    }
    if card.is_tool {
        // Tool glyphs stand on their own: no circular badge behind them.
        style_layer(&card.icon, color_clear(), 0.0, 0.0, color_clear());
        let icon_tint = if selected {
            color_text(1.0)
        } else {
            color_accent(0.90)
        };
        card.icon.setContentTintColor(Some(&icon_tint));
    } else {
        style_layer(&card.icon, color_clear(), 0.0, 0.0, color_clear());
        let icon_tint = if selected {
            color_text(1.0)
        } else {
            color_accent(0.90)
        };
        card.icon.setContentTintColor(Some(&icon_tint));
    }
}

fn style_small_action(button: &NSButton) {
    button.setFont(Some(&NSFont::systemFontOfSize(8.5)));
    button.setContentTintColor(Some(&color_text(1.0)));
    style_layer(button, color_surface(0.30), 7.0, 1.0, color_accent(0.22));
}

fn style_settings_button(button: &NSButton, selected: bool) {
    let tag = button.tag();
    button.setBordered(false);
    if (40..=47).contains(&tag) {
        style_layer(
            button,
            if selected {
                color_primary(0.75)
            } else {
                color_clear()
            },
            16.0,
            0.0,
            color_clear(),
        );
        button.setContentTintColor(Some(&color_text(1.0)));
        return;
    }
    if tag == WIND_CALM_TAG {
        style_layer(
            button,
            if selected {
                color_primary(0.65)
            } else {
                color_clear()
            },
            14.0,
            if selected { 1.0 } else { 0.0 },
            color_accent(0.75),
        );
        let tint = if selected {
            color_text(1.0)
        } else {
            color_accent(0.85)
        };
        button.setContentTintColor(Some(&tint));
        return;
    }
    if tag == 50 {
        style_layer(button, color_surface(0.28), 10.0, 0.8, color_accent(0.25));
        button.setContentTintColor(Some(&color_text(1.0)));
        button.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        return;
    }
    if matches!(tag, 1..=2) {
        style_layer(button, color_surface(0.35), 10.0, 0.7, color_accent(0.20));
        button.setContentTintColor(Some(&color_text(1.0)));
    }
}

fn style_toggle_control(control: &ToggleControl, enabled: bool) {
    control.button.setState(if enabled {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    style_layer(
        &control.container,
        if enabled {
            color_primary(1.0)
        } else {
            color_surface(0.55)
        },
        control.switch_height / 2.0,
        1.0,
        if enabled {
            color_accent(0.85)
        } else {
            color_surface(0.95)
        },
    );
    let knob = control.knob_size;
    let inset = 3.0;
    let x = if enabled {
        control.switch_width - inset - knob
    } else {
        inset
    };
    control.knob.setFrame(NSRect::new(
        NSPoint::new(x, (control.switch_height - knob) / 2.0),
        NSSize::new(knob, knob),
    ));
    style_layer(
        &control.knob,
        if enabled {
            color_text(1.0)
        } else {
            color_accent(0.75)
        },
        knob / 2.0,
        0.0,
        color_clear(),
    );
}

fn slider_label_value(tag: isize, value: f64) -> String {
    match tag {
        91 => format!("{value:.0}"),
        94 => format!(
            "{:.0}%",
            (value / MAX_WIND_STRENGTH as f64).clamp(0.0, 1.0) * 100.0
        ),
        95 => format!(
            "{:.0}%",
            (value / MAX_WIND_TURBULENCE as f64).clamp(0.0, 1.0) * 100.0
        ),
        _ => format!("{:.0}%", value * 100.0),
    }
}

fn direction_angle(tag: isize) -> f32 {
    match tag {
        40 => -3.0 * std::f32::consts::FRAC_PI_4,
        41 => -std::f32::consts::FRAC_PI_2,
        42 => -std::f32::consts::FRAC_PI_4,
        43 => std::f32::consts::PI,
        44 => 0.0,
        45 => 3.0 * std::f32::consts::FRAC_PI_4,
        46 => std::f32::consts::FRAC_PI_2,
        _ => std::f32::consts::FRAC_PI_4,
    }
}

fn direction_tag(direction: f32) -> isize {
    let normalized = direction.rem_euclid(TAU);
    let directions = [
        (40, 5.0 * std::f32::consts::FRAC_PI_4),
        (41, 3.0 * std::f32::consts::FRAC_PI_2),
        (42, 7.0 * std::f32::consts::FRAC_PI_4),
        (43, std::f32::consts::PI),
        (44, 0.0),
        (45, 3.0 * std::f32::consts::FRAC_PI_4),
        (46, std::f32::consts::FRAC_PI_2),
        (47, std::f32::consts::FRAC_PI_4),
    ];
    directions
        .into_iter()
        .min_by(|(_, first), (_, second)| {
            angle_distance(normalized, *first).total_cmp(&angle_distance(normalized, *second))
        })
        .map_or(48, |(tag, _)| tag)
}

fn angle_distance(first: f32, second: f32) -> f32 {
    let difference = (first - second).abs().rem_euclid(TAU);
    difference.min(TAU - difference)
}

fn preset_name(mode: u8, language: Language) -> &'static str {
    match mode {
        0 => tr(language, "Лёгкий", "Light"),
        1 => tr(language, "Спокойный", "Calm"),
        2 => tr(language, "Метель", "Blizzard"),
        _ => tr(language, "Изменённый пресет", "Custom preset"),
    }
}

fn weather_summary(settings: &AppSettings, language: Language) -> String {
    let name = preset_name(settings.weather_mode, language);
    let particles = settings.particle_count;
    let accumulation = (settings.accumulation_speed * 100.0).round() as i32;
    let wind = (settings.wind_strength / MAX_WIND_STRENGTH * 100.0).round() as i32;
    match language {
        Language::Russian => {
            format!("{name} · {particles} частиц · {accumulation}% накопления · ветер {wind}%")
        }
        Language::English => {
            format!("{name} · {particles} particles · {accumulation}% accumulation · {wind}% wind")
        }
    }
}

fn wind_description(strength: f32, language: Language) -> &'static str {
    match strength {
        value if value < 0.004 => tr(language, "Штиль", "Calm"),
        value if value < 0.03 => tr(language, "Лёгкий ветер", "Light wind"),
        value if value < 0.06 => tr(language, "Ветер", "Wind"),
        _ => tr(language, "Метель", "Blizzard"),
    }
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct FlippedSectionView;

    unsafe impl NSObjectProtocol for FlippedSectionView {}

    impl FlippedSectionView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedSectionView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct PanelBackgroundView;

    unsafe impl NSObjectProtocol for PanelBackgroundView {}

    impl PanelBackgroundView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let bounds = self.bounds();
            let base = [color_dark(1.0), color_dark(1.0)];
            let base = NSArray::from_retained_slice(&base);
            if let Some(gradient) = NSGradient::initWithColors(NSGradient::alloc(), &base) {
                gradient.drawInRect_angle(bounds, 270.0);
            }
            // A soft #3D518C glow from the top, plus a lighter band near the
            // about footer, gives the mostly #091540 panel depth without noise.
            let glow_height = bounds.size.height * 0.55;
            let glow_rect = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(bounds.size.width, glow_height),
            );
            let glow = [color_surface(0.48), color_surface(0.0)];
            let glow = NSArray::from_retained_slice(&glow);
            if let Some(gradient) = NSGradient::initWithColors(NSGradient::alloc(), &glow) {
                gradient.drawInRect_angle(glow_rect, 270.0);
            }
            let bottom_rect = NSRect::new(
                NSPoint::new(0.0, bounds.size.height * 0.76),
                NSSize::new(bounds.size.width, bounds.size.height * 0.24),
            );
            let bottom = [color_surface(0.0), color_surface(0.16)];
            let bottom = NSArray::from_retained_slice(&bottom);
            if let Some(gradient) = NSGradient::initWithColors(NSGradient::alloc(), &bottom) {
                gradient.drawInRect_angle(bottom_rect, 270.0);
            }
            draw_snow_scene(bounds);
        }
    }
);

impl PanelBackgroundView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("Frosty must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)));
    app.run();
}

#[cfg(test)]
mod tests {
    use super::{
        apply_weather_preset, direction_tag, dock_surfaces_for_display, is_fullscreen_window,
        language_from_code, panel_frame, slider_label_value, toggle_tooltip, tool_hint_text,
        top_frame, weather_summary, window_snow_edge_margin_points, DisplaySimulationSpace,
        Language, ABOUT_HEIGHT, ABOUT_TOP_COLLAPSED, ABOUT_TOP_EXPANDED, ADDITIONAL_HEIGHT,
        ADDITIONAL_TOP, CLEAR_HEIGHT, CLEAR_TOP, COMPASS_CARD_HEIGHT, COMPASS_CARD_TOP,
        COMPASS_CENTER_X, COMPASS_RADIUS, COMPASS_STEP_X, COMPASS_STEP_Y, HEADER_HEIGHT,
        HEADER_TOP, PANEL_HEIGHT_COLLAPSED, PANEL_HEIGHT_EXPANDED, PANEL_PADDING, PANEL_WIDTH,
        PHYSICS_SECOND_ROW_TOP, PHYSICS_TOGGLE_TOP, PHYSICS_TOP, SECTION_TITLE_HEIGHT,
        SLIDER_ROW_HEIGHT, TOGGLES_TOP, TOGGLE_HEIGHT, TOOL_HEIGHT, TOOL_HINT_HEIGHT,
        TOOL_HINT_TOP, TOOL_TITLE_TOP, TOOL_TOP, WEATHER_HEIGHT, WEATHER_HINT_HEIGHT,
        WEATHER_HINT_TOP, WEATHER_META_HEIGHT, WEATHER_META_TOP, WEATHER_TITLE_TOP, WEATHER_TOP,
        WIND_CARD_LEFT_WIDTH, WIND_SLIDER_FIRST_TOP, WIND_SLIDER_SECOND_TOP, WIND_SLIDER_WIDTH,
        WIND_SLIDER_X, WIND_TITLE_TOP,
    };
    use crate::settings::AppSettings;
    use crate::windows::VisibleWindow;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use winter_physics::SurfaceOrientation;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }

    fn dock_window(x: f64, y: f64, width: f64, height: f64) -> VisibleWindow {
        VisibleWindow {
            id: 1,
            x,
            y,
            width,
            height,
            is_dock: true,
            is_launchpad: false,
        }
    }

    #[test]
    fn window_snow_edge_margin_scales_and_stays_within_bounds() {
        assert_eq!(window_snow_edge_margin_points(800.0), 20.0);
        assert_eq!(window_snow_edge_margin_points(2_000.0), 24.0);
        assert_eq!(window_snow_edge_margin_points(20.0), 4.8);
    }

    #[test]
    fn dock_surfaces_support_bottom_and_side_placements() {
        let screen = rect(0.0, 0.0, 1_440.0, 900.0);
        let visible = rect(0.0, 80.0, 1_440.0, 820.0);

        let bottom = dock_surfaces_for_display(
            screen,
            visible,
            &[dock_window(200.0, 0.0, 1_040.0, 80.0)],
            false,
        );
        assert_eq!(bottom.len(), 3);
        assert!(bottom.iter().all(|surface| surface.persistent));
        assert!(bottom
            .iter()
            .all(|surface| surface.orientation == SurfaceOrientation::Horizontal));
        assert!(
            bottom.iter().all(|surface| surface.edge_margin > 0.0),
            "dock snow must be inset from the dock's rounded ends"
        );

        let left = dock_surfaces_for_display(
            screen,
            rect(80.0, 0.0, 1_360.0, 900.0),
            &[dock_window(0.0, 20.0, 80.0, 860.0)],
            false,
        );
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].orientation, SurfaceOrientation::VerticalToRight);
        assert!(left[0].persistent);
        assert!(left[0].edge_margin > 0.0);
        assert!((left[0].x - 80.0 / 1_440.0).abs() < f32::EPSILON);

        let right = dock_surfaces_for_display(
            screen,
            rect(0.0, 0.0, 1_360.0, 900.0),
            &[dock_window(1_360.0, 20.0, 80.0, 860.0)],
            false,
        );
        assert_eq!(right.len(), 1);
        assert_eq!(right[0].orientation, SurfaceOrientation::VerticalToLeft);
        assert!(right[0].persistent);
    }

    #[test]
    fn fullscreen_suppresses_dock_surfaces() {
        let surfaces = dock_surfaces_for_display(
            rect(0.0, 0.0, 1_440.0, 900.0),
            rect(0.0, 80.0, 1_440.0, 820.0),
            &[dock_window(200.0, 0.0, 1_040.0, 80.0)],
            true,
        );

        assert!(surfaces.is_empty());
    }

    #[test]
    fn fullscreen_detection_uses_screen_coverage_not_dock_visibility() {
        let screen = rect(0.0, 0.0, 1_440.0, 900.0);
        assert!(is_fullscreen_window(
            &dock_window(0.0, 0.0, 1_440.0, 900.0),
            screen
        ));
        assert!(!is_fullscreen_window(
            &dock_window(20.0, 20.0, 1_400.0, 850.0),
            screen
        ));
    }

    #[test]
    fn screen_points_normalize_to_top_left_coordinates_independent_of_display_scale() {
        let standard_scale = DisplaySimulationSpace::new(rect(0.0, 0.0, 1_000.0, 800.0));
        let retina_scale = DisplaySimulationSpace::new(rect(0.0, 0.0, 2_000.0, 1_600.0));
        let standard = standard_scale
            .normalize_rect(100.0, 200.0, 400.0, 600.0)
            .unwrap();
        let retina = retina_scale
            .normalize_rect(200.0, 400.0, 800.0, 1_200.0)
            .unwrap();

        assert_eq!(standard.x, 0.1);
        assert_eq!(standard.y, 0.25);
        assert_eq!(standard.width, 0.3);
        assert_eq!(standard.height, 0.5);
        assert_eq!(standard.x, retina.x);
        assert_eq!(standard.y, retina.y);
        assert_eq!(standard.width, retina.width);
        assert_eq!(standard.height, retina.height);
    }

    #[test]
    fn physical_sliders_display_their_own_units() {
        assert_eq!(slider_label_value(90, 0.0), "0%");
        assert_eq!(slider_label_value(90, 0.25), "25%");
        assert_eq!(slider_label_value(90, 1.0), "100%");
        assert_eq!(slider_label_value(92, 0.65), "65%");
        assert_eq!(slider_label_value(93, 1.8), "180%");
        assert_eq!(slider_label_value(94, 0.0), "0%");
        assert_eq!(slider_label_value(94, 0.04), "50%");
        assert_eq!(slider_label_value(94, 0.08), "100%");
        assert_eq!(slider_label_value(95, 0.025), "50%");
        assert_eq!(slider_label_value(91, 9_000.0), "9000");
    }

    #[test]
    fn every_physical_switch_has_a_tooltip() {
        for tag in 60..=63 {
            assert!(
                !toggle_tooltip(tag, Language::Russian).is_empty(),
                "switch {tag} has no Russian tooltip"
            );
            assert!(
                !toggle_tooltip(tag, Language::English).is_empty(),
                "switch {tag} has no English tooltip"
            );
        }
    }
    #[test]
    fn weather_summary_shows_current_values_and_custom_state() {
        let mut settings = AppSettings::default();
        let calm = weather_summary(&settings, Language::Russian);
        assert!(calm.contains("Спокойный"));
        assert!(calm.contains("1500"));
        assert!(calm.contains("50% накопления"));
        settings.weather_mode = 3;
        let custom = weather_summary(&settings, Language::Russian);
        assert!(custom.starts_with("Изменённый пресет"));
        assert!(weather_summary(&settings, Language::English).starts_with("Custom preset"));
    }

    #[test]
    fn settings_panel_rows_are_stacked_without_overlap_and_fit_the_panel() {
        let rows = [
            (HEADER_TOP, HEADER_HEIGHT),
            (WEATHER_TITLE_TOP, SECTION_TITLE_HEIGHT),
            (WEATHER_TOP, WEATHER_HEIGHT),
            (WEATHER_META_TOP, WEATHER_META_HEIGHT),
            (WEATHER_HINT_TOP, WEATHER_HINT_HEIGHT),
            (TOOL_TITLE_TOP, SECTION_TITLE_HEIGHT),
            (TOOL_TOP, TOOL_HEIGHT),
            (TOOL_HINT_TOP, TOOL_HINT_HEIGHT),
            (WIND_TITLE_TOP, SECTION_TITLE_HEIGHT),
            (COMPASS_CARD_TOP, COMPASS_CARD_HEIGHT),
            (TOGGLES_TOP, TOGGLE_HEIGHT),
            (CLEAR_TOP, CLEAR_HEIGHT),
            (ADDITIONAL_TOP, ADDITIONAL_HEIGHT),
            (ABOUT_TOP_COLLAPSED, ABOUT_HEIGHT),
        ];
        for pair in rows.windows(2) {
            assert!(
                pair[0].0 + pair[0].1 <= pair[1].0,
                "row at {} overlaps the row at {}",
                pair[0].0,
                pair[1].0
            );
        }
        let (last_top, last_height) = rows[rows.len() - 1];
        assert!(last_top + last_height < PANEL_HEIGHT_COLLAPSED);
    }

    #[test]
    fn expanded_physics_fits_between_disclosure_and_about() {
        let rows = [
            (PHYSICS_TOP, SLIDER_ROW_HEIGHT),
            (PHYSICS_SECOND_ROW_TOP, SLIDER_ROW_HEIGHT),
            (PHYSICS_TOGGLE_TOP, 40.0),
            (ABOUT_TOP_EXPANDED, ABOUT_HEIGHT),
        ];
        for pair in rows.windows(2) {
            assert!(
                pair[0].0 + pair[0].1 <= pair[1].0,
                "row at {} overlaps the row at {}",
                pair[0].0,
                pair[1].0
            );
        }
        const {
            assert!(PANEL_HEIGHT_COLLAPSED < 880.0);
            assert!(PANEL_HEIGHT_EXPANDED < 880.0);
        }
    }

    #[test]
    fn panel_frame_converts_top_offsets_to_appkit_coordinates() {
        let frame = panel_frame(24.0, 20.0, 100.0, 30.0);
        assert_eq!(frame.origin.x, 24.0);
        assert_eq!(frame.origin.y, 20.0);
        assert_eq!(frame.size.width, 100.0);
        assert_eq!(frame.size.height, 30.0);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn compass_buttons_fit_inside_the_compass_card() {
        let half_height = COMPASS_CARD_HEIGHT / 2.0;
        assert!(COMPASS_STEP_Y + 15.0 <= half_height);
        assert!(COMPASS_RADIUS * 2.0 <= COMPASS_CARD_HEIGHT);
        assert!(COMPASS_STEP_X + 15.0 <= WIND_CARD_LEFT_WIDTH / 2.0);
        // The small compass stays on the left, clear of the wind sliders.
        assert!(COMPASS_CENTER_X - COMPASS_RADIUS >= PANEL_PADDING);
        assert!(COMPASS_CENTER_X + COMPASS_RADIUS < WIND_SLIDER_X);
        // Wind sliders fit beside the compass inside the same card.
        assert!(WIND_SLIDER_FIRST_TOP >= COMPASS_CARD_TOP);
        assert!(
            WIND_SLIDER_SECOND_TOP + SLIDER_ROW_HEIGHT <= COMPASS_CARD_TOP + COMPASS_CARD_HEIGHT
        );
        assert!(WIND_SLIDER_X + WIND_SLIDER_WIDTH <= PANEL_WIDTH - PANEL_PADDING);
        // Diagonal buttons sit on the outer ring rather than far outside it.
        let diagonal = COMPASS_STEP_X.hypot(COMPASS_STEP_Y);
        assert!((diagonal - COMPASS_RADIUS).abs() < 6.0);
    }

    #[test]
    fn weather_presets_apply_the_described_configurations() {
        let mut light = AppSettings::default();
        apply_weather_preset(&mut light, 0);
        assert_eq!(light.particle_count, 800);
        assert_eq!(light.wind_strength, 0.0);
        assert_eq!(light.accumulation_speed, 0.0);
        assert!(!light.particles_in_windows);

        let mut calm = AppSettings::default();
        apply_weather_preset(&mut calm, 1);
        assert_eq!(calm.accumulation_speed, 0.5);
        assert_eq!(calm.particle_count, 1_500);
        assert!(calm.wind_strength > 0.0 && calm.wind_strength < 0.02);
        assert_eq!(calm.wind_direction, 0.0);

        let mut blizzard = AppSettings::default();
        apply_weather_preset(&mut blizzard, 2);
        assert_eq!(blizzard.accumulation_speed, 1.0);
        assert_eq!(blizzard.particle_count, 20_000);
        assert_eq!(blizzard.wind_strength, 0.08);
        assert!(blizzard.particles_in_windows);
    }

    #[test]
    fn wind_direction_maps_to_the_matching_compass_button() {
        assert_eq!(direction_tag(0.0), 44);
        assert_eq!(direction_tag(std::f32::consts::FRAC_PI_2), 46);
        assert_eq!(direction_tag(std::f32::consts::PI), 43);
        assert_eq!(direction_tag(3.0 * std::f32::consts::FRAC_PI_2), 41);
    }

    #[test]
    fn tool_hints_and_weather_text_switch_language() {
        assert!(tool_hint_text(30, Language::Russian).starts_with("Варежка"));
        assert!(tool_hint_text(31, Language::Russian).starts_with("Лопата"));
        assert!(tool_hint_text(30, Language::English).starts_with("Mitten"));
        assert!(tool_hint_text(31, Language::English).starts_with("Shovel"));
        assert_eq!(language_from_code("ru-RU"), Language::Russian);
        assert_eq!(language_from_code("en-US"), Language::English);
        assert_eq!(language_from_code("de-DE"), Language::English);
    }

    #[test]
    fn top_frame_converts_card_offsets_to_appkit_coordinates() {
        let frame = top_frame(80.0, 10.0, 13.0, 26.0, 26.0);
        assert_eq!(frame.origin.x, 10.0);
        assert_eq!(frame.origin.y, 41.0);
        assert_eq!(frame.size.width, 26.0);
        assert_eq!(frame.size.height, 26.0);
    }
}
