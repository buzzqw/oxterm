//! The thin row of user-defined widgets at the bottom of a window.
//!
//! Each widget runs a shell command in a worker thread on an interval and
//! shows the first line of its output. The GTK label is only touched on the
//! main thread: the worker sends a [`SendWeak`](crate::SendWeak) and uses
//! [`run_on_main`](crate::run_on_main) to apply the result.

use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gtk::prelude::*;

use crate::headless::extensions::{self, UiConfig, Widget};

pub struct StatusBar {
    container: gtk::Box,
}

impl StatusBar {
    pub fn new(config: &UiConfig, cwd: Rc<dyn Fn() -> String>) -> Option<Self> {
        if config.widgets.is_empty() {
            return None;
        }
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        container.style_context().add_class("oxterm-status-bar");
        container.set_margin_start(8);
        container.set_margin_end(8);
        for widget in &config.widgets {
            let label = gtk::Label::new(None);
            label.set_single_line_mode(true);
            label.set_ellipsize(pango::EllipsizeMode::End);
            label.set_max_width_chars(widget.max_chars.min(i32::MAX as usize) as i32);
            label.set_tooltip_text(Some(&widget.command));
            label.style_context().add_class("oxterm-status-widget");
            container.pack_start(&label, false, false, 0);
            start_widget(label, widget.clone(), cwd.clone());
        }
        container.show_all();
        Some(Self { container })
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}

fn start_widget(label: gtk::Label, widget: Widget, cwd: Rc<dyn Fn() -> String>) {
    let running = Arc::new(AtomicBool::new(false));
    let weak = crate::SendWeak::new(&label);
    let interval = widget.interval.max(1) as u32;
    let refresh = std::rc::Rc::new(move || {
        // A widget that was removed with its window must stop running shell
        // commands; otherwise every closed status bar would keep a timer and
        // a child process alive for the rest of the session.
        if weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        if running.swap(true, Ordering::SeqCst) {
            // The previous run is still in flight; skip this tick.
            return glib::ControlFlow::Continue;
        }
        let directory = cwd();
        let command = widget.command.clone();
        let template = widget.clone();
        let running = running.clone();
        let weak = weak.clone();
        std::thread::spawn(move || {
            let path = Path::new(&directory);
            let cwd = if !directory.is_empty() && path.is_dir() {
                Some(path)
            } else {
                None
            };
            let result = extensions::run_command(&command, cwd, Duration::from_secs(2));
            let markup = render(&template, result);
            let running = running.clone();
            crate::run_on_main(move || {
                if let Some(label) = weak.upgrade() {
                    label.set_markup(&markup);
                }
                running.store(false, Ordering::SeqCst);
            });
        });
        glib::ControlFlow::Continue
    });
    // The first terminal is created from an idle callback, so give it a moment
    // before running the command in its directory.
    let first = refresh.clone();
    glib::timeout_add_local_once(Duration::from_millis(300), move || {
        (*first)();
    });
    glib::timeout_add_seconds_local(interval, move || (*refresh)());
}

fn render(widget: &Widget, result: Result<String, String>) -> String {
    let value = match result {
        Ok(output) => first_line(&output),
        Err(_) => "—".to_string(),
    };
    let mut text = if value.is_empty() {
        widget.label.clone()
    } else if widget.label.is_empty() {
        value
    } else {
        format!("{}: {}", widget.label, value)
    };
    truncate(&mut text, widget.max_chars);
    let escaped = glib::markup_escape_text(&text);
    match &widget.color {
        Some(color) => format!("<span foreground='{}'>{}</span>", color, escaped),
        None => escaped.to_string(),
    }
}

fn first_line(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

fn truncate(text: &mut String, max_chars: usize) {
    if text.chars().count() <= max_chars {
        return;
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    text.clear();
    text.push_str(&kept);
    text.push('…');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(color: Option<&str>) -> Widget {
        Widget {
            id: "w".to_string(),
            label: "git".to_string(),
            command: "true".to_string(),
            interval: 5,
            color: color.map(str::to_string),
            max_chars: 20,
        }
    }

    #[test]
    fn renders_the_first_line_and_escapes_markup() {
        let markup = render(
            &widget(Some("#aabbcc")),
            Ok("main\nother <b>\n".to_string()),
        );
        assert_eq!(markup, "<span foreground='#aabbcc'>git: main</span>");
        let escaped = render(&widget(None), Ok("a < b".to_string()));
        assert_eq!(escaped, "git: a &lt; b");
    }

    #[test]
    fn failures_and_long_values_are_capped() {
        assert_eq!(render(&widget(None), Err("no".to_string())), "git: —");
        let long = "x".repeat(100);
        let markup = render(&widget(None), Ok(long));
        assert!(markup.chars().count() <= 20, "markup too long: {markup}");
        assert!(markup.ends_with('…'));
    }
}
