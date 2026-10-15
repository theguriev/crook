//! Where the window is on the desktop, and where it may open again.
//!
//! A window that comes back where it was needs facts its frame cannot give:
//! where its corner was, whether it was maximised or fullscreen, and — for a
//! window that was — the size it goes back to, which the frame of a maximised
//! window does not show. [`WindowPlacement`] is those facts, followed as the
//! window moves and resizes and handed to the application to keep.
//! [`onto_a_monitor`] is what a remembered position has to pass before a
//! window opens there, since the monitor it was on may have been unplugged
//! in between.
//!
//! # The desktop's coordinates
//!
//! A position is kept in the coordinates the desktop lays its monitors out
//! in, and those are not one unit everywhere. Windows and X11 place windows
//! in pixels, which is exactly what winit's physical positions are. macOS
//! places them in points, and winit's "physical" position there is a
//! window's points times the scale of the screen it is on — so a window
//! dragged from a Retina screen to a plain one changes coordinates without
//! moving, and a position written on one screen lands somewhere else when it
//! is read back on the other. Points are what macOS itself means by where a
//! window is, so points are what is kept there. Wayland says nothing about
//! where a window is, and nothing is kept.
//!
//! # When the window is looked at
//!
//! Not as each event arrives: an event can come before the state that
//! explains it. Windows sends the move of a maximise — the frame already the
//! work area's — before the resize that tells winit the window is maximised,
//! so a look on the move sees an ordinary window the size of the work area and
//! keeps that as the size to go back to. An event only says the window is
//! worth looking at again, and [`Following`] takes the look once the whole
//! batch of events is over.
//!
//! # What winit cannot see
//!
//! On X11 winit knows only the fullscreen it asked for itself. A window the
//! window manager made fullscreen — the only kind there is, since Crook has
//! no fullscreen of its own — is reported as an ordinary window the size of
//! the monitor, and one winit made fullscreen stays fullscreen to winit after
//! the window manager has taken it out. So on X11 a window whose inside
//! covers a monitor exactly — see [`fills_a_monitor`] — keeps the size it goes
//! back to, as a fullscreen one does. That is a guess, and a lone window in a
//! tiling window manager with no bar covers a monitor too, so it is not kept
//! as fullscreen: the session file is read on Wayland as well, where a
//! window kept as fullscreen is opened fullscreen, over everything. Nor is a
//! window asked to open fullscreen on X11.

use std::time::{Duration, Instant};

use crookui_core::geometry::Vector2F;

/// Where the window is on the desktop and what it is doing, as a later launch
/// would open it.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct WindowPlacement {
    /// Where the window's top-left corner is when it is neither maximised nor
    /// fullscreen, in the desktop's own coordinates: pixels on Windows and
    /// X11, points on macOS.
    ///
    /// `None` where the platform does not say, which is Wayland.
    pub position: Option<[i32; 2]>,
    /// Its inner size in logical pixels when it is neither maximised nor
    /// fullscreen, which is the size a maximised window goes back to.
    pub size: Vector2F,
    /// Whether it fills the work area.
    pub maximized: bool,
    /// Whether it has the whole screen.
    pub fullscreen: bool,
}

/// The window as the platform reports it at one moment.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(super) struct Sighting {
    /// Where its top-left corner is, in the desktop's coordinates.
    pub(super) position: Option<[i32; 2]>,
    /// Its inner size, in logical pixels.
    pub(super) size: Vector2F,
    /// Whether it fills the work area.
    pub(super) maximized: bool,
    /// Whether it has the whole screen.
    pub(super) fullscreen: bool,
    /// Whether its inside covers a monitor exactly, where that is the only
    /// sign the window may be fullscreen — X11 — and `false` elsewhere: see
    /// the module docs.
    pub(super) covers_a_monitor: bool,
    /// Whether it is in the dock or the taskbar, where the platform can say.
    pub(super) minimized: bool,
}

