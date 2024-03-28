use crate::{Component, Event, EventAccess, Id, Jobs, LayerId};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use futures_util::{
    stream::{self, select_all},
    Stream, StreamExt,
};
use ratatui::{
    backend::Backend,
    prelude::{Buffer, Rect},
    widgets::Widget,
    Terminal,
};
use std::{
    any::Any,
    collections::BTreeMap,
    future::Future,
    io,
    mem::{take, transmute},
    pin::Pin,
    time::Duration,
};
use tokio::{
    sync::mpsc::{self, Receiver},
    task::LocalSet,
    time::interval,
};
use tokio_stream::wrappers::{IntervalStream, ReceiverStream};

pub type Callback<S, E> = Box<dyn FnOnce(&mut Compositor<S, E>)>;

/// Context of the current update.
pub struct Context<'comp, S = (), E = ()> {
    callbacks: Vec<Callback<S, E>>,
    jobs: &'comp Jobs<'comp, S, E>,
    size: Rect,
    state: S,
}

impl<'comp, S: 'static, E: 'static> Context<'comp, S, E> {
    pub fn jobs(&self) -> &'comp Jobs<S, E> {
        self.jobs
    }

    /// Adds a callback that will be executed after all components have been drawn in this frame.
    pub fn add_callback(&mut self, func: impl FnOnce(&mut Compositor<S, E>) + 'static) {
        self.callbacks.push(Box::new(func))
    }

    /// Returns the size of the terminal in cells.
    pub fn size(&self) -> Rect {
        self.size
    }

    /// Returns an immutable reference to the compositor state.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Returns a mutable reference to the compositor state.
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }
}

#[non_exhaustive]
pub(crate) enum Resume<S, E> {
    Event(Event<E>),
    JobCallback(Callback<S, E>),
}

/// Main interface that draws components and dispatches events.
#[allow(clippy::type_complexity)]
pub struct Compositor<S = (), E = ()> {
    layers: BTreeMap<LayerId, Vec<Box<dyn Component<S, E>>>>,
    state: S,

    streams: Vec<Pin<Box<dyn Stream<Item = Resume<S, E>>>>>,
    timeout: Duration,

    exit: bool,
}

impl<E: 'static> Compositor<(), E> {
    /// Creates new compositor without state.
    pub fn new() -> Self {
        Self::with_state(())
    }
}

/// Non-builder functions
impl<S: 'static, E: 'static> Compositor<S, E> {
    /// Inserts a new component at a layer, if already exists returns it.
    pub fn insert_at<C: Component<S, E>>(
        &mut self,
        layer_id: LayerId,
        component: C,
    ) -> Result<(), C> {
        let layer = self.layers.entry(layer_id).or_default();

        if layer.iter().any(|c| c.id() == component.id()) {
            Err(component)
        } else {
            layer.push(Box::new(component));
            Ok(())
        }
    }

    /// Replaces component or adds new one at some layer.
    pub fn replace_at<C: Component<S, E>>(&mut self, layer_id: LayerId, component: C) {
        let layer = self.layers.entry(layer_id).or_default();
        layer.retain(|c| c.id() != component.id());
        layer.push(Box::new(component));
    }

    /// Removes all components with `component_id` on all layers.
    pub fn remove_all(&mut self, component_id: Id) {
        self.layers
            .values_mut()
            .for_each(|l| l.retain(|c| c.id() != component_id));
    }

    /// Downcasts mounted component and returns a reference to it.
    pub fn get_at<C: Component<S, E>>(&self, layer_id: LayerId, component_id: Id) -> Option<&C> {
        let dyncomp = &**self
            .layers
            .get(&layer_id)?
            .iter()
            .find(|c| c.id() == component_id)? as &dyn Any;
        dyncomp.downcast_ref::<C>()
    }

    /// Downcasts mounted component and returns a mutable reference to it.
    pub fn get_mut_at<C: Component<S, E>>(
        &mut self,
        layer_id: LayerId,
        component_id: Id,
    ) -> Option<&mut C> {
        let dyncomp = &mut **self
            .layers
            .get_mut(&layer_id)?
            .iter_mut()
            .find(|c| c.id() == component_id)? as &mut dyn Any;
        dyncomp.downcast_mut::<C>()
    }

    /// Unmounts a component and downcasts it.
    pub fn take_at<C: Component<S, E>>(
        &mut self,
        layer_id: LayerId,
        component_id: Id,
    ) -> Option<Box<C>> {
        let layer = self.layers.get_mut(&layer_id)?;
        let position = layer.iter().position(|c| c.id() == component_id)?;

        let dyncomp = layer.swap_remove(position) as Box<dyn Any>;
        match dyncomp.downcast::<C>() {
            Ok(comp) => Some(comp),
            Err(other) => {
                // SAFETY: It's the same component we casted above.
                let dyncomp = unsafe { transmute::<Box<dyn Any>, Box<dyn Component<S, E>>>(other) };
                layer.push(dyncomp);

                None
            }
        }
    }

