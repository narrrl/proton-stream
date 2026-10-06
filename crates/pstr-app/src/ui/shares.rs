//! The shares page: which links this app knows, and how to add one.
//!
//! The URL fragment of a Proton share link **is** its decryption password, so
//! this page never prints a URL back — `shares.json` holds only the id, the name
//! and the token, and the secrets live in the OS credential store. The form
//! below is the only place a link is ever visible, and only while it is being
//! typed.

use pstr_core::Share;
use pstr_core::library::Library;

use crate::app::{Action, ShareForm};
use crate::theme;
use crate::ui;

pub fn show(
    ui: &mut egui::Ui,
    shares: &[Share],
    library: &Library,
    form: &mut ShareForm,
    account: &mut ui::account::AccountPanel,
    actions: &mut Vec<Action>,
) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui::section(ui, "Shares");
            if shares.is_empty() {
                ui.label(ui::muted(
                    "No shares yet. Add one below and its contents become your library.",
                ));
            }
            for share in shares {
                let titles = library
                    .titles
                    .iter()
                    .filter(|title| title.share_ids.contains(&share.id))
                    .count();
                share_row(ui, share, titles, actions);
            }

            ui.add_space(theme::space::XXL);
            add_form(ui, shares, form, actions);

            ui.add_space(theme::space::XXL);
            ui::account::section(ui, account, actions);
            ui::account::browser(ui, account, shares, actions);
        });
}

fn share_row(ui: &mut egui::Ui, share: &Share, titles: usize, actions: &mut Vec<Action>) {
    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(theme::radius::MD))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&share.name).strong());
                    // What is in it, rather than what it is called inside the
                    // app. The id is still a hover away for a bug report.
                    let mut detail = ui::library::plural(titles, "title");
                    if share.folder.is_some() {
                        detail.push_str("  ·  from your Drive");
                    }
                    if share.has_custom_password {
                        detail.push_str("  ·  password protected");
                    }
                    ui.label(ui::muted(detail)).on_hover_text(&share.id);
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
    ui.add_space(theme::space::S);
}

fn add_form(ui: &mut egui::Ui, shares: &[Share], form: &mut ShareForm, actions: &mut Vec<Action>) {
    ui::section(ui, "Add a share");
    ui.label(ui::muted(
        "Paste the whole link, including everything after the # — that part is the key that \
         decrypts it, and it is stored in your system keyring rather than on disk.",
    ));
    ui.add_space(theme::space::L);

    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(theme::radius::MD))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));

            ui.label(ui::muted("Name"));
            ui.add(
                egui::TextEdit::singleline(&mut form.name)
                    .hint_text("Anime")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(theme::space::M);

            ui.label(ui::muted("Link"));
            ui.horizontal(|ui| {
                let paste = 60.0;
                let field = egui::Id::new("share-link");
                ui.add(
                    egui::TextEdit::singleline(&mut form.url)
                        .id(field)
                        .password(true)
                        .hint_text("https://drive.proton.me/urls/…#…")
                        .desired_width(ui.available_width() - paste),
                );
                // The link is masked, so pasting is the only sensible way in —
                // and a button says that more plainly than the field does.
                if ui
                    .add_sized(
                        [paste - 8.0, ui.spacing().interact_size.y],
                        egui::Button::new("Paste"),
                    )
                    .clicked()
                {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    // The paste lands in whatever has focus, so give it some.
                    ui.memory_mut(|memory| memory.request_focus(field));
                }
            });
            // Said while typing, not after a refused submit: everything this
            // checks is knowable from the text alone.
            let problem = link_problem(&form.url, shares);
            if let Some(problem) = &problem {
                ui.label(theme::Role::Caption.rich(problem).color(theme::danger()));
            }
            ui.add_space(theme::space::M);

            ui::widgets::toggle(ui, &mut form.has_password, "The link asks for a password");
            if form.has_password {
                ui.add_space(theme::space::S);
                ui.add(
                    egui::TextEdit::singleline(&mut form.password)
                        .password(true)
                        .hint_text("Link password")
                        .desired_width(f32::INFINITY),
                );
            }

            ui.add_space(theme::space::L);
            let ready = !form.name.trim().is_empty()
                && !form.url.trim().is_empty()
                && problem.is_none()
                && (!form.has_password || !form.password.is_empty())
                && !form.sending;

            if let Some(error) = &form.error {
                ui.label(theme::Role::Caption.rich(error).color(theme::danger()));
                ui.add_space(theme::space::S);
            }
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
                if form.sending {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(ui::muted("adding…"));
                } else if form.name.trim().is_empty() || form.url.trim().is_empty() {
                    ui.label(ui::muted("A name and a link are needed."));
                }
            });
        });
}

/// What is wrong with a link as typed, if anything.
///
/// Nothing for an empty box — that is "not done yet", which the button already
/// says.
fn link_problem(url: &str, shares: &[Share]) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    match pstr_core::shares::share_token(url) {
        Ok(token) if shares.iter().any(|share| share.token == token) => {
            Some("That share is already added.".into())
        }
        Ok(_) => None,
        Err(error) => Some(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(token: &str) -> Share {
        Share {
            id: format!("share-{token}"),
            name: "anime".into(),
            token: token.into(),
            has_custom_password: false,
            folder: None,
        }
    }

    #[test]
    fn an_empty_link_is_not_yet_a_problem() {
        assert_eq!(link_problem("  ", &[]), None);
    }

    #[test]
    fn a_link_without_its_key_is_refused_while_typing() {
        assert!(link_problem("https://drive.proton.me/urls/ABC123", &[]).is_some());
        assert!(link_problem("https://example.com/", &[]).is_some());
        assert_eq!(
            link_problem("https://drive.proton.me/urls/ABC123#key", &[]),
            None
        );
    }

    #[test]
    fn a_share_already_added_is_refused_while_typing() {
        assert!(
            link_problem(
                "https://drive.proton.me/urls/ABC123#key",
                &[share("ABC123")]
            )
            .is_some()
        );
    }
}