impl WindowPlacement {
    /// The placement to keep once the window has been `seen` as it is now.
    ///
    /// A window that is neither maximised nor fullscreen is where it is and
    /// the size it is. One that is either keeps the corner and the size it
    /// had before, because those are what it goes back to: the frame of a
    /// maximised window is the work area's, and a window opened at that size
    /// and then restored would stay that big. So does one that only covers a
    /// monitor, which on X11 may be fullscreen, but it is not kept as
    /// fullscreen. A minimised window changes nothing, since Windows reports
    /// one at (-32000, -32000) and zero by zero, and that is nowhere a window
    /// should open.
    pub(super) fn after(self, seen: Sighting) -> Self {
        if seen.minimized || seen.size.x() < 1. || seen.size.y() < 1. {
            return self;
        }
        if seen.maximized || seen.fullscreen || seen.covers_a_monitor {
            return Self {
                maximized: seen.maximized,
                fullscreen: seen.fullscreen,
                ..self
            };
        }
        Self {
            position: seen.position,
            size: seen.size,
            maximized: false,
            fullscreen: false,
        }
    }
}

/// A window's placement as it is followed through batches of events — see
/// the module docs for why the look waits for the end of one.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct Following {
    /// The placement as of the last look.
    placement: WindowPlacement,
    /// Whether an event since the last look said the window moved, was
    /// resized or changed scale.
    stirred: bool,
}

impl Following {
    /// Follows a window that has not been looked at yet, and was asked to be
    /// `placement`.
    pub(super) fn new(placement: WindowPlacement) -> Self {
        Self {
            placement,
            stirred: false,
        }
    }

    /// The placement as of the last look.
    pub(super) fn placement(&self) -> WindowPlacement {
        self.placement
    }

    /// Notes that an event said the window moved, was resized or changed
    /// scale, for the look at the end of the batch.
    pub(super) fn stirred(&mut self) {
        self.stirred = true;
    }

    /// Ends a batch of events: looks at the window — `look` is the window as
    /// it is now — if anything in the batch said it changed, and returns the
    /// placement to keep if the look changed it.
    pub(super) fn settle(&mut self, look: impl FnOnce() -> Sighting) -> Option<WindowPlacement> {
        if !self.stirred {
            return None;
        }
        self.see(look())
    }

    /// Takes the window as `seen` whatever was said, and returns the
    /// placement to keep if that changed it: as the window opens and as the
    /// event loop stops, when there is no later batch to wait for.
    pub(super) fn see(&mut self, seen: Sighting) -> Option<WindowPlacement> {
        self.stirred = false;
        let placement = self.placement.after(seen);
        (placement != self.placement).then(|| {
            self.placement = placement;
            placement
        })
    }
}

/// How long macOS is given between refusing a fullscreen and being asked
/// again: winit's own interval, for a window created fullscreen.
const ASK_AGAIN_AFTER: Duration = Duration::from_millis(500);

/// How long after the window opens a refused fullscreen is still asked for.
///
/// What refuses it is the animation into a new Space, which takes about a
/// second; a refusal that outlasts several of those is not going to change,
/// and asking stops rather than keeping a clock running.
const STOP_ASKING_AFTER: Duration = Duration::from_secs(4);

/// A fullscreen asked for as the window opened, followed until AppKit says
/// the window has it.
///
/// macOS refuses to enter fullscreen while it is animating into a new Space
/// — which a window opened from a fullscreen application, a terminal in VS
/// Code, sets off — and winit says so only by forgetting the fullscreen it
/// was asked for. winit asks again by itself for a window given fullscreen as
/// it was created, and not for one asked once it exists, which is how this
/// window is asked: given it at creation, macOS would bring it back from
/// fullscreen at the size of the screen.
///
/// Nothing but AppKit's word ends it early — see
/// [`FullscreenEntered`](super::fullscreen_entered::FullscreenEntered).
/// winit takes the window as fullscreen from the moment it asks, so a
/// refusal still to come looks the same as a fullscreen on its way; and a
/// window opened maximised is as wide as its monitor before macOS has
/// answered, so its size says nothing either.
#[derive(Copy, Clone, Debug)]
pub(super) struct FullscreenAsked {
    /// When the window is next looked at.
    next_look: Instant,
    /// When asking stops.
    stop_at: Instant,
}

