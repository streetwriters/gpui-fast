//! What the test platform renders with. It lives apart from `test_rendering`
//! because benchmark builds (`bench-support`) give their windows a renderer
//! too, but have no `TestAppContext` for `test_rendering` to extend.

use std::{cell::RefCell, sync::Arc};

use anyhow::Result;

use crate::{PlatformHeadlessRenderer, PlatformTextSystem};

pub(crate) type RendererFactory =
    Box<dyn Fn() -> Result<Option<Box<dyn PlatformHeadlessRenderer>>>>;

/// What the test platform renders with: the offscreen renderer its windows
/// get (given when it is built, or by `TestAppContext::enable_rendering`)
/// and the text system a test swapped in.
pub(crate) struct Installed {
    pub(crate) text_system: RefCell<Option<Arc<dyn PlatformTextSystem>>>,
    pub(crate) renderer_factory: RefCell<Option<RendererFactory>>,
}

impl Installed {
    pub(crate) fn new(renderer_factory: Option<RendererFactory>) -> Self {
        Self {
            text_system: RefCell::new(None),
            renderer_factory: RefCell::new(renderer_factory),
        }
    }

    pub(crate) fn text_system(installed: &Self) -> Option<Arc<dyn PlatformTextSystem>> {
        installed.text_system.borrow().clone()
    }

    pub(crate) fn renderer(
        installed: &Self,
    ) -> Result<Option<Box<dyn PlatformHeadlessRenderer>>> {
        match installed.renderer_factory.borrow().as_ref() {
            Some(factory) => factory(),
            None => Ok(None),
        }
    }
}
