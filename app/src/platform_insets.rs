//! How much of the header the window manager has already spoken for.
//!
//! Crook draws its own header across the full width of the window. Whether the
//! OS draws its window controls *on top of* that header or in a title bar
//! above it is the whole question here, and it has one answer per window, not
//! one per platform: a window the window manager decorates keeps its controls
//! in its own bar and costs the header nothing, while an undecorated window
//! that draws its own title bar has to leave room for them.
//!
//! This is worth its own module and its own tests because two thirds of it is
//! invisible on whichever machine you develop on: reserve when there is
//! nothing to reserve for and the header ends in a 136px hole on Windows that
//! no one on macOS will ever see; fail to reserve and the first tab sits under
//! the close button on a platform you are not looking at.
//!
//! Crook opens a client-decorated window, so every number here is live. What
//! is reserved is not always the same thing, either: on macOS the room is for
//! the *system's* traffic lights, painted over Crook's surface by AppKit,
//! while on Windows and Linux it is the room Crook's own caption buttons are
//! drawn in — see `workspace::title_bar`, which measures its cluster against
//! this table so the two can never disagree.
//!
//! # Which element the reservation belongs to
//!
//! "How much" is only half the answer. The other half is *who pays*, and that
//! moved the day the tabs became a panel down the left edge: the controls sit
//! at the two top corners of the window, and which of Crook's elements owns
//! each corner depends on the layout. With a horizontal strip the header spans
//! the whole top edge and owes both ends; with a vertical panel the panel owns
//! the top-left corner and the header only the top-right. Reserving on the
//! header in the vertical layout puts macOS's traffic lights straight through
//! the panel's gear button, and it is invisible on Windows and Linux, where
//! the controls are on the other side. [`TabsPlacement`] is what makes that
//! one decision instead of two guesses, and it is tested for every platform in
//! both layouts.

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

