//! Demand-driven X11 frames.
//!
//! A window's refresh timer parks when a tick leaves no frame requested and is
//! started again by the next request, so a static window costs no wakeups.

use calloop::ping::Ping;
use std::{cell::Cell, rc::Rc};

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
