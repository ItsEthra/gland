use crate::{Component, Event, EventAccess, Id, LayerId};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use futures_util::{stream::select_all, Stream, StreamExt};
use ratatui::{
    backend::Backend,
    prelude::{Buffer, Rect},
    widgets::Widget,
    Terminal,
};
use std::{collections::BTreeMap, io, mem::take, pin::Pin, time::Duration};
use tokio::{sync::mpsc::Receiver, time::interval};
use tokio_stream::wrappers::{IntervalStream, ReceiverStream};

type Callback<'comp, S, E> = Box<dyn FnOnce(&mut Compositor<'comp, S, E>) + 'comp>;

/// Context of the current update.
pub struct Context<'comp, S: 'comp = (), E: 'comp = ()> {
    callbacks: Vec<Callback<'comp, S, E>>,
    state: S,
}

impl<'comp, S: 'comp, E: 'comp> Context<'comp, S, E> {
    /// Adds a callback that will be executed after all components have been drawn in this frame.
    pub fn add_callback(&mut self, func: impl FnOnce(&mut Compositor<'comp, S, E>) + 'comp) {
        self.callbacks.push(Box::new(func))
    }

    /// Returns immutable ref to the compositor state.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Returns mutable ref to the compositor state.
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }

    fn new(state: S) -> Self {
        Self {
            callbacks: Vec::with_capacity(8),
            state,
        }
    }
}

pub struct Compositor<'comp, S: 'comp = (), E: 'comp = ()> {
    layers: BTreeMap<LayerId, Vec<Box<dyn Component<'comp, S, E> + 'comp>>>,
    state: S,

    streams: Vec<Pin<Box<dyn Stream<Item = Event<E>> + 'comp>>>,
    timeout: Duration,

    exit: bool,
}

impl<'comp, E: 'comp> Compositor<'comp, (), E> {
    /// Creates new compositor without state.
    pub fn new() -> Self {
        Self::with_state(())
    }
}

impl<'comp, S: 'comp, E: 'comp> Compositor<'comp, S, E> {
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

    /// Replaces component or adds new one at some layer.
    pub fn replace_at<T: Component<'comp, S, E> + 'comp>(
        &mut self,
        layer_id: LayerId,
        component: T,
    ) {
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

    /// Removes component at a layer, returning `true` if the component was removed.
    pub fn remove_at(&mut self, layer_id: LayerId, component_id: Id) -> bool {
        self.layers
            .get_mut(&layer_id)
            .unwrap()
            .retain(|c| c.id() != component_id);
        true
    }

    /// Adds event wait timeout, when `timeout` passes, new `Event::Tick` is generated and ui is re-rendered.
    /// Default is 3 seconds. To disable periodic ui updates set this to `Duration::ZERO`.
    pub fn set_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.timeout = timeout;
        self
    }

    /// Adds new stream of events, UI is re-rendered when event is received.
    pub fn with_stream(&mut self, stream: impl Stream<Item = Event<E>> + 'comp) -> &mut Self {
        self.streams.push(Box::pin(stream.map(Into::into)));
        self
    }

    /// Adds new stream built from the receiver.
    pub fn with_receiver_stream(&mut self, recv: Receiver<E>) -> &mut Self {
        self.with_stream(ReceiverStream::new(recv).map(Event::Custom));
        self
    }

    /// Adds new stream created from terminal event.
    #[cfg(feature = "event-stream")]
    pub fn with_event_stream(&mut self) -> &mut Self {
        use crossterm::event::EventStream;

        let stream = EventStream::new()
            .map(|x| x.expect("failed to receive a terminal event"))
            .map(Event::Terminal);
        self.with_stream(stream);
        self
    }

    /// Returns state of the compositor immutably.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Returns state of the compositor mutably.
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }

    pub fn exit(&mut self) {
        self.exit = true;
    }

    pub async fn run<B: Backend>(mut self, backend: B) -> io::Result<()> {
        let guard = TerminalGuard::new()?;

        if !self.timeout.is_zero() {
            self.with_stream(IntervalStream::new(interval(self.timeout)).map(|_| Event::Tick));
        }

        let mut flux = select_all(take(&mut self.streams));
        let mut terminal = Terminal::new(backend)?;

        while let Some(event) = flux.next().await {
            if matches!(event, Event::Exit) {
                break;
            }

            // Pass event to all components.
            let mut cx: Context<'comp, S, E> = Context::new(self.state);
            let mut access: EventAccess<E> = EventAccess::new(event);

            // Iterate from top to bottom, break if event is consumed.
            'outer: for layer in self.layers.values_mut().rev() {
                for component in layer.iter_mut() {
                    component.handle_event(&mut access, &mut cx);

                    if access.is_consumed() {
                        break 'outer;
                    }
                }
            }

            let Context { callbacks, state } = cx;
            self.state = state;
            callbacks.into_iter().for_each(|cc| cc(&mut self));

            if self.exit {
                break;
            }

            terminal
                .draw(|f| {
                    self.layers
                        .values()
                        .flat_map(|l| l.iter())
                        .filter(|c| c.should_update(&self.state))
                        .for_each(|c| {
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

        drop(guard);
        Ok(())
    }
}

impl<'comp, S: 'comp + Default, E: 'comp> Default for Compositor<'comp, S, E> {
    #[inline]
    fn default() -> Self {
        Self::with_state(S::default())
    }
}

struct ComponentWidget<'r, 'comp, S: 'comp, E: 'comp> {
    component: &'r (dyn Component<'comp, S, E> + 'comp),
    state: &'r S,
}

impl<'r, 'comp, S: 'comp, E: 'comp> Widget for ComponentWidget<'r, 'comp, S, E> {
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