/// What to do about a fullscreen asked for and not yet seen.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum FullscreenStep {
    /// Stop: the window has entered it, or has been refused it for too long.
    Over,
    /// Ask again now, and look again then.
    AskAgain(Instant),
    /// Look again then.
    LookAt(Instant),
}

impl FullscreenAsked {
    /// A fullscreen asked for at `now`.
    pub(super) fn new(now: Instant) -> Self {
        Self {
            next_look: now + ASK_AGAIN_AFTER,
            stop_at: now + STOP_ASKING_AFTER,
        }
    }

    /// What to do at `now`, about a window winit takes as `fullscreen` — on
    /// its way there, in it, or about to be refused it — and that AppKit has
    /// said has `entered` it, which only a window in it has.
    ///
    /// Once it has entered, a window winit no longer takes as fullscreen was
    /// taken out by the person, and is not asked again.
    pub(super) fn step(&mut self, now: Instant, fullscreen: bool, entered: bool) -> FullscreenStep {
        if entered || now >= self.stop_at {
            return FullscreenStep::Over;
        }
        if now < self.next_look {
            return FullscreenStep::LookAt(self.next_look);
        }
        self.next_look = now + ASK_AGAIN_AFTER;
        if fullscreen {
            FullscreenStep::LookAt(self.next_look)
        } else {
            FullscreenStep::AskAgain(self.next_look)
        }
    }
}

/// Whether a window whose inside is at `position` and `size`, in the
/// desktop's own coordinates, covers the whole of one of the `monitors` —
/// which is what a fullscreen window does, and on X11 a lone tiled window with
/// no bar beside it.
///
/// A maximised window stops at the panels and the dock, and a window the
/// size of a monitor somewhere else on the desktop is only that big.
pub(super) fn fills_a_monitor(position: [i32; 2], size: [i32; 2], monitors: &[Monitor]) -> bool {
    monitors
        .iter()
        .any(|monitor| monitor.origin == position && monitor.size == size)
}

/// One monitor, in the desktop's coordinates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct Monitor {
    /// Its top-left corner.
    pub(super) origin: [i32; 2],
    /// How wide and how tall it is.
    pub(super) size: [i32; 2],
    /// How many of the desktop's units one logical pixel is on it: its scale
    /// factor where the desktop counts pixels, and one on macOS, which counts
    /// points.
    pub(super) scale: f64,
}

/// How much of a window's top edge has to be on a monitor, along it and below
/// it, in logical pixels, for the window to be left where it was.
///
/// Enough to take hold of and drag the rest back by: the top of the window is
/// its title bar, Crook's own header or the desktop's.
const REACHABLE: f64 = 64.;

