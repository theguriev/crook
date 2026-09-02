//! How much of the header the window manager has already spoken for.
//!
//! Crook draws its own header across the full width of the window. Whether the
//! OS draws its window controls *on top of* that header or in a title bar
//! above it is the whole question here, and it has one answer per window, not
//! one per platform: a window the window manager decorates keeps its controls
//! in its own bar and costs the header nothing, while an undecorated window
//! that draws its own title bar has to leave room for them.
//!
//! This is worth its own module and its own tests because it is invisible on
//! the machine you develop on and wrong on the other two: reserve when there
//! is nothing to reserve for and the header ends in a 136px hole on Windows
//! that no one on macOS will ever see; fail to reserve on a client-decorated
//! window and the first tab sits under the close button.

/// Who draws the window's controls, and therefore whether they overlap the
/// header.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WindowChrome {
    /// The window manager draws a title bar above the client area. Its
    /// controls are not over Crook's header, so the header owes them nothing.
    Native,
    /// The window is borderless and Crook's header *is* the title bar, so the
    /// controls are drawn over it.
    Client,
}

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
            // The three traffic lights plus the margins around them.
            Self::MacOs => WindowControlInsets {
                left: 64.,
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

    #[test]
    fn macos_reserves_the_left_edge_and_gives_it_back_in_fullscreen() {
        assert_eq!(
            ControlLayout::MacOs.insets(WindowChrome::Client, false),
            WindowControlInsets {
                left: 64.,
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
    }
}
