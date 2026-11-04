//! Demand-driven X11 frames.
//!
//! A window's refresh timer parks when a tick leaves no frame requested and is
//! started again by the next request, so a static window costs no wakeups.

use crate::linux::{RefreshState, X11ClientState};
use calloop::ping::Ping;
use std::{cell::Cell, rc::Rc, time::Duration};
use x11rb::protocol::xproto;

pub(crate) struct FrameDemand {
    requested: Cell<bool>,
    parked: Cell<bool>,
    ping: Ping,
}

impl FrameDemand {
    pub(crate) fn new(ping: Ping) -> Rc<Self> {
        Rc::new(Self {
            requested: Cell::new(true),
            parked: Cell::new(false),
            ping,
        })
    }

    pub(crate) fn request(&self) {
        self.requested.set(true);
        if self.parked.get() {
            self.ping.ping();
        }
    }

    pub(crate) fn begin_tick(&self) {
        self.requested.set(false);
    }

    /// Parks the window unless the tick left a frame requested.
    pub(crate) fn park_if_idle(&self) -> bool {
        let idle = !self.requested.get();
        self.parked.set(idle);
        idle
    }

    pub(crate) fn needs_restart(&self) -> bool {
        self.parked.get() && self.requested.get()
    }

    pub(crate) fn resumed(&self) {
        self.parked.set(false);
    }

    pub(crate) fn hidden(&self) {
        self.parked.set(false);
        self.requested.set(true);
    }
}

pub(crate) fn waker(demand: &Rc<FrameDemand>) -> Rc<dyn Fn()> {
    let demand = demand.clone();
    Rc::new(move || demand.request())
}

pub(crate) fn park_refresh_loop(
    client: &mut X11ClientState,
    x_window: xproto::Window,
    refresh_rate: Duration,
) -> bool {
    let Some(window) = client.windows.get_mut(&x_window) else {
        return false;
    };
    if !window.frame.park_if_idle() {
        return false;
    }
    window.refresh_state = Some(RefreshState::Parked { refresh_rate });
    true
}

pub(crate) fn restart_parked_refresh_loops(client: &mut X11ClientState) {
    let restart = client
        .windows
        .iter()
        .filter(|(_, window)| window.frame.needs_restart())
        .filter_map(|(x_window, window)| match window.refresh_state {
            Some(RefreshState::Parked { refresh_rate }) => Some((*x_window, refresh_rate)),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (x_window, refresh_rate) in restart {
        let event_loop_token = client.start_refresh_loop(x_window, refresh_rate);
        if let Some(window) = client.windows.get_mut(&x_window) {
            window.frame.resumed();
            window.refresh_state = Some(RefreshState::PeriodicRefresh {
                refresh_rate,
                event_loop_token,
            });
        }
    }
}