/// Where to open a window last seen with its top-left corner at `position`
/// and `size` logical pixels inside, on the `monitors` there are now, and
/// which of them it opens on.
///
/// Where it was, while enough of its top edge is on one of them to drag it
/// by — see [`REACHABLE`]: a window somebody left hanging off an edge is
/// left hanging there. Otherwise onto the monitor it overlaps most, or the
/// one nearest it when it overlaps none — the one it was on has been
/// unplugged — moved by as little as puts it wholly on that monitor, or into
/// its top-left corner when it is bigger than the monitor. `None` when there
/// are no monitors to put it on, where the desktop's own placement is the
/// better answer.
pub(super) fn onto_a_monitor(
    position: [i32; 2],
    size: Vector2F,
    monitors: &[Monitor],
) -> Option<([i32; 2], usize)> {
    let [x, y] = position.map(f64::from);
    // In each monitor's own units, since a logical pixel is a different
    // number of them on a monitor of another scale.
    let extent = |monitor: &Monitor| {
        (
            f64::from(size.x()) * monitor.scale,
            f64::from(size.y()) * monitor.scale,
        )
    };
    let bounds = |monitor: &Monitor| {
        let [left, top] = monitor.origin.map(f64::from);
        let [width, height] = monitor.size.map(f64::from);
        (left, top, left + width, top + height)
    };

    let reachable = monitors.iter().position(|monitor| {
        let (width, height) = extent(monitor);
        let (left, top, right, bottom) = bounds(monitor);
        let reach = REACHABLE * monitor.scale;
        let along = (x + width).min(right) - x.max(left);
        along >= reach.min(width) && y >= top && y + reach.min(height) <= bottom
    });
    if let Some(index) = reachable {
        return Some((position, index));
    }

    // The most overlap first, then the least distance from the window's
    // middle; the first monitor listed wins a tie, which is the primary one
    // on most desktops.
    let mut best: Option<(usize, f64, f64)> = None;
    for (index, monitor) in monitors.iter().enumerate() {
        let (width, height) = extent(monitor);
        let (left, top, right, bottom) = bounds(monitor);
        let overlap = ((x + width).min(right) - x.max(left)).max(0.)
            * ((y + height).min(bottom) - y.max(top)).max(0.);
        let (middle_x, middle_y) = (x + width / 2., y + height / 2.);
        let off_x = (left - middle_x).max(middle_x - right).max(0.);
        let off_y = (top - middle_y).max(middle_y - bottom).max(0.);
        let distance = off_x.hypot(off_y);
        let better = best.is_none_or(|(_, most, nearest)| {
            overlap > most || (overlap == most && distance < nearest)
        });
        if better {
            best = Some((index, overlap, distance));
        }
    }
    let (index, ..) = best?;

    let monitor = &monitors[index];
    let (width, height) = extent(monitor);
    let (left, top, right, bottom) = bounds(monitor);
    let fit = |start: f64, length: f64, low: f64, high: f64| {
        if length >= high - low {
            low
        } else {
            start.clamp(low, high - length)
        }
    };
    Some((
        [
            fit(x, width, left, right).round() as i32,
            fit(y, height, top, bottom).round() as i32,
        ],
        index,
    ))
}

/// The model: what is kept for what the window was seen as, when a batch's
/// look is taken, and what to do about a fullscreen asked for. Not the wiring
/// that feeds it — which events `App` takes as stirring the window, that
/// `about_to_wait` settles it, what `window::sighting` reads off winit on each
/// desktop — since that needs a real window, and tests have none. Nor has any
/// of it been tried on a real desktop.
#[cfg(test)]
mod tests {
    use crookui_core::geometry::vec2f;

    use super::*;

    /// A 1920 by 1080 monitor at `origin`, at a scale of one.
    fn monitor(origin: [i32; 2]) -> Monitor {
        Monitor {
            origin,
            size: [1920, 1080],
            scale: 1.,
        }
    }

    const SIZE: Vector2F = vec2f(800., 600.);

    #[test]
    fn a_window_on_a_monitor_that_is_still_there_opens_exactly_where_it_was() {
        let monitors = [monitor([0, 0]), monitor([1920, 0])];
        assert_eq!(
            onto_a_monitor([2100, 140], SIZE, &monitors),
            Some(([2100, 140], 1))
        );
        assert_eq!(onto_a_monitor([0, 0], SIZE, &monitors), Some(([0, 0], 0)));
    }

