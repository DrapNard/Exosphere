// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux window lifecycle. Rendering and guest input translation remain separate.

use std::sync::Arc;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

/// A renderer can create its surface from the live window on the first event.
/// Request another redraw only when a new frame is ready; the loop does not spin.
pub trait WindowClient {
    fn created(&mut self, _window: Arc<Window>) -> Result<(), String> {
        Ok(())
    }
    fn event(&mut self, window: &Window, event: &WindowEvent) -> Result<(), String>;
}

struct WindowApplication<C> {
    title: String,
    client: C,
    window: Option<Arc<Window>>,
    failure: Option<String>,
}

impl<C: WindowClient> ApplicationHandler for WindowApplication<C> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match event_loop.create_window(Window::default_attributes().with_title(&self.title)) {
            Ok(window) => {
                let window = Arc::new(window);
                window.request_redraw();
                self.window = Some(window.clone());
                if let Err(error) = self.client.created(window) {
                    self.failure = Some(error);
                    event_loop.exit();
                }
            }
            Err(error) => {
                self.failure = Some(format!("Linux window creation failed: {error}"));
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = &self.window else { return };
        if window.id() != window_id {
            return;
        }
        if let Err(error) = self.client.event(window, &event) {
            self.failure = Some(error);
            event_loop.exit();
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) => window.request_redraw(),
            _ => {}
        }
    }
}

/// Run on the Linux application's main thread. A missing display server is an
/// explicit error, not a successful headless presentation fallback.
pub fn run_window(title: impl Into<String>, client: impl WindowClient) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|error| format!("Linux event loop: {error}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut application = WindowApplication {
        title: title.into(),
        client,
        window: None,
        failure: None,
    };
    event_loop
        .run_app(&mut application)
        .map_err(|error| format!("Linux window loop: {error}"))?;
    application.failure.map_or(Ok(()), Err)
}
