// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Minimal iced shell. Complex launcher behavior intentionally stays out of the UI layer.

use iced::Element;
use iced::widget::{column, text};

fn main() -> iced::Result {
    iced::run("Exosphere", update, view)
}

#[derive(Default)]
struct App;
#[derive(Debug, Clone)]
enum Message {}
fn update(_app: &mut App, _message: Message) {}
fn view(_app: &App) -> Element<'_, Message> {
    column![text("Exosphere"), text("Phase 10 Linux bootstrap")].into()
}