    #[test]
    fn a_window_whose_monitor_was_unplugged_comes_back_whole_on_one_that_is_there() {
        // It was on the right-hand monitor, and only the laptop's own screen
        // is left: a window opened at x = 2100 would be one nobody can see,
        // reach or close.
        let monitors = [monitor([0, 0])];
        let (position, index) =
            onto_a_monitor([2100, 140], SIZE, &monitors).expect("a monitor to open on");
        assert_eq!(index, 0);
        assert_eq!(position, [1920 - 800, 140], "moved by as little as fits it");

        // Below and to the left of every monitor there is.
        let (position, _) =
            onto_a_monitor([-5000, 4000], SIZE, &monitors).expect("a monitor to open on");
        assert_eq!(position, [0, 1080 - 600]);
    }

    #[test]
    fn a_window_left_hanging_off_an_edge_stays_while_its_top_can_be_dragged() {
        let monitors = [monitor([0, 0])];
        // Mostly past the right edge and the bottom, with a strip of the top
        // still on screen to take hold of.
        assert_eq!(
            onto_a_monitor([1800, 900], SIZE, &monitors),
            Some(([1800, 900], 0))
        );
    }

    #[test]
    fn a_window_whose_top_is_off_the_monitor_is_brought_down_onto_it() {
        // Its title bar above the top of the screen is a window that cannot
        // be dragged anywhere, however much of the rest shows.
        let monitors = [monitor([0, 0])];
        assert_eq!(
            onto_a_monitor([200, -300], SIZE, &monitors),
            Some(([200, 0], 0))
        );
        // And a sliver along the bottom is not enough to take hold of.
        assert_eq!(
            onto_a_monitor([200, 1060], SIZE, &monitors),
            Some(([200, 1080 - 600], 0))
        );
    }

    #[test]
    fn a_window_bigger_than_the_monitor_opens_in_its_top_left_corner() {
        let monitors = [monitor([0, 0])];
        assert_eq!(
            onto_a_monitor([3000, 3000], vec2f(2500., 1500.), &monitors),
            Some(([0, 0], 0))
        );
    }

    #[test]
    fn a_window_off_every_monitor_goes_to_the_nearest() {
        // Monitors side by side; a position far to the right is nearest the
        // right-hand one, even though the left-hand one is listed first.
        let monitors = [monitor([0, 0]), monitor([1920, 0])];
        let (position, index) =
            onto_a_monitor([9000, 100], SIZE, &monitors).expect("a monitor to open on");
        assert_eq!(index, 1);
        assert_eq!(position, [1920 + 1920 - 800, 100]);
    }

    #[test]
    fn a_monitor_of_another_scale_measures_the_window_in_its_own_pixels() {
        // 800 logical pixels are 1600 of a monitor at twice the scale. A
        // window brought down from above the top of it is fitted inside it
        // as well, and at x = 500 it would run 180 of those pixels past the
        // right-hand edge, which counting it as 800 would never notice.
        let retina = Monitor {
            origin: [0, 0],
            size: [1920, 1080],
            scale: 2.,
        };
        let (position, _) =
            onto_a_monitor([500, -50], SIZE, &[retina]).expect("a monitor to open on");
        assert_eq!(position, [1920 - 1600, 0]);
    }

    #[test]
    fn with_no_monitors_to_go_by_the_desktop_places_the_window() {
        assert_eq!(onto_a_monitor([100, 100], SIZE, &[]), None);
    }

    /// A window seen at `position` and `size`, neither maximised, fullscreen
    /// nor minimised.
    fn seen(position: [i32; 2], size: Vector2F) -> Sighting {
        Sighting {
            position: Some(position),
            size,
            ..Sighting::default()
        }
    }

    #[test]
    fn a_window_in_its_ordinary_state_is_kept_where_it_is() {
        let placement = WindowPlacement::default().after(seen([300, 200], SIZE));
        assert_eq!(
            placement,
            WindowPlacement {
                position: Some([300, 200]),
                size: SIZE,
                maximized: false,
                fullscreen: false,
            }
        );
    }

