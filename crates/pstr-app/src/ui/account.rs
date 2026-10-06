//! The Proton account, on the shares page: signing in, watch-history sync, and
//! picking folders of the viewer's own Drive as library shares.
//!
//! Optional and said so: public links need no account, and the page reads the
//! same without one. What signing in adds is spelled out next to the form,
//! because "sign in" alone does not say why an app that streams public links
//! would want a password.

use pstr_core::account::{DriveEntry, DrivePlace, PlaceKind};
use pstr_core::proton_sdk::ids::NodeUid;
use pstr_core::sync::SyncReport;

use crate::app::Action;
use crate::engine::AccountStatus;
use crate::theme;
use crate::ui;

/// What the account section holds between frames.
#[derive(Default)]
pub struct AccountPanel {
    pub status: AccountStatus,
    pub username: String,
    pub password: String,
    pub code: String,
    pub mailbox: String,
    /// A request is out and not answered yet.
    pub busy: bool,
    /// Why the last sign-in step was refused.
    pub error: Option<String>,
    /// When watch history last synced, in Unix seconds, and what it did.
    pub synced: Option<(i64, SyncReport)>,
    /// Why the last sync failed, until one succeeds.
    pub sync_error: Option<String>,
    pub browser: DriveBrowser,
}

impl AccountPanel {
    /// Apply where the account now stands. The typed secrets go as soon as
    /// they are not needed: a password has no business sitting in a struct for
    /// the rest of the session.
    pub fn set_status(&mut self, status: AccountStatus) {
        self.busy = false;
        match &status {
            AccountStatus::SignedIn(_) | AccountStatus::SignedOut => {
                self.password.clear();
                self.code.clear();
                self.mailbox.clear();
                if matches!(status, AccountStatus::SignedIn(_)) {
                    self.error = None;
                } else {
                    self.browser = DriveBrowser::default();
                    self.synced = None;
                    self.sync_error = None;
                }
            }
            AccountStatus::SecondFactor => self.error = None,
            AccountStatus::MailboxPassword => {
                self.code.clear();
                self.error = None;
            }
            AccountStatus::Unknown => {}
        }
        self.status = status;
    }

    pub fn refused(&mut self, error: String) {
        self.busy = false;
        self.error = Some(error);
    }
}

/// Where the Drive browser is.
#[derive(Default)]
pub struct DriveBrowser {
    pub open: bool,
    /// The folders descended through, each by name. Empty means the places.
    pub trail: Vec<(String, NodeUid)>,
    pub places: Option<Vec<DrivePlace>>,
    /// The listing of the folder at the end of the trail, once it arrives.
    pub entries: Option<(NodeUid, Vec<DriveEntry>)>,
    pub loading: bool,
    pub error: Option<String>,
}

impl DriveBrowser {
    pub fn places_arrived(&mut self, places: Vec<DrivePlace>) {
        self.loading = false;
        self.error = None;
        self.places = Some(places);
    }

    /// Take a listing, unless the viewer has already moved on from it.
    pub fn folder_arrived(&mut self, uid: NodeUid, entries: Vec<DriveEntry>) {
        if self
            .trail
            .last()
            .is_some_and(|(_, current)| *current == uid)
        {
            self.loading = false;
            self.error = None;
            self.entries = Some((uid, entries));
        }
    }

    pub fn failed(&mut self, error: String) {
        self.loading = false;
        self.error = Some(error);
    }

    fn enter(&mut self, name: String, uid: NodeUid, actions: &mut Vec<Action>) {
        self.trail.push((name, uid.clone()));
        self.entries = None;
        self.loading = true;
        self.error = None;
        actions.push(Action::BrowseFolder(uid));
    }

    fn up(&mut self, actions: &mut Vec<Action>) {
        self.trail.pop();
        self.entries = None;
        self.error = None;
        match self.trail.last() {
            Some((_, uid)) => {
                self.loading = true;
                actions.push(Action::BrowseFolder(uid.clone()));
            }
            None => self.loading = self.places.is_none(),
        }
    }
}

