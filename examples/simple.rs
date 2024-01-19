#![feature(if_let_guard)]

use crossterm::event::{Event as CTEvent, KeyCode};
use gland::{
    forward_handle_event, id, Component, Compositor, Context, Event, EventAccess, Id, LayerId,
};
use ratatui::{
    prelude::{Buffer, CrosstermBackend, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Widget},
};
use std::{
    error::Error,
    io,
    time::{Duration, Instant},
};
use tokio::time::sleep;

struct AppState {
    text: String,
    start: Instant,
}

struct MainScreen {
    counter: u32,
    input: Input,
}

impl Component<AppState> for MainScreen {
    fn id(&self) -> Id {
        Id::new("main")
    }

    fn view(&self, area: Rect, buf: &mut Buffer, state: &AppState) {
        let mut x = area.width / 2;
        let y = area.height / 2;

        let text = format!(
            "Counter: {} Passed: {}",
            self.counter,
            state.start.elapsed().as_secs()
        );
        x -= text.len() as u16 / 2;

        self.input.view(Rect { y: y - 1, ..area }, buf, state);
        buf.set_string(x, y, text, Style::new());
    }

    fn handle_event(&mut self, event: &mut EventAccess, cx: &mut Context<AppState>) {
        forward_handle_event!(event, cx, self.input);

        if let Event::Terminal(CTEvent::Key(ke)) = event.peek() {
            match ke.code {
                KeyCode::Esc => cx.add_callback(|cc| cc.exit()),
                KeyCode::Tab => {
                    let id = self.id();
                    cx.add_callback(move |cc| {
                        let screen = cc.get_at::<MainScreen>(LayerId::FOREGROUND, id).unwrap();
                        cc.replace_at(
                            LayerId::POPUP,
                            Popup {
                                title_counter: screen.counter,
                                ..Default::default()
                            },
                        );
                    });
                }
                KeyCode::Enter => {
                    self.counter += 1;

                    if self.counter == 10 {
                        cx.add_callback(|cc| cc.exit());
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Default)]
struct Popup {
    title_counter: u32,
    text: String,
}

impl<S: 'static> Component<S> for Popup {
    fn id(&self) -> Id {
        Id::new("popup")
    }

    fn view(&self, area: Rect, buf: &mut Buffer, _: &S) {
        let area = Rect {
            x: area.width / 3,
            y: area.height / 4,
            width: area.width / 3,
            height: area.height / 8,
        };

        Clear.render(area, buf);
        let block = Block::new()
            .title(format!(
                "Popup: {} (value returned by downcasting)",
                self.title_counter
            ))
            .borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);

        buf.set_string(inner.x, inner.y, &self.text, Style::default());
    }

    fn handle_event(&mut self, event: &mut EventAccess, cx: &mut Context<S>) {
        match event.peek() {
            Event::Terminal(CTEvent::Key(ke)) if ke.code == KeyCode::Esc => {
                let id = id!(S, self);
                cx.add_callback(move |cc| cc.remove_all(id));
                event.consume();
            }
            Event::Terminal(CTEvent::Key(ke)) if let KeyCode::Char(ref c) = ke.code => {
                self.text.push(*c);
                // If you completes text to `clear` then we clear the text after 1 second.

                if self.text.ends_with("clear") {
                    let id = id!(self);
                    cx.jobs().spawn(async move {
                        sleep(Duration::from_secs(1)).await;

                        move |cc: &mut Compositor<S>| {
                            cc.get_mut_at::<Self>(LayerId::POPUP, id)
                                .unwrap()
                                .text
                                .clear();
                        }
                    });
                }

                event.consume();
            }
            Event::Terminal(CTEvent::Key(ke)) if matches!(ke.code, KeyCode::Backspace) => {
                self.text.pop();
                event.consume();
            }
            _ => {}
        }
    }
}

struct Input;
impl Component<AppState> for Input {
    fn id(&self) -> Id {
        Id::new("input")
    }

    fn view(&self, area: Rect, buf: &mut Buffer, state: &AppState) {
        let x = area.width / 2 - state.text.len() as u16 / 2;
        buf.set_string(
            x,
            area.y,
            &state.text,
            Style::default().bg(Color::Green).fg(Color::Black),
        );
    }

    fn handle_event(&mut self, event: &mut EventAccess, cx: &mut Context<AppState>) {
        if let Event::Terminal(CTEvent::Key(ke)) = event.peek() {
            match ke.code {
                KeyCode::Char(ch) => {
                    cx.state_mut().text.push(ch);
                    event.consume();
                }
                KeyCode::Backspace if !cx.state().text.is_empty() => {
                    cx.state_mut().text.pop();
                    event.consume();
                }
                _ => (),
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut comp: Compositor<AppState> = Compositor::with_state(AppState {
        text: "Write to modify the text, press enter to increment".to_owned(),
        start: Instant::now(),
    })
    .with_event_stream();

    comp.replace_at(
        LayerId::FOREGROUND,
        MainScreen {
            input: Input,
            counter: 0,
        },
    );
    comp.run(CrosstermBackend::new(io::stdout())).await?;

    Ok(())
}