/// Where the window's tabs are, which decides who is under the controls.
///
/// Derived from the layout rather than from the platform: it is Crook's own
/// arrangement, and the same two values mean the same two things on all three
/// operating systems.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TabsPlacement {
    /// A strip inside the header, so the header spans the whole top edge and
    /// carries both ends of the reservation.
    Header,
    /// A panel down the left edge, full window height. The panel's control bar
    /// is the top-left corner and the header is only the top-right, so the two
    /// ends of the reservation go to two different elements.
    LeftPanel,
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
    /// Which element owes which end of this reservation, under `placement`.
    ///
    /// The left end follows the top-left corner of the window and the right
    /// end follows the top-right one; all a placement changes is which element
    /// each corner belongs to. The right end never moves, because no layout
    /// puts anything but the header in the top-right corner.
    pub const fn split_for(self, placement: TabsPlacement) -> LayoutInsets {
        match placement {
            TabsPlacement::Header => LayoutInsets {
                panel_left: 0.,
                header_left: self.left,
                header_right: self.right,
            },
            TabsPlacement::LeftPanel => LayoutInsets {
                panel_left: self.left,
                header_left: 0.,
                header_right: self.right,
            },
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
            // Three 45px caption buttons plus a pixel of separation. Windows
            // keeps them in fullscreen, so there is no fullscreen case.
            Self::Windows => WindowControlInsets {
                left: 0.,
                right: 136.,
            },
            // GNOME and KDE draw narrower buttons than Windows does; 116px is
            // what Warp measured across the common desktop environments.
            Self::Freedesktop => WindowControlInsets {
                left: 0.,
                right: 116.,
            },
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
/// [`WindowControlInsets::split_for`] — because `--controls` lets it be asked
/// about a platform that is not this one.
pub fn layout_insets(
    placement: TabsPlacement,
    chrome: WindowChrome,
    fullscreen: bool,
) -> LayoutInsets {
    window_control_insets(chrome, fullscreen).split_for(placement)
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
    fn windows_and_freedesktop_reserve_the_right_edge_in_every_state() {
        for fullscreen in [false, true] {
            assert_eq!(
                ControlLayout::Windows.insets(WindowChrome::Client, fullscreen),
                WindowControlInsets {
                    left: 0.,
                    right: 136.
                }
            );
            assert_eq!(
                ControlLayout::Freedesktop.insets(WindowChrome::Client, fullscreen),
                WindowControlInsets {
                    left: 0.,
                    right: 116.
                }
            );
        }
    }

    const PLACEMENTS: [TabsPlacement; 2] = [TabsPlacement::Header, TabsPlacement::LeftPanel];

    #[test]
    fn a_natively_decorated_window_costs_either_layout_nothing_anywhere() {
        // The state Crook actually ships in, on every platform and in both
        // layouts. A regression here is a hole in the header or in the panel.
        for layout in LAYOUTS {
            for placement in PLACEMENTS {
                assert_eq!(
                    layout
                        .insets(WindowChrome::Native, false)
                        .split_for(placement),
                    LayoutInsets::default(),
                    "{layout:?} reserved space in {placement:?} for controls drawn elsewhere"
                );
            }
        }
    }

    #[test]
    fn the_panel_takes_over_the_left_reservation_and_only_the_left_one() {
        // macOS is the platform where this is visible at all: its lights are
        // top-left, which is the corner the panel owns. Getting it wrong there
        // puts them through the panel's gear, and nobody on Windows or Linux
        // would ever see it.
        assert_eq!(
            ControlLayout::MacOs
                .insets(WindowChrome::Client, false)
                .split_for(TabsPlacement::Header),
            LayoutInsets {
                panel_left: 0.,
                header_left: TRAFFIC_LIGHTS,
                header_right: 0.
            }
        );
        assert_eq!(
            ControlLayout::MacOs
                .insets(WindowChrome::Client, false)
                .split_for(TabsPlacement::LeftPanel),
            LayoutInsets {
                panel_left: TRAFFIC_LIGHTS,
                header_left: 0.,
                header_right: 0.
            }
        );
    }

    #[test]
    fn a_right_hand_reservation_stays_on_the_header_in_both_layouts() {
        // Windows and Linux put their controls in the top-right corner, which
        // no layout takes away from the header — so moving the tabs must not
        // move this. A `split_for` that swapped both ends would still pass the
        // macOS test above and would leave the usage chip under the close
        // button on the other two platforms.
        for (layout, right) in [
            (ControlLayout::Windows, 136.),
            (ControlLayout::Freedesktop, 116.),
        ] {
            for placement in PLACEMENTS {
                assert_eq!(
                    layout
                        .insets(WindowChrome::Client, false)
                        .split_for(placement),
                    LayoutInsets {
                        panel_left: 0.,
                        header_left: 0.,
                        header_right: right
                    },
                    "{layout:?} moved its right-hand reservation in {placement:?}"
                );
            }
        }
    }

    #[test]
    fn every_platform_and_layout_reserves_exactly_what_the_controls_need() {
        // The whole table at once: what a split hands out is never more and
        // never less than what the platform asked for.
        for layout in LAYOUTS {
            for chrome in [WindowChrome::Native, WindowChrome::Client] {
                for fullscreen in [false, true] {
                    let insets = layout.insets(chrome, fullscreen);
                    for placement in PLACEMENTS {
                        let split = insets.split_for(placement);
                        assert_eq!(
                            split.panel_left + split.header_left,
                            insets.left,
                            "{layout:?}/{chrome:?}/{placement:?} lost or invented left inset"
                        );
                        assert_eq!(
                            split.header_right, insets.right,
                            "{layout:?}/{chrome:?}/{placement:?} moved the right inset"
                        );
                    }
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
        for placement in PLACEMENTS {
            assert_eq!(
                layout_insets(placement, WindowChrome::Client, false),
                expected
                    .insets(WindowChrome::Client, false)
                    .split_for(placement)
            );
        }
    }
}