/// The account card: a sign-in form, or who is signed in and how sync is doing.
pub fn section(ui: &mut egui::Ui, panel: &mut AccountPanel, actions: &mut Vec<Action>) {
    ui::section(ui, "Proton account");
    ui.label(ui::muted(
        "Optional. Signed in, your watch history is kept in your own Drive and every device \
         resumes where another left off, and folders of your Drive — your files, other \
         devices, what was shared with you — can be added to the library.",
    ));
    ui.add_space(theme::space::L);

    card(ui, |ui| match panel.status.clone() {
        AccountStatus::Unknown => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(ui::muted("reading the stored session…"));
            });
        }
        AccountStatus::SignedOut => sign_in_form(ui, panel, actions),
        AccountStatus::SecondFactor => code_form(ui, panel, actions),
        AccountStatus::MailboxPassword => mailbox_form(ui, panel, actions),
        AccountStatus::SignedIn(username) => signed_in(ui, panel, &username, actions),
    });
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(theme::radius::MD))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            add(ui);
        });
}

fn sign_in_form(ui: &mut egui::Ui, panel: &mut AccountPanel, actions: &mut Vec<Action>) {
    ui.label(ui::muted("Address"));
    ui.add(
        egui::TextEdit::singleline(&mut panel.username)
            .hint_text("you@proton.me")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(theme::space::M);
    ui.label(ui::muted("Password"));
    let field = ui.add(
        egui::TextEdit::singleline(&mut panel.password)
            .password(true)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(theme::space::L);
    error_line(ui, panel);
    let ready = !panel.username.trim().is_empty() && !panel.password.is_empty() && !panel.busy;
    let submitted = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled_ui(ready, |ui| ui::accent_button(ui, "Sign in"))
            .inner
            .clicked();
        if ready && (clicked || submitted) {
            panel.busy = true;
            panel.error = None;
            actions.push(Action::SignIn {
                username: panel.username.trim().to_owned(),
                password: panel.password.clone(),
            });
        }
        busy_line(ui, panel, "signing in…");
    });
}

fn code_form(ui: &mut egui::Ui, panel: &mut AccountPanel, actions: &mut Vec<Action>) {
    ui.label(ui::muted("Two-factor code from your authenticator app"));
    let field = ui.add(
        egui::TextEdit::singleline(&mut panel.code)
            .hint_text("123456")
            .desired_width(160.0),
    );
    field.request_focus();
    ui.add_space(theme::space::S);
    // Security keys need WebAuthn, which the sign-in here does not speak. An
    // account with nothing else is refused before this form (`AccountStore`).
    ui.label(ui::muted(
        "Security keys are not supported here — use the code from your authenticator app.",
    ));
    ui.add_space(theme::space::L);
    error_line(ui, panel);
    let ready = !panel.code.trim().is_empty() && !panel.busy;
    let submitted = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled_ui(ready, |ui| ui::accent_button(ui, "Continue"))
            .inner
            .clicked();
        if ready && (clicked || submitted) {
            panel.busy = true;
            actions.push(Action::SubmitSecondFactor(panel.code.trim().to_owned()));
        }
        if ui.button("Cancel").clicked() {
            actions.push(Action::CancelSignIn);
        }
        busy_line(ui, panel, "checking…");
    });
}

fn mailbox_form(ui: &mut egui::Ui, panel: &mut AccountPanel, actions: &mut Vec<Action>) {
    ui.label(ui::muted(
        "This account has a separate mailbox password. It unlocks your files, and is kept \
         in your system keyring.",
    ));
    ui.add_space(theme::space::S);
    let field = ui.add(
        egui::TextEdit::singleline(&mut panel.mailbox)
            .password(true)
            .hint_text("Mailbox password")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(theme::space::L);
    error_line(ui, panel);
    let ready = !panel.mailbox.is_empty() && !panel.busy;
    let submitted = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled_ui(ready, |ui| ui::accent_button(ui, "Unlock"))
            .inner
            .clicked();
        if ready && (clicked || submitted) {
            panel.busy = true;
            actions.push(Action::SubmitMailboxPassword(panel.mailbox.clone()));
        }
        if ui.button("Cancel").clicked() {
            actions.push(Action::CancelSignIn);
        }
        busy_line(ui, panel, "unlocking…");
    });
}

