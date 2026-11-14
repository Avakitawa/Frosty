use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType, CGRect};
use objc2_core_graphics::{
    kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowNumber,
    kCGWindowOwnerName, kCGWindowOwnerPID, CGRectMakeWithDictionaryRepresentation,
    CGWindowListCopyWindowInfo, CGWindowListOption,
};

#[derive(Clone, Copy, Debug)]
pub struct VisibleWindow {
    pub id: u64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub is_dock: bool,
    pub is_launchpad: bool,
}

#[derive(Debug)]
pub struct WindowSnapshot {
    pub top_edges: Vec<VisibleWindow>,
    pub dock_windows: Vec<VisibleWindow>,
    pub blocked_regions: Vec<VisibleWindow>,
}

#[derive(Clone, Debug)]
struct WindowGeometry {
    id: u32,
    owner_pid: i64,
    owner_name: String,
    window_name: String,
    layer: i64,
    alpha: f64,
    x: f64,
    cg_top: f64,
    y: f64,
    width: f64,
    height: f64,
}

pub fn visible_windows(appkit_top: f64, display_area: f64) -> Result<WindowSnapshot, String> {
    let descriptions = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    )
    .ok_or_else(|| "Core Graphics could not list visible windows".to_owned())?;
    let descriptions = unsafe { descriptions.cast_unchecked::<CFType>() };
    let mut windows = Vec::with_capacity(descriptions.len());

    for index in 0..descriptions.len() {
        let Some(description) = descriptions.get(index) else {
            continue;
        };
        let Some(description) = description.downcast_ref::<CFDictionary>() else {
            continue;
        };
        let description = unsafe { description.cast_unchecked::<CFString, CFType>() };
        let Some(layer) = number_value(description, unsafe { kCGWindowLayer }) else {
            continue;
        };
        let Some(owner_pid) = number_value(description, unsafe { kCGWindowOwnerPID }) else {
            continue;
        };
        let Some(id) = number_value(description, unsafe { kCGWindowNumber }) else {
            continue;
        };
        let Some(alpha) = float_value(description, unsafe { kCGWindowAlpha }) else {
            continue;
        };
        let owner_name =
            string_value(description, unsafe { kCGWindowOwnerName }).unwrap_or_default();
        let window_name = string_value(description, unsafe { kCGWindowName }).unwrap_or_default();
        if alpha <= 0.01 || owner_pid == std::process::id() as i64 {
            continue;
        }

        let Some(bounds_value) = description.get(unsafe { kCGWindowBounds }) else {
            continue;
        };
        let Some(bounds) = bounds_value.downcast_ref::<CFDictionary>() else {
            continue;
        };
        let mut rect = CGRect::default();
        if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(bounds), &mut rect) } {
            continue;
        }
        let (x, y, width, height) = (
            rect.origin.x,
            appkit_top - rect.origin.y - rect.size.height,
            rect.size.width,
            rect.size.height,
        );
        if !x.is_finite()
            || !y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
        {
            continue;
        }
        windows.push(WindowGeometry {
            id: id as u32,
            owner_pid,
            owner_name,
            window_name,
            layer,
            alpha,
            x,
            cg_top: rect.origin.y,
            y,
            width,
            height,
        });
    }

    let blocked_regions = windows
        .iter()
        .filter(|window| is_blocking_snow(window))
        .map(|window| VisibleWindow {
            id: u64::from(window.id),
            x: window.x,
            y: window.y,
            width: window.width,
            height: window.height,
            is_dock: window.owner_name == "Dock",
            is_launchpad: false,
        })
        .collect();
    let dock_windows = windows
        .iter()
        .filter(|window| is_dock_window(window, display_area))
        .map(|window| VisibleWindow {
            id: u64::from(window.id),
            x: window.x,
            y: window.y,
            width: window.width,
            height: window.height,
            is_dock: window.owner_name == "Dock",
            is_launchpad: false,
        })
        .collect();
    Ok(WindowSnapshot {
        top_edges: exposed_top_segments(&windows),
        dock_windows,
        blocked_regions,
    })
}

