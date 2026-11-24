//! The shares page: which links this app knows, and how to add one.
//!
//! The URL fragment of a Proton share link **is** its decryption password, so
//! this page never prints a URL back — `shares.json` holds only the id, the name
//! and the token, and the secrets live in the OS credential store. The form
//! below is the only place a link is ever visible, and only while it is being
//! typed.

use std::collections::HashMap;

use pstr_core::Share;
use pstr_core::catalog::CrawlRecord;
use pstr_core::invite::InvitedShare;
use pstr_core::library::Library;

use crate::app::{Action, ShareForm};
use crate::theme;
use crate::ui;

pub fn show(
    ui: &mut egui::Ui,
    shares: &[Share],
    crawls: &HashMap<String, CrawlRecord>,
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
                share_row(ui, share, titles, crawls.get(&share.id), form, actions);
            }
            // Two or more links make a library worth handing on whole.
            if shares.iter().filter(|share| share.folder.is_none()).count() > 1 {
                ui.add_space(theme::space::S);
                if ui
                    .button(format!(
                        "{}  Share the whole library",
                        egui_phosphor::regular::SHARE_NETWORK
                    ))
                    .on_hover_text("One link that adds every share here on another device")
                    .clicked()
                {
                    actions.push(Action::ShareInvite(Vec::new()));
                }
            }

            ui.add_space(theme::space::XXL);
            add_form(ui, shares, form, actions);

            ui.add_space(theme::space::XXL);
            ui::account::section(ui, account, actions);
            ui::account::browser(ui, account, shares, actions);
        });

    let ctx = ui.ctx().clone();
    if let Some(incoming) = &mut form.invite_in
        && !incoming_dialog(&ctx, incoming, actions)
    {
        form.invite_in = None;
    }
    if let Some(outgoing) = &mut form.invite_out
        && !outgoing_dialog(&ctx, outgoing)
    {
        form.invite_out = None;
    }
}

/// A `pstr://` invite that arrived, and which of its shares to add.
pub struct IncomingInvite {
    /// Each share, whether the library already has it, and whether it is
    /// ticked.
    rows: Vec<(InvitedShare, bool, bool)>,
}

impl IncomingInvite {
    pub fn new(invited: Vec<InvitedShare>, shares: &[Share]) -> Self {
        let rows = invited
            .into_iter()
            .map(|share| {
                let have = pstr_core::shares::share_token(&share.url)
                    .is_ok_and(|token| shares.iter().any(|known| known.token == token));
                (share, have, !have)
            })
            .collect();
        Self { rows }
    }
}

/// The invite confirmation. `false` once it is closed.
fn incoming_dialog(
    ctx: &egui::Context,
    invite: &mut IncomingInvite,
    actions: &mut Vec<Action>,
) -> bool {
    let mut open = true;
    egui::Modal::new(egui::Id::new("invite-in"))
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.set_width(460.0);
            let count = invite.rows.len();
            ui.label(
                theme::Role::Heading
                    .rich(if count == 1 {
                        "Add a shared library?".to_owned()
                    } else {
                        format!("Add {count} shared libraries?")
                    })
                    .strong()
                    .color(theme::text()),
            );
            ui.add_space(theme::space::S);
            ui.label(ui::muted(
                "Someone sent you an invite. Each share opens before it is added, and the \
                 links and passwords go to your system keyring.",
            ));
            ui.add_space(theme::space::L);
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    for (share, have, ticked) in &mut invite.rows {
                        let name = if share.name.is_empty() {
                            "Shared folder".to_owned()
                        } else {
                            share.name.clone()
                        };
                        if *have {
                            ui.horizontal(|ui| {
                                ui.add_enabled_ui(false, |ui| {
                                    ui::widgets::toggle(ui, &mut false, &name)
                                });
                                ui.label(ui::muted("already added"));
                            });
                        } else {
                            let label = if share.password.is_some() {
                                format!("{name}  ·  password included")
                            } else {
                                name
                            };
                            ui::widgets::toggle(ui, ticked, &label);
                        }
                        ui.add_space(theme::space::XS);
                    }
                });
            ui.add_space(theme::space::XL);
            let chosen: Vec<InvitedShare> = invite
                .rows
                .iter()
                .filter(|(_, have, ticked)| !have && *ticked)
                .map(|(share, _, _)| share.clone())
                .collect();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let verb = match chosen.len() {
                    0 | 1 => "Add".to_owned(),
                    count => format!("Add {count}"),
                };
                if ui
                    .add_enabled_ui(!chosen.is_empty(), |ui| ui::accent_button(ui, &verb))
                    .inner
                    .clicked()
                {
                    actions.push(Action::AddInvited(chosen));
                    open = false;
                }
                if ui.button("Cancel").clicked() {
                    open = false;
                }
            });
        });
    open
}