fn signed_in(
    ui: &mut egui::Ui,
    panel: &mut AccountPanel,
    username: &str,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{}  {username}",
                    egui_phosphor::regular::USER_CIRCLE
                ))
                .strong(),
            );
            let status = match (&panel.sync_error, &panel.synced) {
                (Some(error), _) => format!("Watch history did not sync: {error}"),
                (None, Some((at, report))) => sync_line(*at, report, unix_now()),
                (None, None) => "Watch history syncs with your Drive".to_owned(),
            };
            ui.label(ui::muted(status));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(format!("{}  Sign out", egui_phosphor::regular::SIGN_OUT))
                        .fill(theme::card_hover()),
                )
                .clicked()
            {
                actions.push(Action::SignOut);
            }
            if ui
                .button(format!(
                    "{}  Sync now",
                    egui_phosphor::regular::ARROWS_CLOCKWISE
                ))
                .clicked()
            {
                actions.push(Action::SyncNow);
            }
        });
    });
}

/// "Synced 3 min ago · 2 positions from other devices".
fn sync_line(at: i64, report: &SyncReport, now: i64) -> String {
    let ago = (now - at).max(0);
    let when = match ago {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", ago / 60),
        _ => format!("{} h ago", ago / 3600),
    };
    let mut line = format!("Synced {when}");
    if report.applied > 0 {
        line.push_str(&format!(
            "  ·  {} from other devices",
            ui::library::plural(report.applied, "position")
        ));
    }
    line
}

fn error_line(ui: &mut egui::Ui, panel: &AccountPanel) {
    if let Some(error) = &panel.error {
        ui.label(theme::Role::Caption.rich(error).color(theme::danger()));
        ui.add_space(theme::space::S);
    }
}

fn busy_line(ui: &mut egui::Ui, panel: &AccountPanel, text: &str) {
    if panel.busy {
        ui.add(egui::Spinner::new().size(14.0));
        ui.label(ui::muted(text));
    }
}

/// Browse the account's Drive and add a folder of it to the library.
pub fn browser(
    ui: &mut egui::Ui,
    panel: &mut AccountPanel,
    shares: &[pstr_core::Share],
    actions: &mut Vec<Action>,
) {
    if !matches!(panel.status, AccountStatus::SignedIn(_)) {
        return;
    }
    ui.add_space(theme::space::XXL);
    ui::section(ui, "Add from your Drive");
    let browser = &mut panel.browser;
    if !browser.open {
        ui.label(ui::muted(
            "A folder of your own Drive becomes a share like any link: crawled, browsable and \
             streamed through your account.",
        ));
        ui.add_space(theme::space::L);
        if ui
            .button(format!(
                "{}  Browse your Drive",
                egui_phosphor::regular::FOLDER
            ))
            .clicked()
        {
            browser.open = true;
            if browser.places.is_none() {
                browser.loading = true;
                actions.push(Action::BrowsePlaces);
            }
        }
        return;
    }

    card(ui, |ui| {
        ui.horizontal(|ui| {
            if !browser.trail.is_empty()
                && ui
                    .button(egui_phosphor::regular::ARROW_LEFT)
                    .on_hover_text("Up a folder")
                    .clicked()
            {
                browser.up(actions);
            }
            let mut crumbs = vec!["Drive".to_owned()];
            crumbs.extend(browser.trail.iter().map(|(name, _)| name.clone()));
            ui.label(
                egui::RichText::new(
                    crumbs.join(&format!("  {}  ", egui_phosphor::regular::CARET_RIGHT)),
                )
                .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    browser.open = false;
                }
            });
        });
        ui.add_space(theme::space::M);

        if let Some(error) = &browser.error {
            ui.label(theme::Role::Caption.rich(error).color(theme::danger()));
            ui.add_space(theme::space::S);
        }
        if browser.loading {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(ui::muted("listing…"));
            });
            return;
        }

        let mut entered = None;
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .auto_shrink([false, true])
            .show(ui, |ui| match browser.trail.last() {
                None => {
                    for place in browser.places.iter().flatten() {
                        let icon = match place.kind {
                            PlaceKind::MyFiles => egui_phosphor::regular::HARD_DRIVES,
                            PlaceKind::Device => egui_phosphor::regular::DESKTOP,
                            PlaceKind::SharedWithMe => egui_phosphor::regular::USERS,
                        };
                        if row(ui, icon, &place.name, None, true).clicked() {
                            entered = Some((place.name.clone(), place.uid.clone()));
                        }
                    }
                }
                Some(_) => {
                    let entries = browser
                        .entries
                        .as_ref()
                        .map(|(_, entries)| entries.as_slice());
                    if entries.is_some_and(<[DriveEntry]>::is_empty) {
                        ui.label(ui::muted("This folder is empty."));
                    }
                    for entry in entries.unwrap_or_default() {
                        let (icon, detail) = if entry.is_folder {
                            (egui_phosphor::regular::FOLDER, None)
                        } else if entry
                            .media_type
                            .as_deref()
                            .is_some_and(|media| media.starts_with("video/"))
                        {
                            (
                                egui_phosphor::regular::FILE_VIDEO,
                                entry.size.map(format_size),
                            )
                        } else {
                            (egui_phosphor::regular::FILE, entry.size.map(format_size))
                        };
                        if row(ui, icon, &entry.name, detail.as_deref(), entry.is_folder).clicked()
                            && entry.is_folder
                        {
                            entered = Some((entry.name.clone(), entry.uid.clone()));
                        }
                    }
                }
            });
        if let Some((name, uid)) = entered {
            browser.enter(name, uid, actions);
        }

        if let Some((name, uid)) = browser.trail.last() {
            ui.add_space(theme::space::L);
            let added = shares.iter().any(|share| {
                share
                    .folder
                    .as_ref()
                    .is_some_and(|folder| folder.uid() == *uid)
            });
            ui.horizontal(|ui| {
                if added {
                    ui.label(ui::muted("This folder is already in the library."));
                } else if ui::accent_button(ui, "Add this folder").clicked() {
                    actions.push(Action::AddAccountFolder {
                        name: name.clone(),
                        uid: uid.clone(),
                    });
                    browser.open = false;
                }
            });
        }
    });
}

