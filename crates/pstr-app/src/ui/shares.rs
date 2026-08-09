//! The shares page: which links this app knows, and how to add one.
//!
//! The URL fragment of a Proton share link **is** its decryption password, so
//! this page never prints a URL back — `shares.json` holds only the id, the name
//! and the token, and the secrets live in the OS credential store. The form
//! below is the only place a link is ever visible, and only while it is being
//! typed.

use pstr_core::Share;

use crate::app::{Action, ShareForm};
use crate::theme;
use crate::ui;

pub fn show(ui: &mut egui::Ui, shares: &[Share], form: &mut ShareForm, actions: &mut Vec<Action>) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui::section(ui, "Shares");
            if shares.is_empty() {
                ui.label(ui::muted("No shares yet."));
            }
            for share in shares {
                share_row(ui, share, actions);
            }

            ui.add_space(22.0);
            add_form(ui, form, actions);
        });
}

fn share_row(ui: &mut egui::Ui, share: &Share, actions: &mut Vec<Action>) {
    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&share.name).strong());
                    let detail = if share.has_custom_password {
                        format!("{}  ·  custom password", share.id)
                    } else {
                        share.id.clone()
                    };
                    ui.label(ui::muted(detail));
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("Remove").fill(theme::card_hover()))
                        .on_hover_text("Forget this share, its catalog rows and its stored secrets")
                        .clicked()
                    {
                        actions.push(Action::RemoveShare(share.id.clone()));
                    }
                    if ui.button("Crawl").clicked() {
                        actions.push(Action::Crawl(Some(share.id.clone())));
                    }
                });
            });
        });
    ui.add_space(6.0);
}

fn add_form(ui: &mut egui::Ui, form: &mut ShareForm, actions: &mut Vec<Action>) {
    ui::section(ui, "Add a share");
    ui.label(ui::muted(
        "Paste the whole link, including everything after the # — that part is the key that \
         decrypts it, and it is stored in your system keyring rather than on disk.",
    ));
    ui.add_space(10.0);

    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));

            ui.label(ui::muted("Name"));
            ui.add(
                egui::TextEdit::singleline(&mut form.name)
                    .hint_text("Anime")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);

            ui.label(ui::muted("Link"));
            ui.add(
                egui::TextEdit::singleline(&mut form.url)
                    .password(true)
                    .hint_text("https://drive.proton.me/urls/…#…")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);

            ui.checkbox(&mut form.has_password, "The link asks for a password");
            if form.has_password {
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut form.password)
                        .password(true)
                        .hint_text("Link password")
                        .desired_width(f32::INFINITY),
                );
            }

            ui.add_space(12.0);
            let ready = !form.name.trim().is_empty()
                && !form.url.trim().is_empty()
                && (!form.has_password || !form.password.is_empty());

            ui.horizontal(|ui| {
                if ui
                    .add_enabled_ui(ready, |ui| ui::accent_button(ui, "Add and crawl"))
                    .inner
                    .clicked()
                {
                    actions.push(Action::AddShare {
                        name: form.name.trim().to_string(),
                        url: form.url.trim().to_string(),
                        password: form
                            .has_password
                            .then(|| form.password.clone())
                            .filter(|password| !password.is_empty()),
                    });
                }
                if !ready {
                    ui.label(ui::muted("A name and a link are needed."));
                }
            });
        });
}