/// An invite being handed out.
pub struct OutgoingInvite {
    link: String,
    left_out: usize,
    /// `None` when the link is too long for a QR code — a very large library.
    qr: Option<egui::TextureHandle>,
}

impl OutgoingInvite {
    pub fn new(ctx: &egui::Context, link: String, left_out: usize) -> Self {
        let qr = pstr_core::handoff::Qr::encode(&link)
            .ok()
            .map(|qr| ctx.load_texture("invite-qr", qr_image(&qr), egui::TextureOptions::NEAREST));
        Self { link, left_out, qr }
    }
}

/// The QR code as an image: black on white whatever the theme, since that is
/// what a camera reads best, with the four-module quiet zone the standard asks
/// for.
fn qr_image(qr: &pstr_core::handoff::Qr) -> egui::ColorImage {
    const QUIET: usize = 4;
    let side = qr.width + 2 * QUIET;
    let mut pixels = vec![egui::Color32::WHITE; side * side];
    for (index, dark) in qr.dark.iter().enumerate() {
        if *dark {
            let (y, x) = (index / qr.width, index % qr.width);
            pixels[(y + QUIET) * side + x + QUIET] = egui::Color32::BLACK;
        }
    }
    egui::ColorImage {
        size: [side, side],
        source_size: egui::vec2(side as f32, side as f32),
        pixels,
    }
}

/// The invite's QR code and a button that copies its link. `false` once it is
/// closed.
fn outgoing_dialog(ctx: &egui::Context, invite: &mut OutgoingInvite) -> bool {
    let mut open = true;
    egui::Modal::new(egui::Id::new("invite-out"))
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                theme::Role::Heading
                    .rich("Invite to your library")
                    .strong()
                    .color(theme::text()),
            );
            ui.add_space(theme::space::S);
            ui.label(ui::muted(
                "Scan with a phone that has Proton Stream, or send the link. It opens in \
                 Proton Stream on any device.",
            ));
            ui.add_space(theme::space::L);
            if let Some(texture) = &invite.qr {
                ui.vertical_centered(|ui| {
                    ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(egui::vec2(240.0, 240.0))
                            .corner_radius(egui::CornerRadius::same(theme::radius::MD)),
                    );
                });
                ui.add_space(theme::space::L);
            }
            // The link carries every password in it, so it is never printed:
            // only copied, on purpose.
            ui.label(
                theme::Role::Caption
                    .rich(format!(
                        "{}  Anyone with this link can open what it shares, passwords included. \
                         Send it only to people you trust.",
                        egui_phosphor::regular::WARNING
                    ))
                    .color(theme::muted()),
            );
            if invite.left_out > 0 {
                ui.add_space(theme::space::XS);
                ui.label(
                    theme::Role::Caption
                        .rich(format!(
                            "{} from your Proton account left out: they have no link to share.",
                            ui::library::plural(invite.left_out, "folder")
                        ))
                        .color(theme::muted()),
                );
            }
            ui.add_space(theme::space::XL);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui::accent_button(ui, "Copy link").clicked() {
                    ui.ctx().copy_text(invite.link.clone());
                    open = false;
                }
                if ui.button("Close").clicked() {
                    open = false;
                }
            });
        });
    open
}

