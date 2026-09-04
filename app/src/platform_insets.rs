//! How much of the header the window manager has already spoken for.
//!
//! Crook draws its own header across the full width of the window. Whether the
//! OS draws its window controls *on top of* that header or in a title bar
//! above it is the whole question here, and it has one answer per window, not
//! one per platform: a window the window manager decorates keeps its controls
//! in its own bar and costs the header nothing, while an undecorated window
//! that draws its own title bar has to leave room for whatever the system
//! still paints over it.
//!
//! Crook draws no controls of its own, so there is exactly one thing left to
//! reserve for: macOS's traffic lights, which AppKit paints over Crook's
//! surface on a client-decorated window. Windows and Linux hand the whole
//! header back — their undecorated windows have nothing over them at all.
//!
//! This is worth its own module and its own tests because the platform that
//! needs the reservation is not necessarily the one you are looking at:
//! reserve where nothing is painted and the header ends in a hole nobody on
//! macOS will ever see; fail to reserve on macOS and the first tab sits under
//! the green light.
//!
//! # Which element the reservation belongs to
//!
//! "How much" is only half the answer. The other half is *who pays*: the
//! controls sit at the two top corners of the window, and Crook's tabs are a
//! panel down the left edge — so the panel owns the top-left corner and the
//! header owns only the top-right. Reserving the left end on the header
//! instead puts macOS's traffic lights straight through the panel's search
//! box, and it is invisible on Windows and Linux, where the controls are on
//! the other side. [`WindowControlInsets::split`] is what makes that one
//! decision instead of two guesses, and it is tested for every platform.
//!
//! The panel spends its end on a strip of nothing above its first row, rather
//! than on padding beside one — see `tabs_panel::title_strip`. A 70px indent
//! on a 248px column leaves no room to type a path into the search box, and
//! the reservation is a fact about the corner rather than about whatever
//! happens to be drawn in it. Which is also why `panel_left` decides whether
//! that strip is drawn at all: nonzero on a client-decorated macOS window and
//! nowhere else, so nowhere else has a bar.

/// Who draws the window's controls, and therefore whether they overlap the
/// header.
///
/// The windowing layer's own enum, because it is one decision and not two:
/// [`WINDOW_CHROME`](crate::WINDOW_CHROME) is what a window is *opened* with
/// and what the header reserves for, and a second copy of the type here would
/// be a second place for those to drift apart.
pub use crookui::WindowChrome;

/// Room to leave at each end of the header for the window's own controls.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct WindowControlInsets {
    /// Logical pixels to leave before the first tab.
    pub left: f32,
    /// Logical pixels to leave after the last header item.
    pub right: f32,
}

impl WindowControlInsets {
    /// Nothing reserved at either end.
    pub const NONE: Self = Self {
        left: 0.,
        right: 0.,
    };
}

/// The reservation, already handed to the elements that owe it.
///
/// Three numbers rather than two because two elements can be under the
/// controls at once, and each has to be told separately — the header cannot
/// pad itself out of the way of something sitting over the panel.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct LayoutInsets {
    /// Logical pixels the tabs panel must leave free before its first control.
    /// Always zero when there is no panel.
    pub panel_left: f32,
    /// Logical pixels the header must leave free at its left edge.
    pub header_left: f32,
    /// Logical pixels the header must leave free at its right edge.
    pub header_right: f32,
}

impl WindowControlInsets {
    /// Which element owes which end of this reservation.
    ///
    /// The left end follows the top-left corner of the window, which the tabs
    /// panel owns; the right end follows the top-right, which is the header's.
    /// There was once a second arrangement to choose between — a strip across
    /// the header, owing both ends — and this was a `split_for` that took it as
    /// a parameter.
    pub const fn split(self) -> LayoutInsets {
        LayoutInsets {
            panel_left: self.left,
            header_left: 0.,
            header_right: self.right,
        }
    }
}

/// How far into the window macOS's traffic lights reach, in logical pixels.
///
/// Measured on the live window rather than taken from a specification: the
/// three buttons this build's macOS draws span x = 9.0 to 68.5, so 70 is the
/// first whole point clear of the zoom button. The earlier 64 was four and a
/// half points short, and only the header's own padding kept the first tab off
/// the green light.
///
/// This is what the lights *occupy*, not what they need around them. The gap
/// after them is the padding the element beside them was going to have anyway,
/// which is why it is not added twice.
const TRAFFIC_LIGHTS: f32 = 70.;

