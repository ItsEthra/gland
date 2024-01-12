use crossterm::event::{Event as CTEvent, KeyCode};
use gland::{
    forward_handle_event, Component, Compositor, Context, Event, EventAccess, Id, LayerId,
};
use ratatui::{
    prelude::{Buffer, CrosstermBackend, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Widget},
};
use std::{error::Error, io, time::Instant};

struct AppState {
    counter: i32,
    text: String,
    start: Instant,
}

struct MainScreen {
    input: Input,
}

impl<'comp> Component<'comp, AppState> for MainScreen {
    fn id(&self) -> Id {
        Id::new("stuff")
    }

    fn view(&self, area: Rect, buf: &mut Buffer, state: &AppState) {
        let mut x = area.width / 2;
        let y = area.height / 2;

        let text = format!(
            "Counter: {} Passed: {}",
            state.counter,
            state.start.elapsed().as_secs()
        );
        x -= text.len() as u16 / 2;

        self.input.view(Rect { y: y - 1, ..area }, buf, state);
        buf.set_string(x, y, text, Style::new());
    }

    fn handle_event(&mut self, event: &mut EventAccess, cx: &mut Context<'comp, AppState>) {
        forward_handle_event!(event, cx, self.input);

        if let Event::Terminal(CTEvent::Key(ke)) = event.peak() {
            match ke.code {
                KeyCode::Esc => cx.add_callback(|cc| cc.exit()),
                KeyCode::Tab => cx.add_callback(|cc| {
                    cc.replace_at(LayerId::POPUP, Popup);
                }),
                KeyCode::Enter => {
                    cx.state_mut().counter += 1;

                    if cx.state().counter == 10 {
                        cx.add_callback(|cc| cc.exit());
                    }
                }
                _ => {}
            }
        }
    }
}

struct Popup;
impl<'comp, T: 'comp> Component<'comp, T> for Popup {
    fn id(&self) -> Id {
        Id::new("popup")
    }

    fn view(&self, area: Rect, buf: &mut Buffer, _: &T) {
        let area = Rect {
            x: area.width / 2 - 20,
            y: area.height / 4 - 5,
            width: 40,
            height: 10,
        };

        Clear.render(area, buf);
        Block::new()
            .title("Popup!")
            .borders(Borders::ALL)
            .render(area, buf);
    }

    fn handle_event(&mut self, event: &mut EventAccess, cx: &mut Context<'_, T>) {
        match event.peak() {
            Event::Terminal(CTEvent::Key(ke)) if ke.code == KeyCode::Esc => {
                // tf?
                let id = <Popup as gland::Component<'_, T>>::id(self);

                cx.add_callback(move |cc| cc.remove_all(id));
                event.consume();
            }
            _ => {}
        }
    }
}

struct Input;

impl<'comp> Component<'comp, AppState> for Input {
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

    fn handle_event(&mut self, event: &mut EventAccess, cx: &mut Context<'_, AppState>) {
        if let Event::Terminal(CTEvent::Key(ke)) = event.peak() {
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
        counter: 1,
        text: "Write to modify the text, press enter to increment".to_owned(),
        start: Instant::now(),
    });
    comp.with_event_stream()
        .replace_at(LayerId::FOREGROUND, MainScreen { input: Input });
    comp.run(CrosstermBackend::new(io::stdout())).await?;

    Ok(())
}