fn dialog_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::surface())
        .inner_margin(egui::Margin::same(18))
        .corner_radius(egui::CornerRadius::same(theme::radius::LG))
}

fn share_row(
    ui: &mut egui::Ui,
    share: &Share,
    titles: usize,
    crawl: Option<&CrawlRecord>,
    form: &mut ShareForm,
    actions: &mut Vec<Action>,
) {
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
                    match crawl {
                        Some(crawl) if crawl.crawled_at > 0 => {
                            detail.push_str("  ·  crawled ");
                            detail.push_str(&ui::ago(crawl.crawled_at, crate::engine::unix_now()));
                        }
                        _ => detail.push_str("  ·  not crawled yet"),
                    }
                    ui.label(ui::muted(detail)).on_hover_text(&share.id);
                    // Why the last look failed, until one succeeds: a link
                    // revoked upstream otherwise just stops growing.
                    if let Some(error) = crawl.and_then(|crawl| crawl.error.as_deref()) {
                        ui.label(
                            theme::Role::Caption
                                .rich(format!("Last crawl failed: {error}"))
                                .color(theme::danger()),
                        );
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("Remove").fill(theme::card_hover()))
                        .on_hover_text("Forget this share, its catalog rows and its stored secrets")
                        .clicked()
                    {
                        actions.push(Action::RemoveShare(share.id.clone()));
                    }
                    if share.folder.is_none()
                        && ui
                            .button(egui_phosphor::regular::SHARE_NETWORK)
                            .on_hover_text("Invite someone: a link and QR code that add this share")
                            .clicked()
                    {
                        actions.push(Action::ShareInvite(vec![share.id.clone()]));
                    }
                    if ui.button("Crawl").clicked() {
                        actions.push(Action::Crawl(Some(share.id.clone())));
                    }
                    if ui.button("Rename").clicked() {
                        form.renaming = Some(share.id.clone());
                        form.rename_to = share.name.clone();
                    }
                    // Not for a folder of the account, which has no link.
                    if share.folder.is_none()
                        && ui
                            .button("Re-enter link")
                            .on_hover_text(
                                "For a keyring that lost the link, or a password that changed. \
                                 The library, history and downloads stay.",
                            )
                            .clicked()
                    {
                        form.repairing = Some(share.id.clone());
                        form.repair_url.clear();
                        form.repair_password.clear();
                    }
                });
            });
            if form.repairing.as_deref() == Some(share.id.as_str()) {
                repair_form(ui, share, form, actions);
            }
            if form.renaming.as_deref() == Some(share.id.as_str()) {
                rename_form(ui, share, form, actions);
            }
        });
    ui.add_space(theme::space::S);
}

/// A new name for a share, in its own row. Enter saves, as everywhere else.
fn rename_form(ui: &mut egui::Ui, share: &Share, form: &mut ShareForm, actions: &mut Vec<Action>) {
    ui.add_space(theme::space::M);
    ui.horizontal(|ui| {
        let field = ui.add(
            egui::TextEdit::singleline(&mut form.rename_to)
                .hint_text("Name")
                .desired_width(320.0),
        );
        let ready = !form.rename_to.trim().is_empty();
        let entered = field.lost_focus() && ui_enter_pressed(ui.ctx());
        if ui
            .add_enabled_ui(ready, |ui| ui::accent_button(ui, "Save"))
            .inner
            .clicked()
            || (ready && entered)
        {
            actions.push(Action::RenameShare {
                id: share.id.clone(),
                name: form.rename_to.trim().to_owned(),
            });
        }
        if ui.button("Cancel").clicked() {
            form.renaming = None;
        }
    });
}