fn exposed_top_segments(windows: &[WindowGeometry]) -> Vec<VisibleWindow> {
    let mut segments = Vec::new();
    for (index, window) in windows.iter().enumerate() {
        let launchpad = is_launchpad(window);
        if (window.layer != 0 && !launchpad)
            || (window.owner_name == "Dock" && !launchpad)
            || window.alpha <= 0.01
            || window.owner_pid == std::process::id() as i64
            || window.width < 40.0
            || window.height < 20.0
        {
            continue;
        }

        let mut visible = vec![(window.x, window.x + window.width)];
        for foreground in &windows[..index] {
            if (foreground.layer != 0 && !is_launchpad(foreground))
                || (foreground.owner_name == "Dock" && !is_launchpad(foreground))
            {
                continue;
            }
            let foreground_bottom = foreground.cg_top + foreground.height;
            if foreground.alpha <= 0.01
                || foreground.cg_top > window.cg_top
                || foreground_bottom <= window.cg_top
            {
                continue;
            }
            let cover_left = foreground.x;
            let cover_right = foreground.x + foreground.width;
            visible = visible
                .into_iter()
                .flat_map(|(left, right)| {
                    if cover_right <= left || cover_left >= right {
                        vec![(left, right)]
                    } else {
                        let mut remaining = Vec::with_capacity(2);
                        if cover_left > left {
                            remaining.push((left, cover_left.min(right)));
                        }
                        if cover_right < right {
                            remaining.push((cover_right.max(left), right));
                        }
                        remaining
                    }
                })
                .collect();
            if visible.is_empty() {
                break;
            }
        }

        for (segment_index, (left, right)) in visible.into_iter().enumerate() {
            if right - left < 4.0 {
                continue;
            }
            segments.push(VisibleWindow {
                id: (u64::from(window.id) << 32) | segment_index as u64,
                x: left,
                y: window.y,
                width: right - left,
                height: window.height,
                is_dock: window.owner_name == "Dock",
                is_launchpad: is_launchpad(window),
            });
        }
    }
    segments
}

fn is_launchpad(window: &WindowGeometry) -> bool {
    if window.owner_name != "Dock" {
        return false;
    }

    window
        .window_name
        .to_ascii_lowercase()
        .contains("launchpad")
        || (window.layer <= 1 && window.width >= 600.0 && window.height >= 400.0)
}

fn is_dock_window(window: &WindowGeometry, display_area: f64) -> bool {
    window.owner_name == "Dock"
        && window.layer >= 0
        && !is_launchpad(window)
        && window.alpha > 0.01
        && window.width >= 20.0
        && window.height >= 20.0
        && window.width * window.height < display_area * 0.25
}

fn is_blocking_snow(window: &WindowGeometry) -> bool {
    let application_window = window.layer == 0
        && !is_launchpad(window)
        && window.owner_name != "Dock"
        && window.owner_pid != std::process::id() as i64;
    application_window && window.alpha > 0.01 && window.width >= 4.0 && window.height >= 4.0
}

fn float_value(dictionary: &CFDictionary<CFString, CFType>, key: &'static CFString) -> Option<f64> {
    dictionary
        .get(key)
        .and_then(|value| value.downcast_ref::<CFNumber>().and_then(CFNumber::as_f64))
}

fn number_value(
    dictionary: &CFDictionary<CFString, CFType>,
    key: &'static CFString,
) -> Option<i64> {
    dictionary
        .get(key)
        .and_then(|value| value.downcast_ref::<CFNumber>().and_then(CFNumber::as_i64))
}

fn string_value(
    dictionary: &CFDictionary<CFString, CFType>,
    key: &'static CFString,
) -> Option<String> {
    dictionary
        .get(key)
        .and_then(|value| value.downcast_ref::<CFString>().map(ToString::to_string))
}

#[cfg(test)]
mod tests {
    use super::{
        exposed_top_segments, is_blocking_snow, is_dock_window, is_launchpad, WindowGeometry,
    };

    fn window(id: u32, x: f64, cg_top: f64, width: f64, height: f64) -> WindowGeometry {
        WindowGeometry {
            id,
            owner_pid: 123,
            owner_name: "Test".to_owned(),
            window_name: String::new(),
            layer: 0,
            alpha: 1.0,
            x,
            cg_top,
            y: 1_000.0 - cg_top - height,
            width,
            height,
        }
    }

