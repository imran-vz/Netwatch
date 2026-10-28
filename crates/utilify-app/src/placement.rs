//! Where the dashboard goes: centered under the cursor (which just clicked
//! the status item), just below the menu bar, kept on screen.

/// A rectangle in Cocoa screen coordinates (origin bottom-left, y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CocoaRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl CocoaRect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// Window origin relative to its display's top-left, as GPUI expects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub display_id: Option<u32>,
    /// Height from the window's top to the bottom of the visible area
    /// (above the Dock), less a margin.
    pub room: f64,
}

const MARGIN: f64 = 8.0;
const GAP: f64 = 6.0;

/// `frame` and `visible` describe the screen holding the cursor;
/// `visible` excludes the menu bar.
pub fn place(cursor_x: f64, frame: CocoaRect, visible: CocoaRect, w: f64) -> (f64, f64) {
    let menu_bar = (frame.y + frame.h) - (visible.y + visible.h);
    let max_x = (frame.w - w - MARGIN).max(MARGIN);
    let x = (cursor_x - frame.x - w / 2.0).clamp(MARGIN, max_x);
    (x, menu_bar.max(0.0) + GAP)
}

/// Usable height below the menu bar on a screen with this visible frame.
pub fn room(visible: CocoaRect) -> f64 {
    (visible.h - GAP - MARGIN).max(200.0)
}

/// Pick the screen containing `cursor` (Cocoa coordinates).
pub fn screen_for(
    cursor: (f64, f64),
    screens: &[(CocoaRect, CocoaRect, Option<u32>)],
) -> Option<&(CocoaRect, CocoaRect, Option<u32>)> {
    screens
        .iter()
        .find(|(f, _, _)| f.contains(cursor.0, cursor.1))
        .or_else(|| screens.first())
}

#[cfg(target_os = "macos")]
pub fn below_menu_bar(w: f64) -> Option<Placement> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSScreen};
    use objc2_foundation::{NSNumber, NSRect, NSString};

    let mtm = MainThreadMarker::new()?;
    let cursor = NSEvent::mouseLocation();
    let rect = |r: NSRect| CocoaRect {
        x: r.origin.x,
        y: r.origin.y,
        w: r.size.width,
        h: r.size.height,
    };
    let key = NSString::from_str("NSScreenNumber");
    let screens: Vec<_> = NSScreen::screens(mtm)
        .iter()
        .map(|s| {
            let id = s
                .deviceDescription()
                .objectForKey(&key)
                .and_then(|n| n.downcast::<NSNumber>().ok())
                .map(|n| n.unsignedIntValue());
            (rect(s.frame()), rect(s.visibleFrame()), id)
        })
        .collect();
    let (frame, visible, id) = *screen_for((cursor.x, cursor.y), &screens)?;
    let mut cursor_x = cursor.x;
    // Debug builds: UTILIFY_DEV_SCREEN=main opens it on the main display
    // (top right), whatever screen the pointer is on. For screenshots.
    #[cfg(debug_assertions)]
    let (frame, visible, id) = match (
        std::env::var("UTILIFY_DEV_SCREEN").as_deref(),
        screens.first(),
    ) {
        (Ok("main"), Some(&(f, v, i))) => {
            cursor_x = f.x + f.w - 200.0;
            (f, v, i)
        }
        _ => (frame, visible, id),
    };
    let (x, y) = place(cursor_x, frame, visible, w);
    Some(Placement {
        x,
        y,
        display_id: id,
        room: room(visible),
    })
}

#[cfg(not(target_os = "macos"))]
pub fn below_menu_bar(_w: f64) -> Option<Placement> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: CocoaRect = CocoaRect {
        x: 0.0,
        y: 0.0,
        w: 1512.0,
        h: 982.0,
    };
    const MAIN_VISIBLE: CocoaRect = CocoaRect {
        x: 0.0,
        y: 70.0,
        w: 1512.0,
        h: 875.0, // 37 pt notch menu bar on top, Dock below
    };

    #[test]
    fn centers_under_the_cursor_below_the_menu_bar() {
        assert_eq!(place(1000.0, MAIN, MAIN_VISIBLE, 340.0), (830.0, 43.0));
    }

    #[test]
    fn room_is_the_visible_height_less_margins() {
        assert_eq!(room(MAIN_VISIBLE), 875.0 - 14.0);
    }

    #[test]
    fn stays_on_screen_at_the_edges() {
        assert_eq!(place(1500.0, MAIN, MAIN_VISIBLE, 340.0).0, 1164.0);
        assert_eq!(place(5.0, MAIN, MAIN_VISIBLE, 340.0).0, 8.0);
    }

    #[test]
    fn uses_the_screen_under_the_cursor() {
        let side = CocoaRect {
            x: 1512.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let side_visible = CocoaRect { h: 1056.0, ..side };
        let screens = [(MAIN, MAIN_VISIBLE, Some(1)), (side, side_visible, Some(2))];
        let (f, v, id) = screen_for((2000.0, 1070.0), &screens).unwrap();
        assert_eq!(*id, Some(2));
        // Relative to that display: 2000 - 1512 - 170.
        assert_eq!(place(2000.0, *f, *v, 340.0), (318.0, 30.0));
        assert_eq!(screen_for((-50.0, 0.0), &screens).unwrap().2, Some(1));
    }
}