/// Where a platform puts its window controls.
///
/// Named rather than derived at each call site so all three answers can be
/// tested on one machine — `cfg!` collapses to a single branch at compile
/// time, which would otherwise make two thirds of this untestable anywhere.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ControlLayout {
    /// Close, minimise and zoom on the left, as three round lights.
    MacOs,
    /// Minimise, maximise and close on the right, as wide flat buttons.
    Windows,
    /// Minimise, maximise and close on the right, as drawn by the desktop
    /// environment. Also the answer for anything not otherwise recognised.
    Freedesktop,
}

impl ControlLayout {
    /// The layout of the platform this binary was built for.
    pub const fn host() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Freedesktop
        }
    }

    /// What this layout costs a header under `chrome`.
    ///
    /// `fullscreen` only matters to a client-decorated window: macOS moves the
    /// traffic lights into the menu-bar overlay there, and the reservation has
    /// to go with them or the tabs never reach the left edge.
    pub const fn insets(self, chrome: WindowChrome, fullscreen: bool) -> WindowControlInsets {
        if matches!(chrome, WindowChrome::Native) {
            return WindowControlInsets::NONE;
        }

        match self {
            Self::MacOs if fullscreen => WindowControlInsets::NONE,
            Self::MacOs => WindowControlInsets {
                left: TRAFFIC_LIGHTS,
                right: 0.,
            },
            // Nothing at all: an undecorated window on Windows or Linux has no
            // controls over Crook's surface, and Crook draws none there
            // itself. The header runs to the corner of the window, and the
            // desktop's own shortcuts are what minimise, maximise and close
            // it — see `workspace::title_bar`.
            Self::Windows | Self::Freedesktop => WindowControlInsets::NONE,
        }
    }
}

/// What the header must leave free on this platform, for a window with this
/// chrome.
pub fn window_control_insets(chrome: WindowChrome, fullscreen: bool) -> WindowControlInsets {
    ControlLayout::host().insets(chrome, fullscreen)
}