    #[test]
    fn removes_fully_occluded_window_edges() {
        let foreground = window(1, 0.0, 100.0, 200.0, 300.0);
        let background = window(2, 20.0, 200.0, 100.0, 200.0);

        let segments = exposed_top_segments(&[foreground, background]);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].id >> 32, 1);
    }

    #[test]
    fn splits_partially_occluded_edges_into_visible_segments() {
        let foreground = window(1, 40.0, 50.0, 20.0, 100.0);
        let background = window(2, 0.0, 100.0, 100.0, 200.0);

        let segments = exposed_top_segments(&[foreground, background]);

        assert_eq!(segments.len(), 2);
        assert_eq!((segments[0].x, segments[0].width), (0.0, 40.0));
        assert_eq!((segments[1].x, segments[1].width), (60.0, 40.0));
    }

    #[test]
    fn windows_not_crossing_an_edge_do_not_hide_it() {
        let foreground = window(1, 0.0, 10.0, 100.0, 20.0);
        let background = window(2, 20.0, 100.0, 50.0, 150.0);

        let segments = exposed_top_segments(&[foreground, background]);

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].width, 50.0);
    }

    #[test]
    fn windows_touching_an_edge_without_covering_it_do_not_hide_it() {
        let foreground = window(1, 0.0, 50.0, 100.0, 50.0);
        let background = window(2, 20.0, 100.0, 50.0, 150.0);

        let segments = exposed_top_segments(&[foreground, background]);

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].width, 50.0);
    }

    #[test]
    fn launchpad_windows_are_supported_even_when_their_layer_is_not_zero() {
        let mut launchpad = window(1, 0.0, 100.0, 1_000.0, 800.0);
        launchpad.owner_name = "Dock".to_owned();
        launchpad.layer = 1;

        let segments = exposed_top_segments(&[launchpad]);

        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn dock_desktop_backing_window_does_not_hide_application_edges() {
        let mut dock_backing = window(1, 0.0, 0.0, 1_512.0, 982.0);
        dock_backing.owner_name = "Dock".to_owned();
        dock_backing.window_name = "Dock".to_owned();
        dock_backing.layer = 20;
        let application = window(2, 158.0, 43.0, 1_284.0, 801.0);
        let application_y = application.y;

        let segments = exposed_top_segments(&[dock_backing, application]);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].id >> 32, 2);
        assert_eq!(segments[0].y, application_y);
    }

    #[test]
    fn dock_desktop_backing_window_is_not_launchpad() {
        let mut dock_backing = window(1, 0.0, 0.0, 1_512.0, 982.0);
        dock_backing.owner_name = "Dock".to_owned();
        dock_backing.window_name = "Dock".to_owned();
        dock_backing.layer = 20;

        assert!(!is_launchpad(&dock_backing));
    }

    #[test]
    fn dock_detection_accepts_side_docks_but_rejects_fullscreen_backing_windows() {
        let mut side_dock = window(1, 0.0, 100.0, 70.0, 700.0);
        side_dock.owner_name = "Dock".to_owned();
        side_dock.window_name = "Dock".to_owned();
        side_dock.layer = 20;

        assert!(is_dock_window(&side_dock, 1_512.0 * 982.0));

        let mut backing = window(2, 0.0, 0.0, 1_512.0, 982.0);
        backing.owner_name = "Dock".to_owned();
        backing.window_name = "Dock".to_owned();
        backing.layer = 20;
        assert!(!is_dock_window(&backing, 1_512.0 * 982.0));
    }

    #[test]
    fn menu_bar_window_is_not_a_snow_surface_or_window_occluder() {
        let mut menu_bar = window(1, 0.0, 0.0, 1_512.0, 33.0);
        menu_bar.owner_name = "Window Server".to_owned();
        menu_bar.window_name = "Menubar".to_owned();
        menu_bar.layer = 24;
        let application = window(2, 158.0, 43.0, 1_284.0, 801.0);

        let segments = exposed_top_segments(&[menu_bar, application]);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].id >> 32, 2);
    }

    #[test]
    fn menu_bar_is_a_pass_through_region_for_snow() {
        let mut menu_bar = window(1, 0.0, 0.0, 1_512.0, 33.0);
        menu_bar.owner_name = "Window Server".to_owned();
        menu_bar.window_name = "Menubar".to_owned();
        menu_bar.layer = 24;

        assert!(!is_blocking_snow(&menu_bar));
        assert!(exposed_top_segments(&[menu_bar]).is_empty());
    }
}