/// One line of the browser. Only folders and places are clickable.
fn row(
    ui: &mut egui::Ui,
    icon: &str,
    name: &str,
    detail: Option<&str>,
    clickable: bool,
) -> egui::Response {
    let mut text = egui::text::LayoutJob::default();
    let body = ui.style().text_styles[&egui::TextStyle::Body].clone();
    text.append(
        &format!("{icon}  {name}"),
        0.0,
        egui::TextFormat::simple(
            body.clone(),
            if clickable {
                theme::text()
            } else {
                theme::muted()
            },
        ),
    );
    if let Some(detail) = detail {
        text.append(
            &format!("   {detail}"),
            0.0,
            egui::TextFormat::simple(body, theme::muted()),
        );
    }
    let sense = if clickable {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let response = ui.add(egui::Label::new(text).sense(sense).truncate());
    if clickable && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn format_size(bytes: i64) -> String {
    let bytes = bytes.max(0) as f64;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else {
        format!("{:.0} MiB", bytes / MIB)
    }
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sync_line_says_how_long_ago_and_what_came_in() {
        let quiet = SyncReport::default();
        assert_eq!(sync_line(100, &quiet, 130), "Synced just now");
        assert_eq!(sync_line(100, &quiet, 400), "Synced 5 min ago");
        let busy = SyncReport {
            applied: 2,
            ..SyncReport::default()
        };
        assert_eq!(
            sync_line(0, &busy, 7200),
            "Synced 2 h ago  ·  2 positions from other devices"
        );
    }

    #[test]
    fn signing_out_forgets_what_was_typed_and_browsed() {
        let mut panel = AccountPanel {
            password: "secret".into(),
            mailbox: "also secret".into(),
            ..AccountPanel::default()
        };
        panel.browser.open = true;
        panel.set_status(AccountStatus::SignedOut);
        assert!(panel.password.is_empty() && panel.mailbox.is_empty());
        assert!(!panel.browser.open);
    }

    #[test]
    fn a_listing_for_a_folder_already_left_is_dropped() {
        let uid = |link: &str| {
            NodeUid::new(
                pstr_core::proton_sdk::ids::VolumeId::new("v".to_owned()),
                pstr_core::proton_sdk::ids::LinkId::new(link.to_owned()),
            )
        };
        let mut browser = DriveBrowser {
            trail: vec![("b".into(), uid("b"))],
            loading: true,
            ..DriveBrowser::default()
        };
        browser.folder_arrived(uid("a"), Vec::new());
        assert!(browser.loading && browser.entries.is_none());
        browser.folder_arrived(uid("b"), Vec::new());
        assert!(!browser.loading && browser.entries.is_some());
    }
}