/// The same, already divided between the panel and the header.
///
/// "How much" and "who pays" as one value, so no caller can get the second
/// half right on the platform it was written on and wrong on the other two.
///
/// For the platform this build is running on. The workspace composes the same
/// two calls itself — [`ControlLayout::insets`] then
/// [`WindowControlInsets::split`] — because `--controls` lets it be asked
/// about a platform that is not this one.
pub fn layout_insets(chrome: WindowChrome, fullscreen: bool) -> LayoutInsets {
    window_control_insets(chrome, fullscreen).split()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUTS: [ControlLayout; 3] = [
        ControlLayout::MacOs,
        ControlLayout::Windows,
        ControlLayout::Freedesktop,
    ];

    #[test]
    fn a_natively_decorated_window_costs_the_header_nothing_anywhere() {
        for layout in LAYOUTS {
            for fullscreen in [false, true] {
                assert_eq!(
                    layout.insets(WindowChrome::Native, fullscreen),
                    WindowControlInsets::NONE,
                    "{layout:?} reserved space for controls the window manager draws elsewhere"
                );
            }
        }
    }

    /// Where the far edge of the zoom button actually is, in logical pixels.
    ///
    /// Measured off a screenshot of the running window rather than taken from
    /// a header file: AppKit draws the three buttons at 9.0..68.5 on this
    /// build's macOS, and the only way to find that out is to look. The
    /// reservation has to clear it, and the failure when it does not is silent
    /// — the first tab creeps under the green light and nothing but the
    /// header's own padding is holding it off.
    const MEASURED_TRAFFIC_LIGHTS_END: f32 = 68.5;

    #[test]
    fn the_reservation_clears_the_last_traffic_light() {
        let reserved = ControlLayout::MacOs
            .insets(WindowChrome::Client, false)
            .left;
        assert!(
            reserved >= MEASURED_TRAFFIC_LIGHTS_END,
            "{reserved} of reservation for lights that reach {MEASURED_TRAFFIC_LIGHTS_END}"
        );
    }

    #[test]
    fn macos_reserves_the_left_edge_and_gives_it_back_in_fullscreen() {
        assert_eq!(
            ControlLayout::MacOs.insets(WindowChrome::Client, false),
            WindowControlInsets {
                left: TRAFFIC_LIGHTS,
                right: 0.
            }
        );
        assert_eq!(
            ControlLayout::MacOs.insets(WindowChrome::Client, true),
            WindowControlInsets::NONE
        );
    }

    #[test]
    fn windows_and_freedesktop_reserve_nothing_in_any_state() {
        // A frameless window with no controls of Crook's own over it: every
        // pixel of the header is the header's, maximised, fullscreen or
        // neither. A reservation here is a hole in the top-right corner on the
        // two platforms this project has no machine for.
        for layout in [ControlLayout::Windows, ControlLayout::Freedesktop] {
            for fullscreen in [false, true] {
                assert_eq!(
                    layout.insets(WindowChrome::Client, fullscreen),
                    WindowControlInsets::NONE,
                    "{layout:?} reserved room for controls nothing draws"
                );
            }
        }
    }

    #[test]
    fn a_natively_decorated_window_costs_nothing_anywhere() {
        // The state Crook actually ships in, on every platform. A regression
        // here is a hole in the header or in the panel.
        for layout in LAYOUTS {
            assert_eq!(
                layout.insets(WindowChrome::Native, false).split(),
                LayoutInsets::default(),
                "{layout:?} reserved space for controls drawn elsewhere"
            );
        }
    }

    #[test]
    fn the_panel_takes_the_left_reservation_and_the_header_takes_none_of_it() {
        // macOS is the platform where this is visible at all: its lights are
        // top-left, which is the corner the panel owns. Getting it wrong there
        // puts them through the panel's search box, and nobody on Windows or
        // Linux would ever see it.
        assert_eq!(
            ControlLayout::MacOs
                .insets(WindowChrome::Client, false)
                .split(),
            LayoutInsets {
                panel_left: TRAFFIC_LIGHTS,
                header_left: 0.,
                header_right: 0.
            }
        );
    }

    #[test]
    fn a_right_hand_reservation_stays_on_the_header() {
        // No platform reserves a right end today, so this is the rule rather
        // than a number: whatever the right inset is, it is the header's and
        // never the panel's. A `split` that swapped both ends would still pass
        // the macOS test above and would put a future reservation under the
        // panel, on the opposite side of the window from the controls.
        let insets = WindowControlInsets {
            left: 0.,
            right: 120.,
        };
        assert_eq!(
            insets.split(),
            LayoutInsets {
                panel_left: 0.,
                header_left: 0.,
                header_right: 120.
            }
        );
    }

    #[test]
    fn every_platform_reserves_exactly_what_the_controls_need() {
        // The whole table at once: what a split hands out is never more and
        // never less than what the platform asked for.
        for layout in LAYOUTS {
            for chrome in [WindowChrome::Native, WindowChrome::Client] {
                for fullscreen in [false, true] {
                    let insets = layout.insets(chrome, fullscreen);
                    let split = insets.split();
                    assert_eq!(
                        split.panel_left + split.header_left,
                        insets.left,
                        "{layout:?}/{chrome:?} lost or invented left inset"
                    );
                    assert_eq!(
                        split.header_right, insets.right,
                        "{layout:?}/{chrome:?} moved the right inset"
                    );
                }
            }
        }
    }

    #[test]
    fn the_host_layout_is_the_one_this_build_targets() {
        let expected = if cfg!(target_os = "macos") {
            ControlLayout::MacOs
        } else if cfg!(target_os = "windows") {
            ControlLayout::Windows
        } else {
            ControlLayout::Freedesktop
        };

        assert_eq!(ControlLayout::host(), expected);
        assert_eq!(
            window_control_insets(WindowChrome::Client, false),
            expected.insets(WindowChrome::Client, false)
        );
        assert_eq!(
            layout_insets(WindowChrome::Client, false),
            expected.insets(WindowChrome::Client, false).split()
        );
    }
}
