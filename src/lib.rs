use ratatui::prelude::{Buffer, Rect};
use std::{
    fmt,
    hash::{Hash, Hasher},
    mem::replace,
    num::NonZeroU64,
};
use twox_hash::XxHash64;

mod compositor;
pub use compositor::*;

/// LayerId describes elevation of the component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct LayerId(pub i16);

impl LayerId {
    /// Background.
    pub const BACKGROUND: Self = Self(-100);
    /// Foreground.
    pub const FOREGROUND: Self = Self(100);
    /// Popup.
    pub const POPUP: Self = Self(200);
    /// Topmost.
    pub const TOPMOST: Self = Self(500);
}

/// Id of the component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id(NonZeroU64);

impl Id {
    /// Creates new Id from hashable.
    pub fn new(source: impl Hash) -> Self {
        let mut hasher = XxHash64::default();
        source.hash(&mut hasher);

        NonZeroU64::new(hasher.finish()).map(Self).expect("Id is 0")
    }

    /// Combines id with another source of randomness.
    pub fn with(&self, more: impl Hash) -> Self {
        let mut hasher = XxHash64::default();
        self.0.hash(&mut hasher);
        more.hash(&mut hasher);

        NonZeroU64::new(hasher.finish()).map(Self).expect("Id is 0")
    }
}

/// Event that can occur during runtime.
#[non_exhaustive]
pub enum Event<T> {
    /// Custom event
    Custom(T),
    /// Event from the terminal
    Terminal(crossterm::event::Event),
    /// Next tick occured without intermediate event
    Tick,
    /// Exists compositor when emitted
    Exit,
    /// No event, used as a placeholder when event was taken
    None,
}

impl<T> Event<T> {
    /// Checks if event is custom.
    #[inline]
    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// Converts into custom event ref on success.
    #[inline]
    pub fn as_custom(&self) -> Option<&T> {
        match self {
            Event::Custom(e) => Some(e),
            _ => None,
        }
    }

    /// Converts into custom event mut ref on success.
    #[inline]
    pub fn as_mut_custom(&mut self) -> Option<&mut T> {
        match self {
            Event::Custom(e) => Some(e),
            _ => None,
        }
    }

    /// Converts into custom event on success.
    #[inline]
    pub fn into_custom(self) -> Result<T, Self> {
        match self {
            Event::Custom(e) => Ok(e),
            _ => Err(self),
        }
    }

    /// Checks if event is from terminal.
    #[inline]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal(_))
    }

    /// Converts into terminal event ref on success.
    #[inline]
    pub fn as_terminal(&self) -> Option<&crossterm::event::Event> {
        match self {
            Event::Terminal(e) => Some(e),
            _ => None,
        }
    }

    /// Converts into terminal event mut ref on success.
    #[inline]
    pub fn as_mut_terminal(&mut self) -> Option<&mut crossterm::event::Event> {
        match self {
            Event::Terminal(e) => Some(e),
            _ => None,
        }
    }

    /// Converts into terminal event on success.
    #[inline]
    pub fn into_terminal(self) -> Result<crossterm::event::Event, Self> {
        match self {
            Event::Terminal(e) => Ok(e),
            _ => Err(self),
        }
    }
}

impl<T: Clone> Clone for Event<T> {
    fn clone(&self) -> Self {
        match self {
            Event::Terminal(e) => Self::Terminal(e.clone()),
            Event::Custom(e) => Self::Custom(e.clone()),
            Event::Tick => Self::Tick,
            Event::Exit => Self::Exit,
            Event::None => Self::None,
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Event<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::Terminal(e) => f.debug_tuple("Crossterm").field(e).finish(),
            Event::Custom(e) => f.debug_tuple("Custom").field(e).finish(),
            Event::Tick => write!(f, "Tick"),
            Event::Exit => write!(f, "Exit"),
            Event::None => write!(f, "None"),
        }
    }
}

/// Provides access to the [`Event`], default result is `Ignored`
pub struct EventAccess<E = ()> {
    event: Event<E>,
}

impl<E> EventAccess<E> {
    pub(crate) fn new(event: Event<E>) -> Self {
        Self { event }
    }

    /// Peaks the event
    pub fn peak(&self) -> &Event<E> {
        &self.event
    }

    /// Consumes the event, sets old event to `None`
    pub fn consume(&mut self) -> Event<E> {
        replace(&mut self.event, Event::None)
    }

    /// Replaces old event with the one supplied, returns old event
    pub fn replace(&mut self, event: Event<E>) -> Event<E> {
        replace(&mut self.event, event)
    }

    /// Checks if event was consumed
    #[inline]
    pub fn is_consumed(&self) -> bool {
        matches!(self.event, Event::None)
    }
}

impl<E: Clone> EventAccess<E> {
    /// Clones the event, doesn't modify the result
    pub fn cloned(&self) -> Event<E> {
        self.event.clone()
    }
}

pub trait Component<'comp, S: 'comp = (), E: 'comp = ()> {
    fn id(&self) -> Id;
    fn view(&self, area: Rect, buf: &mut Buffer, state: &S);

    fn handle_event(&mut self, _event: &mut EventAccess<E>, _cx: &mut Context<'comp, S, E>) {}
    fn should_update(&self, _state: &S) -> bool {
        true
    }
}

/// Forwards `handle_event` to multiple child components.
#[macro_export]
macro_rules! forward_handle_event {
    (@ret $($tail:tt)*) => {
        if $crate::forward_handle_event!($($tail)*) {
            return;
        }
    };
    ($event:expr, $cx:expr, $($comp:expr),*) => {
        'forward: {
            $(
                $comp.handle_event($event, $cx);
                if $event.is_consumed() {
                    break 'forward true;
                }
            )*

            false
        }
    };
}

/// Forwards `view` to multiple child components.
#[macro_export]
macro_rules! forward_view {
    ($area:expr, $buf:expr, $state:expr, $($comp:expr),*) => {
        {
            let mut any = false;

            $(
                if $comp.should_update($state) {
                    $comp.view($area, $buf, $state);
                    any = true;
                }
            )*

            any
        }
    };
}
