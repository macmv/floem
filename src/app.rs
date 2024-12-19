use std::{cell::RefCell, sync::Arc};

use crossbeam_channel::{Receiver, Sender};
use floem_reactive::WriteSignal;
use parking_lot::Mutex;
use raw_window_handle::HasDisplayHandle;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    window::WindowId,
};

use crate::{
    action::{Timer, TimerToken},
    app_handle::ApplicationHandle,
    clipboard::Clipboard,
    ext_event::ExtSendTrigger,
    inspector::Capture,
    profiler::Profile,
    view::{IntoView, View},
    window::WindowConfig,
    AnyView,
};

type AppEventCallback = dyn Fn(AppEvent);

static EVENT_LOOP_PROXY: Mutex<Option<(EventLoopProxy, Sender<UserEvent>)>> = Mutex::new(None);

thread_local! {
    pub(crate) static APP_UPDATE_EVENTS: RefCell<Vec<AppUpdateEvent>> = Default::default();
}

/// Initializes and runs an application with a single window.
///
/// This function creates a new `Application`, sets up a window with the provided view,
/// and starts the application event loop. The `app_view` closure is used to define
/// the root view of the application window.
///
/// Example:
/// ```no_run
/// floem::launch(|| "Hello, World!")
/// ```
///
/// To build an application and windows with more configuration, see [`Application`].
pub fn launch<V: IntoView + 'static>(app_view: impl FnOnce() -> V + 'static) {
    Application::new().window(move |_| app_view(), None).run()
}

pub enum AppEvent {
    WillTerminate,
    Reopen { has_visible_windows: bool },
}

pub(crate) enum UserEvent {
    AppUpdate(AppUpdateEvent),
    Idle(ExtSendTrigger),
    QuitApp,
    GpuResourcesUpdate { window_id: WindowId },
}

pub(crate) enum AppUpdateEvent {
    NewWindow {
        view_fn: Box<dyn FnOnce(WindowId) -> Box<dyn View> + Send + Sync>,
        config: Option<WindowConfig>,
    },
    CloseWindow {
        window_id: WindowId,
    },
    CaptureWindow {
        window_id: WindowId,
        capture: WriteSignal<Option<Arc<Capture>>>,
    },
    ProfileWindow {
        window_id: WindowId,
        end_profile: Option<WriteSignal<Option<Arc<Profile>>>>,
    },
    RequestTimer {
        timer: Timer,
    },
    CancelTimer {
        timer: TimerToken,
    },
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    MenuAction {
        window_id: WindowId,
        action_id: usize,
    },
}

pub(crate) fn add_app_update_event(event: AppUpdateEvent) {
    Application::send_proxy_event(UserEvent::AppUpdate(event));
}

/// Floem top level application
/// This is the entry point of the application.
pub struct Application {
    receiver: Receiver<UserEvent>,
    handle: ApplicationHandle,
    event_listener: Option<Box<AppEventCallback>>,
    event_loop: Option<EventLoop>,
    initial_windows: Vec<(Box<dyn FnOnce(WindowId) -> AnyView>, Option<WindowConfig>)>,
}

impl Default for Application {
    fn default() -> Self {
        Self::new()
    }
}

impl ApplicationHandler for Application {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        println!("can create surfaces");
        while let Some((view_fn, window_config)) = self.initial_windows.pop() {
            self.handle
                .new_window(event_loop, view_fn, window_config.unwrap_or_default());
        }
        println!("window creation done");
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        println!("window event {event:?}");
        self.handle.handle_timer(event_loop);
        self.handle
            .handle_window_event(window_id, event, event_loop);
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        println!("proxy wake up");
        self.handle.handle_timer(event_loop);
        for event in self.receiver.try_iter() {
            self.handle.handle_user_event(event_loop, event);
        }
        self.handle.handle_updates_for_all_windows();
    }

    fn exiting(&mut self, _event_loop: &dyn ActiveEventLoop) {
        if let Some(action) = self.event_listener.as_ref() {
            action(AppEvent::WillTerminate);
        }
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.handle.handle_timer(event_loop);
    }
}

impl Application {
    pub fn new() -> Self {
        let event_loop = EventLoop::new().expect("can't start the event loop");
        let event_loop_proxy = event_loop.create_proxy();
        let (sender, receiver) = crossbeam_channel::unbounded();
        *EVENT_LOOP_PROXY.lock() = Some((event_loop_proxy.clone(), sender));
        unsafe {
            Clipboard::init(event_loop.display_handle().unwrap().as_raw());
        }
        let handle = ApplicationHandle::new();
        Self {
            receiver,
            handle,
            event_listener: None,
            event_loop: Some(event_loop),
            initial_windows: Vec::new(),
        }
    }

    pub fn on_event(mut self, action: impl Fn(AppEvent) + 'static) -> Self {
        self.event_listener = Some(Box::new(action));
        self
    }

    /// Create a new window for the application, if you want multiple windows,
    /// just chain more window method to the builder.
    ///
    /// # Note
    ///
    /// Using `None` as a configuration argument is equivalent to using
    /// `WindowConfig::default()`.
    pub fn window<V: IntoView + 'static>(
        mut self,
        app_view: impl FnOnce(WindowId) -> V + 'static,
        config: Option<WindowConfig>,
    ) -> Self {
        self.initial_windows.push((
            Box::new(move |window_id: WindowId| app_view(window_id).into_any()),
            config,
        ));
        self
    }

    pub fn run(mut self) {
        let event_loop = self.event_loop.take().unwrap();
        println!("now run app");
        event_loop.run_app(self);
    }

    pub(crate) fn send_proxy_event(event: UserEvent) {
        if let Some((proxy, sender)) = EVENT_LOOP_PROXY.lock().as_ref() {
            let _ = sender.send(event);
            proxy.wake_up();
        }
    }

    // pub fn available_monitors(&self) -> impl Iterator<Item = MonitorHandle> {
    //     self.event_loop.as_ref().unwrap().available_monitors()
    // }

    // pub fn primary_monitor(&self) -> Option<MonitorHandle> {
    //     self.event_loop.as_ref().unwrap().primary_monitor()
    // }
}

/// Initiates the application shutdown process.
///
/// This function sends a `QuitApp` event to the application's event loop,
/// triggering the application to close gracefully.
pub fn quit_app() {
    Application::send_proxy_event(UserEvent::QuitApp);
}