    #[test]
    fn a_maximised_window_keeps_the_size_and_place_it_goes_back_to() {
        let before = WindowPlacement::default().after(seen([300, 200], SIZE));
        let maximised = before.after(Sighting {
            maximized: true,
            ..seen([0, 0], vec2f(1920., 1050.))
        });
        assert_eq!(
            maximised,
            WindowPlacement {
                maximized: true,
                ..before
            },
            "the work area's frame was taken for the window's own"
        );

        let fullscreen = before.after(Sighting {
            fullscreen: true,
            ..seen([0, 0], vec2f(1920., 1080.))
        });
        assert_eq!(
            fullscreen,
            WindowPlacement {
                fullscreen: true,
                ..before
            }
        );

        // Restored, it is where it is again.
        assert_eq!(maximised.after(seen([300, 200], SIZE)), before);
    }

    #[test]
    fn a_minimised_window_changes_nothing() {
        let before = WindowPlacement::default().after(seen([300, 200], SIZE));
        let minimised = Sighting {
            minimized: true,
            ..seen([-32000, -32000], vec2f(0., 0.))
        };
        assert_eq!(before.after(minimised), before);
        // Windows says so with the size alone on the way down.
        assert_eq!(before.after(seen([-32000, -32000], vec2f(0., 0.))), before);
    }

    #[test]
    fn a_maximise_whose_move_comes_before_its_state_keeps_the_size_it_goes_back_to() {
        // Windows' Win+Up, Aero Snap to the top edge, or a maximised window
        // brought back from the taskbar: the move arrives with the frame
        // already the work area's and the window not yet said to be
        // maximised, and the resize after it says so. Both in one batch.
        let before = WindowPlacement::default().after(seen([300, 200], SIZE));
        let moved = seen([0, 0], vec2f(1920., 1050.));
        let resized = Sighting {
            maximized: true,
            ..moved
        };
        let maximised = WindowPlacement {
            maximized: true,
            ..before
        };
        assert_ne!(
            before.after(moved).after(resized),
            maximised,
            "looked at on each event, the work area is kept as the window's own size"
        );

        // Two events, one look, at the window as the last of them left it.
        // That the look waits for the end of the batch at all is `App`'s:
        // `Moved` and `Resized` only stir it, and `about_to_wait` settles.
        let mut following = Following::new(before);
        following.stirred();
        following.stirred();
        let mut looks = 0;
        let settled = following.settle(|| {
            looks += 1;
            resized
        });
        assert_eq!((settled, looks), (Some(maximised), 1));
        assert_eq!(following.placement(), maximised);
    }

    #[test]
    fn a_batch_that_said_nothing_of_the_window_does_not_look_at_it() {
        let before = WindowPlacement::default().after(seen([300, 200], SIZE));
        let mut following = Following::new(before);
        assert_eq!(
            following.settle(|| panic!("looked at with nothing said")),
            None
        );

        // A look taken anyway answers what was said before it.
        following.stirred();
        assert_eq!(
            following.see(seen([310, 200], SIZE)).map(|p| p.position),
            Some(Some([310, 200]))
        );
        assert_eq!(
            following.settle(|| panic!("looked at twice for one move")),
            None
        );
        // And a look that finds nothing new has nothing to tell.
        following.stirred();
        assert_eq!(following.settle(|| seen([310, 200], SIZE)), None);
    }

    #[test]
    fn a_window_that_only_covers_a_monitor_keeps_its_size_but_is_not_kept_fullscreen() {
        // X11, where covering a monitor is all a fullscreen window shows, and
        // all a lone tiled window with no bar shows too. Kept as fullscreen,
        // the tiled one would open fullscreen at the next launch on Wayland.
        let before = WindowPlacement::default().after(seen([300, 200], SIZE));
        let covering = Sighting {
            covers_a_monitor: true,
            ..seen([1920, 0], vec2f(1920., 1080.))
        };
        assert_eq!(before.after(covering), before);

        // And left, it is where it is again.
        let left = before.after(covering).after(seen([400, 250], SIZE));
        assert_eq!(left.position, Some([400, 250]));
        assert!(!left.fullscreen);
    }