    /// Removes component at a layer, returning `true` if the component was removed.
    pub fn remove_at(&mut self, layer_id: LayerId, component_id: Id) -> bool {
        self.layers
            .get_mut(&layer_id)
            .unwrap()
            .retain(|c| c.id() != component_id);
        true
    }

    /// Returns state of the compositor immutably.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Returns state of the compositor mutably.
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }

    /// Exit the compositor.
    pub fn exit(&mut self) {
        self.exit = true;
    }
}

/// Builder functions
impl<S: 'static, E: 'static> Compositor<S, E> {
    /// Creates new compositor with custom state.
    pub fn with_state(state: S) -> Self {
        Self {
            timeout: Duration::from_secs(3),
            layers: BTreeMap::new(),
            streams: Vec::new(),
            exit: false,
            state,
        }
    }

    /// Adds event wait timeout, when `timeout` passes, new `Event::Tick` is generated and ui is re-rendered.
    /// Default is 3 seconds. To disable periodic ui updates set this to `Duration::ZERO`.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Adds new stream of events, UI is re-rendered when event is received.
    pub fn with_stream(mut self, stream: impl Stream<Item = Event<E>> + 'static) -> Self {
        self.streams.push(Box::pin(stream.map(Resume::Event)));
        self
    }

    /// Adds new stream that emits user events built from the receiver.
    pub fn with_receiver_stream(self, recv: Receiver<E>) -> Self {
        self.with_stream(ReceiverStream::new(recv).map(Event::User))
    }

    /// Adds new stream created from terminal event.
    #[cfg(feature = "event-stream")]
    pub fn with_event_stream(self) -> Self {
        use crossterm::event::EventStream;

        let stream = EventStream::new()
            .map(|x| x.expect("failed to receive a terminal event"))
            .map(Event::Terminal);
        self.with_stream(stream)
    }

    /// Exit the compositor when this future resolves
    pub fn with_shutdown(self, shutdown: impl Future + 'static) -> Self {
        self.with_stream(stream::once(shutdown).map(|_| Event::Exit))
    }

    /// Begin polling events and draw ui. Exit after [`Event::Exit`] is emitted or [`Self::exit`] is called.
    pub async fn run<B: Backend>(mut self, backend: B) -> io::Result<()> {
        let guard = TerminalGuard::new()?;

        if !self.timeout.is_zero() {
            self.streams.push(Box::pin(
                IntervalStream::new(interval(self.timeout))
                    .map(|_| Event::Tick)
                    .map(Resume::Event),
            ));
        }

        // Tick once at the start to draw initial ui.
        self = self.with_stream(stream::iter([Event::Tick]));

        let (sender, rx) = mpsc::channel(12);
        self.streams.push(Box::pin(ReceiverStream::new(rx)));

        let set = LocalSet::new();
        let jobs = Jobs::<S, E>::new(&set, sender);

        let mut flux = select_all(take(&mut self.streams));
        let mut terminal = Terminal::new(backend)?;

        set.run_until(async move {
            while let Some(event) = flux.next().await {
                let event = match event {
                    Resume::Event(e) => e,
                    Resume::JobCallback(callback) => {
                        callback(&mut self);
                        Event::None
                    }
                };

                // Pass event to all components.
                let mut cx: Context<S, E> = Context {
                    callbacks: Vec::with_capacity(8),
                    size: terminal.size()?,
                    state: self.state,
                    jobs: &jobs,
                };
                let mut access: EventAccess<E> = EventAccess { event };

                // Iterate from top to bottom, break if event is consumed.
                'outer: for layer in self.layers.values_mut().rev() {
                    for component in layer.iter_mut() {
                        component.handle_event(&mut access, &mut cx);

                        if access.is_consumed() {
                            break 'outer;
                        }
                    }
                }

                let Context {
                    callbacks,
                    state,
                    size: _,
                    jobs: _,
                } = cx;
                self.state = state;
                callbacks.into_iter().for_each(|cc| cc(&mut self));

                if self.exit {
                    break;
                }

                terminal
                    .draw(|f| {
                        self.layers.values().flat_map(|l| l.iter()).for_each(|c| {
                            f.render_widget(
                                ComponentWidget {
                                    component: &**c,
                                    state: &self.state,
                                },
                                f.size(),
                            )
                        });
                    })
                    .unwrap();
            }

            io::Result::Ok(())
        })
        .await?;

        drop(guard);
        Ok(())
    }
}

impl<S: 'static + Default, E: 'static> Default for Compositor<S, E> {
    #[inline]
    fn default() -> Self {
        Self::with_state(S::default())
    }
}

struct ComponentWidget<'r, S, E> {
    component: &'r dyn Component<S, E>,
    state: &'r S,
}

impl<'r, S: 'static, E: 'static> Widget for ComponentWidget<'r, S, E> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.component.view(area, buf, self.state);
    }
}

struct TerminalGuard;
impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            // PushKeyboardEnhancementFlags(
            //     KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            //         | KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            // ),
            crossterm::terminal::Clear(ClearType::All)
        )?;

        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        _ = execute!(
            io::stdout(),
            // PopKeyboardEnhancementFlags,
            DisableMouseCapture,
            LeaveAlternateScreen,
        );
        _ = disable_raw_mode();
    }
}