/// The same link again, and a password if it has one, for a share already in
/// the library.
fn repair_form(ui: &mut egui::Ui, share: &Share, form: &mut ShareForm, actions: &mut Vec<Action>) {
    ui.add_space(theme::space::M);
    ui.add(
        egui::TextEdit::singleline(&mut form.repair_url)
            .password(true)
            .hint_text("The share link, including everything after the #")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(theme::space::S);
    ui.add(
        egui::TextEdit::singleline(&mut form.repair_password)
            .password(true)
            .hint_text("Link password, if it has one")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(theme::space::S);
    let problem = (!form.repair_url.trim().is_empty())
        .then(|| pstr_core::shares::share_token(form.repair_url.trim()).err())
        .flatten();
    if let Some(problem) = &problem {
        ui.label(
            theme::Role::Caption
                .rich(problem.to_string())
                .color(theme::danger()),
        );
    }
    ui.horizontal(|ui| {
        let ready = !form.repair_url.trim().is_empty() && problem.is_none();
        if ui
            .add_enabled_ui(ready, |ui| ui::accent_button(ui, "Save"))
            .inner
            .clicked()
        {
            actions.push(Action::RepairShare {
                id: share.id.clone(),
                url: form.repair_url.trim().to_owned(),
                password: Some(form.repair_password.clone())
                    .filter(|password| !password.is_empty()),
            });
        }
        if ui.button("Cancel").clicked() {
            form.repairing = None;
        }
    });
}

/// The add form's link field, which the library's empty state focuses.
pub fn link_field_id() -> egui::Id {
    egui::Id::new("share-link")
}

fn ui_enter_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.key_pressed(egui::Key::Enter))
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

            // Enter in any field of the form submits it, as the sign-in
            // form's do.
            let mut entered = false;
            let mut submitted_from = |response: egui::Response| {
                entered |= response.lost_focus() && ui_enter_pressed(&response.ctx);
            };

            ui.label(ui::muted("Link"));
            ui.horizontal(|ui| {
                let paste = 60.0;
                let field = link_field_id();
                if std::mem::take(&mut form.focus) {
                    ui.memory_mut(|memory| memory.request_focus(field));
                }
                submitted_from(
                    ui.add(
                        egui::TextEdit::singleline(&mut form.url)
                            .id(field)
                            .password(true)
                            .hint_text("https://drive.proton.me/urls/…#…  or a pstr:// invite")
                            .desired_width(ui.available_width() - paste),
                    ),
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

            ui.label(ui::muted("Name (optional)"));
            submitted_from(
                ui.add(
                    egui::TextEdit::singleline(&mut form.name)
                        .hint_text("The shared folder's own name")
                        .desired_width(f32::INFINITY),
                ),
            );
            ui.add_space(theme::space::M);

            ui::widgets::toggle(ui, &mut form.has_password, "The link asks for a password");
            if form.has_password {
                ui.add_space(theme::space::S);
                submitted_from(
                    ui.add(
                        egui::TextEdit::singleline(&mut form.password)
                            .password(true)
                            .hint_text("Link password")
                            .desired_width(f32::INFINITY),
                    ),
                );
            }

            ui.add_space(theme::space::L);
            let ready = !form.url.trim().is_empty()
                && problem.is_none()
                && (!form.has_password || !form.password.is_empty())
                && !form.sending;

            if let Some(error) = &form.error {
                ui.label(theme::Role::Caption.rich(error).color(theme::danger()));
                ui.add_space(theme::space::S);
            }
            ui.horizontal(|ui| {
                let invite = pstr_core::invite::is_invite(&form.url);
                let clicked = ui
                    .add_enabled_ui(ready, |ui| {
                        ui::accent_button(ui, if invite { "Open invite" } else { "Add share" })
                    })
                    .inner
                    .clicked();
                if invite && (clicked || (ready && entered)) {
                    actions.push(Action::PasteShare(form.url.trim().to_string()));
                } else if clicked || (ready && entered) {
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
                } else if form.url.trim().is_empty() {
                    ui.label(ui::muted("Paste a share link to add it."));
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
    if pstr_core::invite::is_invite(url) {
        return pstr_core::invite::decode(url)
            .err()
            .map(|error| error.to_string());
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