    #[test]
    fn only_a_window_covering_a_whole_monitor_counts_as_covering_one() {
        let monitors = [monitor([0, 0]), monitor([1920, 0])];
        assert!(fills_a_monitor([1920, 0], [1920, 1080], &monitors));
        // Maximised under a panel.
        assert!(!fills_a_monitor([0, 32], [1920, 1048], &monitors));
        // The size of a monitor, across two of them.
        assert!(!fills_a_monitor([960, 0], [1920, 1080], &monitors));
        assert!(!fills_a_monitor([0, 0], [1920, 1080], &[]));
    }

    #[test]
    fn a_refused_fullscreen_is_asked_for_again_until_the_window_has_it() {
        let opened = Instant::now();
        let mut asked = FullscreenAsked::new(opened);
        // Too soon to look: a refusal comes after the request, not with it.
        assert_eq!(
            asked.step(opened, false, false),
            FullscreenStep::LookAt(opened + ASK_AGAIN_AFTER)
        );
        // Refused: winit has forgotten it.
        let first = opened + ASK_AGAIN_AFTER;
        assert_eq!(
            asked.step(first, false, false),
            FullscreenStep::AskAgain(first + ASK_AGAIN_AFTER)
        );
        // On its way in: wait for it.
        let second = first + ASK_AGAIN_AFTER;
        assert_eq!(
            asked.step(second, true, false),
            FullscreenStep::LookAt(second + ASK_AGAIN_AFTER)
        );
        // In.
        assert_eq!(
            asked.step(second + ASK_AGAIN_AFTER, true, true),
            FullscreenStep::Over
        );
    }

    #[test]
    fn a_fullscreen_winit_reports_at_once_is_still_asked_for_again_when_refused() {
        // winit takes the window as fullscreen from the moment it asks, and a
        // window opened maximised already spans its monitor's width: neither
        // means macOS has said yes. Only AppKit's word does, and until it
        // comes a refusal is asked for again.
        let opened = Instant::now();
        let mut asked = FullscreenAsked::new(opened);
        assert_eq!(
            asked.step(opened, true, false),
            FullscreenStep::LookAt(opened + ASK_AGAIN_AFTER)
        );
        let first = opened + ASK_AGAIN_AFTER;
        assert_eq!(
            asked.step(first, true, false),
            FullscreenStep::LookAt(first + ASK_AGAIN_AFTER)
        );
        // Refused after all: winit has forgotten it.
        let second = first + ASK_AGAIN_AFTER;
        assert_eq!(
            asked.step(second, false, false),
            FullscreenStep::AskAgain(second + ASK_AGAIN_AFTER)
        );
    }

    #[test]
    fn a_fullscreen_left_once_entered_is_not_asked_for_again() {
        // In, and taken out by the person before the next look: winit has
        // forgotten it as it does for a refusal, and AppKit's word is what
        // says it was not one.
        let opened = Instant::now();
        let mut asked = FullscreenAsked::new(opened);
        assert_eq!(
            asked.step(opened + ASK_AGAIN_AFTER, false, true),
            FullscreenStep::Over
        );
    }

    #[test]
    fn a_fullscreen_refused_for_seconds_is_not_asked_for_any_more() {
        let opened = Instant::now();
        let mut asked = FullscreenAsked::new(opened);
        assert_eq!(
            asked.step(opened + STOP_ASKING_AFTER, false, false),
            FullscreenStep::Over
        );
    }

    #[test]
    fn a_desktop_that_does_not_say_where_the_window_is_keeps_no_position() {
        // Wayland: a position from a session another desktop wrote is not
        // carried on as though this one had said it.
        let before = WindowPlacement {
            position: Some([300, 200]),
            size: SIZE,
            ..WindowPlacement::default()
        };
        let placement = before.after(Sighting {
            position: None,
            ..seen([0, 0], SIZE)
        });
        assert_eq!(placement.position, None);
    }
}
