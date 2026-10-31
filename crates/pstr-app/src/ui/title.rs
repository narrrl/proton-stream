//! One title: the still, what it is, and every episode under it.

use pstr_core::library::{Episode, Library, Season, Title, TitleKind};

use pstr_core::metadata::TitleMetadata;

use crate::app::{Action, Page};
use crate::engine::{DownloadItem, DownloadKey, DownloadState};
use crate::playback::PlaybackTarget;
use crate::theme;
use crate::ui::widgets::Named;
use crate::ui::{self, Art, Card};

pub struct OfflineView<'a> {
    pub downloads: &'a [DownloadItem],
    pub files: &'a std::collections::HashSet<DownloadKey>,
}

/// Where the title stands beyond its own files.
pub struct Context<'a> {
    /// The keys of its whole franchise in release order, if it has one.
    pub franchise: Option<&'a [String]>,
    /// The keys of the titles most like it, best first.
    pub similar: &'a [String],
    pub on_list: bool,
}

pub fn show(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    library: &Library,
    key: &str,
    context: Context<'_>,
    offline: OfflineView<'_>,
    actions: &mut Vec<Action>,
) {
    let Context {
        franchise,
        similar,
        on_list,
    } = context;
    let Some(title) = library.get(key) else {
        // The catalog was replaced under the page — a recrawl that dropped this
        // title. Nothing to show, so go back rather than draw an empty shell.
        actions.push(Action::Goto(Page::Library));
        return;
    };

    if back_link(ui, "Library").clicked() {
        actions.push(Action::Goto(Page::Library));
    }
    ui.add_space(theme::space::S);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            header(ui, art, title, on_list, &offline, actions);
            ui.add_space(theme::space::XL);

            if let Some(keys) = franchise {
                let members: Vec<&Title> = keys.iter().filter_map(|key| library.get(key)).collect();
                if members.len() > 1 {
                    ui::section(ui, "In this franchise");
                    cover_row(
                        ui,
                        art,
                        ("franchise", &title.key),
                        Some(title),
                        &members,
                        actions,
                    );
                    ui.add_space(theme::space::XL);
                }
            }

            // One season at a time, picked from a row of pills, rather than
            // every season stacked as a collapsing header: a four-season show
            // was a page of headers to scroll past to reach the one being
            // watched. The one opened first is the one with the next episode.
            let picked_id = ui.id().with(("season", &title.key));
            let default = title
                .next_up()
                .and_then(|next| {
                    title.seasons.iter().position(|season| {
                        season
                            .episodes
                            .iter()
                            .any(|episode| episode.node.link_id == next.node.link_id)
                    })
                })
                .unwrap_or(0);
            let mut picked = ui
                .data(|data| data.get_temp::<usize>(picked_id))
                .unwrap_or(default)
                .min(title.seasons.len().saturating_sub(1));

            let Some(season) = title.seasons.get(picked) else {
                return;
            };
            ui.horizontal(|ui| {
                if title.seasons.len() > 1 {
                    let labels: Vec<String> =
                        title.seasons.iter().map(|season| season.label()).collect();
                    let choices: Vec<(usize, &str)> = labels
                        .iter()
                        .enumerate()
                        .map(|(index, label)| (index, label.as_str()))
                        .collect();
                    // A long-running show's row of pills ran off the page;
                    // past a handful it is a list to pick from instead.
                    let chosen = if choices.len() > SEASON_PILLS {
                        egui::ComboBox::from_id_salt(("season-list", &title.key))
                            .selected_text(labels[picked].as_str())
                            .height(360.0)
                            .show_ui(ui, |ui| {
                                let mut chosen = None;
                                for &(index, label) in &choices {
                                    if ui.selectable_label(index == picked, label).clicked() {
                                        chosen = Some(index);
                                    }
                                }
                                chosen
                            })
                            .inner
                            .flatten()
                    } else {
                        ui::widgets::segmented(ui, picked, &choices)
                    };
                    if let Some(index) = chosen {
                        picked = index;
                        ui.data_mut(|data| data.insert_temp(picked_id, index));
                    }
                    ui.add_space(theme::space::L);
                }
                ui.label(ui::muted(ui::library::plural(
                    season.episodes.len(),
                    "episode",
                )));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    season_download(ui, title, season, &offline, actions);
                    season_watched(ui, title, season, actions);
                });
            });
            ui.add_space(theme::space::L);

            let Some(season) = title.seasons.get(picked) else {
                return;
            };
            for episode in &season.episodes {
                episode_row(ui, art, title, season, episode, &offline, actions);
            }
            ui.add_space(theme::space::L);

            let cast = art
                .metadata
                .get(&title.key)
                .and_then(|record| record.metadata.as_ref())
                .and_then(|found| found.details.as_ref())
                .map(|details| details.cast.clone())
                .unwrap_or_default();
            if !cast.is_empty() {
                ui.add_space(theme::space::L);
                ui::section(ui, "Cast");
                cast_row(ui, art, &title.key, &cast, actions);
                ui.add_space(theme::space::L);
            }

            // Under the episodes rather than over them: it is where to go
            // once this one is done, not what the page is for.
            let similar: Vec<&Title> = similar.iter().filter_map(|key| library.get(key)).collect();
            if !similar.is_empty() {
                ui.add_space(theme::space::L);
                ui::section(ui, "More like this");
                cover_row(ui, art, ("similar", &title.key), None, &similar, actions);
                ui.add_space(theme::space::L);
            }
        });
}

/// The characters, each a round portrait over their name and who voices
/// them. A voice actor opens every title in the library they are in.
fn cast_row(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    key: &str,
    cast: &[pstr_core::metadata::CastMember],
    actions: &mut Vec<Action>,
) {
    const SIZE: f32 = 88.0;
    ui::shelf(ui, ("cast", key), |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::L;
            for (i, member) in cast.iter().enumerate() {
                let tile = ui.allocate_ui(egui::vec2(SIZE + 16.0, SIZE + 52.0), |ui| {
                    ui.vertical_centered(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(SIZE, SIZE), egui::Sense::hover());
                        let picture = member
                            .character_image
                            .as_deref()
                            .or(member.actor_image.as_deref())
                            .and_then(|url| art.picture(url));
                        let painter = ui.painter();
                        match picture {
                            Some(texture) => {
                                let shape = egui::epaint::RectShape::filled(
                                    rect,
                                    egui::CornerRadius::same((SIZE / 2.0) as u8),
                                    egui::Color32::WHITE,
                                )
                                .with_texture(
                                    texture.id(),
                                    super::cover_uv(texture.size_vec2(), rect.size()),
                                );
                                painter.add(shape);
                            }
                            None => {
                                painter.circle_filled(rect.center(), SIZE / 2.0, theme::card());
                            }
                        }
                        ui.add(
                            egui::Label::new(
                                theme::Role::Caption
                                    .rich(&member.character)
                                    .color(theme::text()),
                            )
                            .truncate()
                            .selectable(false),
                        );
                        if let Some(actor) = &member.actor {
                            ui.add(
                                egui::Label::new(ui::muted(actor))
                                    .truncate()
                                    .selectable(false),
                            );
                        }
                        rect
                    })
                    .inner
                });
                // The whole tile opens the actor's page, names included, not only the portrait.
                let Some(actor) = &member.actor else { continue };
                let response = ui
                    .interact(
                        tile.response.rect,
                        ui.id().with(("cast", i)),
                        egui::Sense::click(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(format!("Everything in the library with {actor}"));
                if response.hovered() {
                    ui.painter().circle_stroke(
                        tile.inner.center(),
                        SIZE / 2.0,
                        egui::Stroke::new(2.0, theme::accent()),
                    );
                }
                if response.clicked() {
                    actions.push(Action::Goto(Page::Browse {
                        facet: pstr_core::discover::Facet::Person(actor.clone()),
                        from: Some(key.to_owned()),
                    }));
                }
            }
        });
    });
}

/// A row of covers: every title of the franchise in release order, with
/// `current` marked rather than left out, so the row says where it sits in
/// the story — or, with no `current`, the titles like this one.
fn cover_row(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    id: impl std::hash::Hash + std::fmt::Debug,
    current: Option<&Title>,
    members: &[&Title],
    actions: &mut Vec<Action>,
) {
    ui::shelf(ui, id, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::CARD_GAP;
            for member in members {
                let here = current.is_some_and(|current| member.key == current.key);
                let picture = art.poster(member);
                let found = art
                    .metadata
                    .get(&member.key)
                    .and_then(|record| record.metadata.as_ref());
                let what = found
                    .and_then(|found| found.details.as_ref())
                    .and_then(|details| details.format_label())
                    .unwrap_or(match member.kind {
                        TitleKind::Series => "Series",
                        TitleKind::Film => "Film",
                    });
                let year = found
                    .and_then(|found| found.year)
                    .or(member.year)
                    .map(|year| format!("  ·  {year}"))
                    .unwrap_or_default();
                let response = ui::card(
                    ui,
                    Card {
                        art: picture,
                        name: art.name(member),
                        subtitle: format!("{what}{year}"),
                        progress: member.resume().and_then(|e| e.progress()).map(|v| v as f32),
                        badge: here.then(|| "Here".to_owned()),
                        width: 128.0,
                        aspect: theme::POSTER_ASPECT,
                    },
                );
                if !here && response.clicked() {
                    actions.push(Action::Goto(Page::Title(member.key.clone())));
                }
            }
        });
    });
}

/// A title's poster, at the poster's own shape, with how far into it the
/// viewer is along its foot.
fn poster(ui: &mut egui::Ui, texture: &egui::TextureHandle, progress: Option<f64>) {
    let size = egui::vec2(180.0, 270.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let radius = egui::CornerRadius::same(theme::radius::LG);
    ui.painter()
        .add(theme::tile_shadow(1.0).as_shape(rect, radius));
    ui.painter().add(
        egui::epaint::RectShape::filled(rect, radius, egui::Color32::WHITE)
            .with_texture(texture.id(), ui::cover_uv(texture.size_vec2(), size)),
    );
    if let Some(progress) = progress {
        let track = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - 4.0),
            rect.right_bottom(),
        );
        let painter = ui.painter().with_clip_rect(track);
        painter.rect_filled(rect, radius, egui::Color32::from_black_alpha(150));
        let mut played = rect;
        played.set_width(rect.width() * progress.clamp(0.0, 1.0) as f32);
        theme::accent_fill(&painter, played, radius, 0.0);
    }
}

/// "← Library", as a link rather than a button: it is navigation, and a
/// filled button beside the page's real actions competed with them. `to`
/// names where it goes.
pub(crate) fn back_link(ui: &mut egui::Ui, to: &str) -> egui::Response {
    let text = theme::Role::Label.rich(format!("{}  {to}", egui_phosphor::regular::ARROW_LEFT));
    let response = ui.add(egui::Label::new(text.color(theme::muted())).sense(egui::Sense::click()));
    if response.hovered() {
        ui.painter().hline(
            response.rect.x_range(),
            response.rect.bottom(),
            egui::Stroke::new(1.0, theme::muted()),
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Back (Esc, Alt ←)")
}

/// Download, or delete, a whole season.
/// Mark a whole season seen, or unseen once it all is — for someone who
/// watched it elsewhere and joins the show here.
fn season_watched(ui: &mut egui::Ui, title: &Title, season: &Season, actions: &mut Vec<Action>) {
    let all = season.episodes.iter().all(|episode| episode.is_watched());
    let label = if all {
        "Mark season unwatched"
    } else {
        "Mark season watched"
    };
    if ui::tonal_button(ui, label).clicked() {
        actions.push(Action::MarkEpisodes {
            key: title.key.clone(),
            links: season
                .episodes
                .iter()
                .map(|e| (e.node.share_id.clone(), e.node.link_id.clone()))
                .collect(),
            watched: !all,
            what: format!("{} {}", title.name, season.label()),
        });
    }
}

fn season_download(
    ui: &mut egui::Ui,
    title: &Title,
    season: &Season,
    offline: &OfflineView<'_>,
    actions: &mut Vec<Action>,
) {
    let season_keys: Vec<_> = season.episodes.iter().map(key_of).collect();
    let all_offline = season_keys.iter().all(|key| offline.files.contains(key));
    if all_offline {
        if ui
            .button("Delete season")
            .on_hover_text("Delete the offline copies; keep the online source")
            .clicked()
        {
            actions.push(Action::DeleteOffline(
                season_keys,
                format!("{} {}", title.name, season.label()),
            ));
        }
    } else if ui
        .button("Download season")
        .on_hover_text("Download every episode in this season")
        .clicked()
    {
        actions.push(Action::MakeOffline(
            season
                .episodes
                .iter()
                .map(|episode| PlaybackTarget::new(title, episode))
                .collect(),
        ));
    }
}

/// The still, the name and the one button that matters.
fn header(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    title: &Title,
    on_list: bool,
    offline: &OfflineView<'_>,
    actions: &mut Vec<Action>,
) {
    // Cloned out of the borrow: `art` is held mutably for the card below, and
    // the description is drawn beside it.
    let record = art.metadata.get(&title.key);
    let found = record.and_then(|record| record.metadata.clone());
    let by_hand = record.is_some_and(|record| record.manual);
    // The cover beside the text, and the widest, largest picture there is
    // behind all of it — two textures, because the band is drawn across the
    // whole window and a tile-sized one stretched that far is soft.
    let picture = art.poster(title);
    let wide = art.wide(title);

    // Reserved before the header is laid out, because the band is as tall as
    // whatever the header comes to — a fixed height would clip a long overview
    // on one title and leave a gap under a short one on the next.
    let backdrop = wide
        .as_ref()
        .map(|(texture, _)| (texture.clone(), ui.painter().add(egui::Shape::Noop)));
    let shown = art.name(title).to_owned();

    ui.add_space(theme::space::S);
    ui.horizontal_top(|ui| {
        match &picture {
            // A poster is drawn as a poster. Fitted into the 16:9 tile the grid
            // uses, it was a narrow strip of picture between two bars.
            Some((texture, pstr_core::metadata::ArtShape::Portrait)) => {
                poster(ui, texture, title.resume().and_then(|e| e.progress()));
            }
            _ => {
                ui::card(
                    ui,
                    Card {
                        art: picture,
                        name: &shown,
                        subtitle: String::new(),
                        progress: title.resume().and_then(|e| e.progress()).map(|v| v as f32),
                        badge: None,
                        width: theme::CARD_WIDTH,
                        aspect: theme::CARD_ASPECT,
                    },
                );
            }
        }

        ui.add_space(theme::space::XXL);
        ui.vertical(|ui| {
            // Text in a column a person can read, with the picture to the
            // right of it rather than under it.
            ui.set_max_width(ui.available_width().min(820.0));
            if ui::logo_heading(ui, art, title, 120.0).is_none() {
                ui.label(theme::Role::Hero.rich(&shown).color(theme::text()));
            }
            // Whichever of the share's name and the provider's is not the one
            // shown. Worth showing rather than only replacing: a viewer should
            // be able to tell what the match actually matched, and find the
            // folder it came from.
            let other = found
                .as_ref()
                .map(|found| {
                    if shown == found.name {
                        title.name.clone()
                    } else {
                        found.name.clone()
                    }
                })
                .filter(|other| *other != shown);
            if let Some(other) = other {
                ui.label(ui::muted(format!(
                    "also known as {other}{}",
                    if by_hand { " (chosen by hand)" } else { "" }
                )));
            }
            ui.add_space(theme::space::XS);
            ui.label(ui::muted(meta_line(title, found.as_ref())));
            // What the files are, rather than what the title is: set apart
            // from the line above as small outlined marks, the way a box
            // prints them.
            //
            // After where the title stands, which is the one fact here that
            // changes week to week.
            let quality = title.quality().badges();
            let status = found
                .as_ref()
                .and_then(|found| found.details.as_ref())
                .and_then(|details| status_text(details, now()));
            if !quality.is_empty() || status.is_some() {
                ui.add_space(theme::space::XS);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::space::XS;
                    if let Some((text, live)) = &status {
                        ui::widgets::status_chip(ui, text, *live);
                        ui.add_space(theme::space::XS);
                    }
                    for badge in quality {
                        ui::widgets::spec_badge(ui, badge);
                    }
                });
            }
            ui.add_space(theme::space::L);

            ui.horizontal(|ui| {
                if let Some(episode) = title.next_up() {
                    let resuming = episode.resume_at().is_some();
                    // Where it resumes, in the button that resumes it. As a
                    // label of its own it ended up three controls further
                    // along, after the download buttons, reading as a fact
                    // about those.
                    let at = episode
                        .resume_at()
                        .map(|at| format!("  ·  {}", ui::format_time(at)))
                        .unwrap_or_default();
                    let label = match (resuming, episode.numbering()) {
                        (true, Some(numbering)) => format!("Resume {numbering}{at}"),
                        (true, None) => format!("Resume{at}"),
                        (false, Some(numbering)) if title.kind == TitleKind::Series => {
                            format!("Play {numbering}")
                        }
                        _ => "Play".to_string(),
                    };
                    if ui::accent_button(ui, &label).clicked() {
                        actions.push(Action::Play(PlaybackTarget::new(title, episode)));
                    }
                    // Beside the button it is the alternative to.
                    if resuming
                        && ui::tonal_button(ui, "Start over")
                            .on_hover_text("Play from the beginning")
                            .clicked()
                    {
                        actions.push(Action::Play(PlaybackTarget::from_node(
                            title,
                            &episode.node,
                            None,
                        )));
                    }
                    ui::list_button(ui, &title.key, on_list, actions);
                    let title_keys: Vec<_> = title.episodes().map(key_of).collect();
                    let all_offline = title_keys.iter().all(|key| offline.files.contains(key));
                    if all_offline {
                        if ui::tonal_button(ui, "Delete show")
                            .on_hover_text("Delete the offline copies; keep the online source")
                            .clicked()
                        {
                            actions.push(Action::DeleteOffline(title_keys, title.name.clone()));
                        }
                    } else if ui::tonal_button(ui, "Download show")
                        .on_hover_text("Download every episode for disconnected playback")
                        .clicked()
                    {
                        actions.push(Action::MakeOffline(
                            title
                                .episodes()
                                .map(|episode| PlaybackTarget::new(title, episode))
                                .collect(),
                        ));
                    }
                    let active = offline
                        .downloads
                        .iter()
                        .filter(|item| {
                            item.target.title_key == title.key
                                && matches!(
                                    item.state,
                                    DownloadState::Queued
                                        | DownloadState::Running
                                        | DownloadState::Paused
                                )
                        })
                        .count();
                    if active > 0 {
                        ui.label(ui::muted(format!("{active} downloading")));
                    } else if all_offline {
                        ui.label(ui::muted("Available offline"));
                    }
                }

                // The way out of a match the scorer would not make, or made
                // wrongly. Offered whether or not anything matched: the two
                // cases it exists for are a title with no poster at all and a
                // title wearing someone else's.
                if ui::tonal_button(
                    ui,
                    match &found {
                        Some(_) => "Change match",
                        None => "Match…",
                    },
                )
                .on_hover_text("Search the metadata provider and pick the entry yourself")
                .clicked()
                {
                    actions.push(Action::OpenMatcher(title.key.clone()));
                }
            });

            if let Some(found) = &found {
                ui.add_space(theme::space::L);
                description(ui, found, &title.key, actions);
            }
        });
    });

    if let Some((texture, index)) = backdrop {
        ui.add_space(theme::space::L);
        band(ui, &texture, index, art.mood(title));
    }
}

/// The picture again, across the whole width, behind the header.
///
/// The one piece of this page that had to wait for artwork to exist at all: the
/// providers already hand back a picture and its shape, so a backdrop costs no
/// new request — only somewhere to put it.
///
/// Three shapes over one another, in this order: the picture, a flat veil of the
/// page colour so text laid over it stays readable whatever the still happens to
/// be, and a fade into the page along the bottom so the band ends rather than
/// stops.
fn band(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    index: egui::layers::ShapeIdx,
    mood: Option<egui::Color32>,
) {
    // Full width of the window, not of the content: the panel's own margin is
    // what the band has to reach past, or it reads as a picture in a box.
    const BLEED: f32 = super::PAGE_MARGIN.x;
    let content = ui.min_rect();
    let rect = egui::Rect::from_min_max(
        egui::pos2(ui.max_rect().left() - BLEED, content.top()),
        egui::pos2(ui.max_rect().right() + BLEED, content.bottom()),
    );

    // Past the panel margin, or the bleed is trimmed off at exactly the edge it
    // exists to cross.
    let painter = ui.painter().with_clip_rect(rect);
    // Every layer of the band goes into the one slot reserved before the header
    // was laid out. Painting the veils here, after the fact, would put them over
    // the name and the overview as well as the still — which is the picture
    // being readable at the cost of the text it exists behind.
    let mut layers: Vec<egui::Shape> = vec![
        egui::epaint::RectShape::filled(rect, egui::CornerRadius::ZERO, egui::Color32::WHITE)
            .with_texture(
                texture.id(),
                super::cover_uv(texture.size_vec2(), rect.size()),
            )
            .into(),
    ];
    // Heavier than it looks: a still is arbitrary, and the one it lands on may
    // be a white frame. This is the difference between a hero and an unreadable
    // page.
    //
    // Two coats rather than one heavy one. The first is flat and near-opaque —
    // enough on its own that body text over the busiest frame still reads. The
    // second is the picture's own contrast being knocked back further behind the
    // left half, where the poster, the name and the overview actually sit; the
    // right half keeps more of the still, so the band is still a picture.
    //
    // Heavier still when there is no fade coming, since then these are the only
    // things standing between the text and the picture.
    //
    // Both in the page colour tinted by the picture's own, so each title's
    // page carries its light; the fade at the bottom stays the page's, since
    // that is what it ends in.
    let ramps = theme::ramps_on();
    let veil = theme::mood_veil(mood);
    layers.push(
        egui::epaint::RectShape::filled(
            rect,
            egui::CornerRadius::ZERO,
            // Lighter with the ramps on: the horizontal one below is solid
            // where the text is, so this coat only has to calm the picture's
            // right side, not make it disappear.
            veil.gamma_multiply(if ramps { 0.72 } else { 0.94 }),
        )
        .into(),
    );
    if ramps {
        let text_side = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + rect.width() * 0.72, rect.bottom()),
        );
        layers.push(theme::fade_shape(
            ui.ctx(),
            text_side,
            veil,
            theme::Direction::Horizontal,
            // Solid at the left, where the poster and the text are.
            false,
        ));
    }

    // A viewer with ramps off is on a panel that bands them; a 120-point ramp
    // of one hue is exactly the case that shows it. The band then ends at its
    // own edge, which is the honest version of the same thing.
    if ramps {
        let fade =
            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 120.0), rect.max);
        layers.push(theme::fade_shape(
            ui.ctx(),
            fade,
            theme::background(),
            theme::Direction::Vertical,
            // Solid at the bottom, where the page it hands over to is.
            true,
        ));
    }

    painter.set(index, egui::Shape::Vec(layers));
}

/// What the provider had to say. Only ever drawn under a match.
///
/// Its genres, studios and director are chips that open every title in the
/// library sharing them.
fn description(ui: &mut egui::Ui, found: &TitleMetadata, key: &str, actions: &mut Vec<Action>) {
    use pstr_core::discover::Facet;
    let browse = |facet: Facet| {
        Action::Goto(Page::Browse {
            facet,
            from: Some(key.to_owned()),
        })
    };
    let details = found.details.as_ref();
    let tags = details
        .map(|details| details.tags.as_slice())
        .unwrap_or_default();
    if !found.genres.is_empty() || !tags.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for genre in &found.genres {
                if ui::widgets::link_chip(ui, genre)
                    .on_hover_text(format!("Every {genre} title"))
                    .clicked()
                {
                    actions.push(browse(Facet::Genre(genre.clone())));
                }
            }
            // After the genres and quieter than them: a tag is the
            // provider's community describing it, not what it is filed as.
            for tag in tags.iter().take(5) {
                ui.label(ui::muted(tag));
            }
        });
        ui.add_space(theme::space::M);
    }
    // Who made it, out of the grey line and onto chips, so the names lead
    // somewhere.
    let studios = details
        .map(|details| &details.studios[..details.studios.len().min(2)])
        .unwrap_or_default();
    let directors = details
        .map(|details| &details.directors[..details.directors.len().min(1)])
        .unwrap_or_default();
    if !studios.is_empty() || !directors.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (label, names, facet) in [
                ("Studio", studios, Facet::Studio as fn(String) -> Facet),
                ("Director", directors, Facet::Person),
            ] {
                if names.is_empty() {
                    continue;
                }
                ui.label(ui::muted(label));
                for name in names {
                    if ui::widgets::link_chip(ui, name)
                        .on_hover_text(format!("Everything in the library by {name}"))
                        .clicked()
                    {
                        actions.push(browse(facet(name.clone())));
                    }
                }
                ui.add_space(theme::space::S);
            }
        });
        ui.add_space(theme::space::M);
    }
    if let Some(overview) = &found.overview {
        // Four lines, then "More". A provider's synopsis runs to three
        // paragraphs on a long show, and all of them pushed the episodes —
        // what the page is for — below the fold.
        const LINES: usize = 4;
        let id = ui.id().with(("overview", &found.remote_id));
        let open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
        let mut job = egui::text::LayoutJob::simple(
            overview.clone(),
            theme::Role::Label.font(),
            theme::text(),
            ui.available_width(),
        );
        if !open {
            job.wrap.max_rows = LINES;
            job.wrap.overflow_character = Some('…');
        }
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let clipped = galley.elided;
        ui.label(galley);
        if clipped || open {
            let toggle = ui.add(
                egui::Label::new(
                    theme::Role::Caption
                        .rich(if open { "Less" } else { "More" })
                        .color(theme::accent()),
                )
                .sense(egui::Sense::click()),
            );
            if toggle
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                ui.data_mut(|data| data.insert_temp(id, !open));
            }
        }
    }
    let trailer = details
        .and_then(|details| details.trailer.as_ref())
        .and_then(|trailer| trailer.url());
    if found.url.is_some() || trailer.is_some() {
        ui.add_space(theme::space::M);
        ui.horizontal(|ui| {
            // In the browser: the app itself never requests the trailer.
            if let Some(trailer) = &trailer {
                ui.hyperlink_to(
                    ui::muted(format!("{}  Trailer", egui_phosphor::regular::FILM_STRIP)),
                    trailer,
                )
                .on_hover_text("Opens in your browser");
                ui.add_space(theme::space::L);
            }
            if let Some(url) = &found.url {
                ui.hyperlink_to(
                    ui::muted(format!("More on {}", found.provider.label())),
                    url,
                );
            }
        });
    }
}

/// The grey line under the name: what the library knows, plus what the provider
/// added. The library's own counts come first — they describe the files that are
/// actually there, which is what a viewer is deciding about.
fn meta_line(title: &Title, found: Option<&TitleMetadata>) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(
        match title.kind {
            TitleKind::Series => "Series",
            TitleKind::Film => "Film",
        }
        .to_string(),
    );
    if let Some(year) = title.year {
        parts.push(year.to_string());
    }
    // A film's running time is what it costs to start it. A series' is its
    // episodes', which each row says.
    if title.kind == TitleKind::Film
        && let Some(minutes) = found
            .and_then(|found| found.details.as_ref())
            .and_then(|details| details.runtime)
    {
        parts.push(ui::format_runtime(minutes));
    }
    if title.kind == TitleKind::Series {
        if title.seasons.len() > 1 {
            parts.push(ui::library::plural(title.seasons.len(), "season"));
        }
        parts.push(ui::library::plural(title.episode_count(), "episode"));
    }
    let watched = title.watched_count();
    if watched > 0 {
        parts.push(format!("{watched} watched"));
    }

    if let Some(season) = found
        .and_then(|found| found.details.as_ref())
        .and_then(|details| details.season_label())
    {
        parts.push(season);
    }
    if let Some(found) = found {
        if let Some(rating) = found.rating {
            parts.push(format!("{} {rating:.1}", egui_phosphor::regular::STAR));
        }
        // Only when it disagrees with what is on disk: "25 episodes · 25
        // episodes" tells nobody anything, but "12 episodes · 25 on AniList"
        // says the share is missing half a season.
        if let Some(total) = found.episodes
            && title.kind == TitleKind::Series
            && total as usize != title.episode_count()
        {
            parts.push(format!("{total} on {}", found.provider.label()));
        }
    }
    parts.join("  ·  ")
}

/// The most seasons picked from a row of pills; past it, a list.
const SEASON_PILLS: usize = 5;

/// Unix seconds, now.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// What the status chip says — `Airing · episode 8 Sat 1 Nov, 14:00`,
/// `Finished` — and whether more is coming.
fn status_text(details: &pstr_core::metadata::TitleDetails, now: i64) -> Option<(String, bool)> {
    let next = details.next_airing(now);
    let label = details.status_label().or(next.map(|_| "Airing"))?;
    let live = next.is_some() || details.is_airing();
    Some(match next {
        Some((episode, at)) => {
            let when = chrono::DateTime::from_timestamp(at, 0)
                .map(|at| {
                    at.with_timezone(&chrono::Local)
                        .format("%a %-d %b, %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            (format!("{label}  ·  episode {episode} {when}"), live)
        }
        None => (label.to_owned(), live),
    })
}

/// How big an episode's still is drawn.
const STILL: egui::Vec2 = egui::vec2(160.0, 90.0);

/// One row per file: its still, its name and what it is about, and the two
/// things that can be done to it besides playing it.
fn episode_row(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    title: &Title,
    season: &Season,
    episode: &Episode,
    offline: &OfflineView<'_>,
    actions: &mut Vec<Action>,
) {
    // Cloned out before `ui` borrows: the row draws while `art` is held.
    let found = art.episode(&title.key, episode).map(|found| {
        (
            found.name.clone(),
            found.overview.clone(),
            found.air_date.clone(),
            found.runtime,
        )
    });
    let watched = episode.is_watched();
    let row_id = ui.id().with(("episode", &episode.node.link_id));

    let frame = egui::Frame::new()
        .corner_radius(egui::CornerRadius::same(theme::radius::LG))
        .inner_margin(egui::Margin::same(8));
    let mut prepared = frame.begin(ui);
    {
        let ui = &mut prepared.content_ui;
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            still(ui, art, title, episode, watched, STILL, actions);
            ui.add_space(theme::space::S);

            let controls = 96.0;
            let text_width = (ui.available_width() - controls).max(120.0);
            ui.vertical(|ui| {
                ui.set_width(text_width);
                ui.spacing_mut().item_spacing.y = 3.0;

                // The provider's name for the episode, when there is one:
                // "The Immortal Legion" reads as an episode, and
                // "[Reaktor] … E57 v2 [1080p][x265].mkv" reads as a filename.
                let numbering = episode
                    .numbering()
                    .or_else(|| season.number.map(|number| format!("S{number:02}")))
                    .unwrap_or_default();
                let name = found
                    .as_ref()
                    .and_then(|(name, ..)| name.clone())
                    .unwrap_or_else(|| episode.detail().to_string());
                let mut job = egui::text::LayoutJob::default();
                if !numbering.is_empty() {
                    job.append(
                        &format!("{numbering}   "),
                        0.0,
                        egui::TextFormat::simple(
                            egui::FontId::new(theme::Role::Label.size(), theme::semibold()),
                            theme::muted(),
                        ),
                    );
                }
                job.append(
                    &name,
                    0.0,
                    egui::TextFormat::simple(
                        theme::Role::Body.font(),
                        if watched {
                            theme::muted()
                        } else {
                            theme::text()
                        },
                    ),
                );
                job.wrap.max_rows = 1;
                job.wrap.overflow_character = Some('…');
                ui.add(egui::Label::new(job).truncate())
                    .on_hover_text(&episode.node.name);

                ui.label(ui::muted(details(
                    episode,
                    found.as_ref().and_then(|f| f.2.as_deref()),
                    found.as_ref().and_then(|f| f.3),
                )));

                if let Some((_, Some(overview), ..)) = &found {
                    let mut job = egui::text::LayoutJob::simple(
                        overview.clone(),
                        theme::Role::Caption.font(),
                        theme::muted(),
                        text_width,
                    );
                    job.wrap.max_rows = 2;
                    job.wrap.overflow_character = Some('…');
                    ui.add(egui::Label::new(job)).on_hover_text(overview);
                }
            });

            // A fixed height, the still's. `with_layout` would hand the
            // buttons all the height left in the scroll area's viewport and
            // centre them in it, so whichever row sat highest on screen
            // stretched down to the bottom of the window.
            let cluster = egui::vec2(ui.available_width(), STILL.y);
            let layout = egui::Layout::right_to_left(egui::Align::Center);
            ui.allocate_ui_with_layout(cluster, layout, |ui| {
                let mark = ui::widgets::watched_mark(ui, watched).on_hover_text(if watched {
                    "Mark unwatched. Right-click for more."
                } else {
                    "Mark watched. Right-click for more."
                });
                // Joining a show partway is the usual reason to mark
                // anything, and one tick per earlier episode is a chore.
                mark.context_menu(|ui| {
                    if ui.button("Mark watched up to here").clicked() {
                        let links = title
                            .episodes()
                            .take(
                                title
                                    .position_of(&episode.node.share_id, &episode.node.link_id)
                                    .map_or(0, |at| at + 1),
                            )
                            .map(|e| (e.node.share_id.clone(), e.node.link_id.clone()))
                            .collect();
                        actions.push(Action::MarkEpisodes {
                            key: title.key.clone(),
                            links,
                            watched: true,
                            what: format!("{} up to {}", title.name, episode.label()),
                        });
                    }
                });
                if mark.clicked() {
                    actions.push(Action::SetWatched {
                        share_id: episode.node.share_id.clone(),
                        link_id: episode.node.link_id.clone(),
                        watched: !watched,
                        duration: episode.watch.and_then(|watch| watch.duration_secs),
                    });
                }
                ui.add_space(theme::space::S);
                download(ui, title, episode, offline, actions);
            });
        });
    }
    // The row lights up under the pointer, so it is clear which file the
    // buttons on the right belong to in a list of thirty.
    let response = prepared.allocate_space(ui);
    let lit = ui.ctx().animate_bool_with_time(
        row_id,
        response.hovered() || response.contains_pointer(),
        theme::motion::HOVER,
    );
    prepared.frame.fill = theme::card().gamma_multiply(0.35 + 0.65 * lit);
    prepared.paint(ui);
    ui.add_space(theme::space::XXS);
}

/// The grey line under an episode's name: when it aired, how long it runs,
/// how big it is, and where it would resume.
fn details(episode: &Episode, air_date: Option<&str>, runtime: Option<u32>) -> String {
    let mut parts = Vec::new();
    if let Some(at) = episode.resume_at() {
        let left = episode
            .watch
            .and_then(|watch| watch.duration_secs)
            .map(|duration| format!(" · {} left", ui::format_time(duration - at)))
            .unwrap_or_default();
        parts.push(format!("Resume at {}{left}", ui::format_time(at)));
    } else if episode.is_watched() {
        parts.push("Watched".into());
    }
    if let Some(date) = air_date {
        parts.push(date.to_owned());
    }
    if let Some(minutes) = runtime {
        parts.push(ui::format_runtime(minutes));
    }
    match episode.node.size {
        // A file the share says is empty is one that will not play — an upload
        // that never finished, usually. Worth saying on the row rather than
        // only when a click on it comes back with "has no content".
        Some(0) => parts.push("empty — this file cannot be played".into()),
        Some(size) => parts.push(ui::format_size(size)),
        None => {}
    }
    parts.join("  ·  ")
}

/// The still, which is also the play button.
///
/// An explicit target rather than a clickable row: the row also carries the
/// watched mark and the download button, and a click that meant either of
/// those must never be one that starts a 1.4 GiB stream instead.
pub(crate) fn still(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    title: &Title,
    episode: &Episode,
    watched: bool,
    size: egui::Vec2,
    actions: &mut Vec<Action>,
) {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let response = response.named(format!("Play {}", episode.label()));
    if !ui.is_rect_visible(rect) {
        return;
    }
    let radius = egui::CornerRadius::same(theme::radius::MD);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, theme::card_hover());
    // Asked for only once the row is on screen: a season of twenty-five is
    // twenty-five requests, and most of them are never scrolled to.
    let texture = art.still(&title.key, episode);
    // Faded in like a tile's art, so a season's stills settle rather than pop.
    let art_in = ui.ctx().animate_bool_with_time(
        response.id.with("art"),
        texture.is_some(),
        theme::motion::ARRIVE,
    );
    if let Some(texture) = texture {
        let tint = if watched {
            egui::Color32::from_gray(150)
        } else {
            egui::Color32::WHITE
        }
        .gamma_multiply(art_in);
        painter.add(
            egui::epaint::RectShape::filled(rect, radius, tint)
                .with_texture(texture.id(), ui::cover_uv(texture.size_vec2(), rect.size())),
        );
    }

    if response.gained_focus() {
        response.scroll_to_me(None);
    }
    let hover = ui.ctx().animate_bool_with_time(
        response.id,
        response.hovered() || response.has_focus(),
        theme::motion::HOVER,
    );
    if hover > 0.0 {
        painter.rect_filled(
            rect,
            radius,
            egui::Color32::from_black_alpha((90.0 * hover) as u8),
        );
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(2.0, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    // A play mark, always faintly there and full under the pointer.
    let centre = rect.center();
    painter.circle_filled(
        centre,
        18.0,
        egui::Color32::from_black_alpha((110.0 + 80.0 * hover) as u8),
    );
    let ink = egui::Color32::WHITE.gamma_multiply(0.75 + 0.25 * hover);
    let r = 7.0;
    painter.add(egui::Shape::convex_polygon(
        vec![
            centre + egui::vec2(-r * 0.6, -r),
            centre + egui::vec2(r, 0.0),
            centre + egui::vec2(-r * 0.6, r),
        ],
        ink,
        egui::Stroke::NONE,
    ));

    if let Some(progress) = episode.progress() {
        let track = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - 4.0),
            rect.right_bottom(),
        );
        let painter = painter.with_clip_rect(track);
        painter.rect_filled(rect, radius, egui::Color32::from_black_alpha(150));
        let mut played = rect;
        played.set_width(rect.width() * progress.clamp(0.0, 1.0) as f32);
        theme::accent_fill(&painter, played, radius, 0.0);
    }

    // Seen, said on the picture as well as by the mark at the row's end: a
    // season is scanned by its stills, and a dimmed one alone read as one that
    // had not loaded.
    if watched {
        let corner = rect.right_top() + egui::vec2(-14.0, 14.0);
        painter.circle_filled(corner, 9.0, theme::accent());
        painter.text(
            corner,
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::CHECK,
            theme::Role::Micro.font(),
            theme::on_accent(),
        );
    }

    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if episode.resume_at().is_some() {
            "Resume"
        } else {
            "Play"
        })
        .clicked()
    {
        actions.push(Action::Play(PlaybackTarget::new(title, episode)));
    }
}

/// One button that is the file's whole offline story: download it, see it
/// coming, pause it, or delete the copy.
fn download(
    ui: &mut egui::Ui,
    title: &Title,
    episode: &Episode,
    offline: &OfflineView<'_>,
    actions: &mut Vec<Action>,
) {
    use ui::widgets::{DownloadGlyph, download_button};

    let key = key_of(episode);
    let current = offline.downloads.iter().find(|item| item.key == key);
    if offline.files.contains(&key) {
        if download_button(ui, DownloadGlyph::Done)
            .on_hover_text("Available offline. Click to delete the copy; the share keeps the file.")
            .clicked()
        {
            actions.push(Action::DeleteOffline(vec![key], episode.label()));
        }
        return;
    }
    match current.map(|item| (&item.state, item.percent())) {
        Some((DownloadState::Running | DownloadState::Queued, fraction)) => {
            if download_button(ui, DownloadGlyph::Progress(fraction))
                .on_hover_text(format!(
                    "Downloading, {:.0}%. Click to pause.",
                    fraction * 100.0
                ))
                .clicked()
            {
                actions.push(Action::PauseDownload(key));
            }
        }
        Some((
            DownloadState::Paused | DownloadState::Cancelled | DownloadState::Failed(_),
            fraction,
        )) => {
            if download_button(ui, DownloadGlyph::Paused(fraction))
                .on_hover_text(format!(
                    "Stopped at {:.0}%. Click to carry on.",
                    fraction * 100.0
                ))
                .clicked()
            {
                actions.push(Action::ResumeDownload(key));
            }
        }
        Some((DownloadState::Completed, _)) => {
            download_button(ui, DownloadGlyph::Done).on_hover_text("Available offline");
        }
        None => {
            if download_button(ui, DownloadGlyph::Download)
                .on_hover_text("Download, to play without a connection")
                .clicked()
            {
                actions.push(Action::MakeOffline(vec![PlaybackTarget::new(
                    title, episode,
                )]));
            }
        }
    }
}

fn key_of(episode: &Episode) -> DownloadKey {
    DownloadKey {
        share_id: episode.node.share_id.clone(),
        link_id: episode.node.link_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn details(status: &str, next: Option<(u32, i64)>) -> pstr_core::metadata::TitleDetails {
        pstr_core::metadata::TitleDetails {
            status: Some(status.to_owned()),
            next_episode: next.map(|(episode, _)| episode),
            next_airing_at: next.map(|(_, at)| at),
            ..Default::default()
        }
    }

    #[test]
    fn the_status_chip_says_when_the_next_episode_airs_and_only_while_it_is_ahead() {
        let (text, live) = status_text(&details("RELEASING", Some((8, 2_000))), 1_000).unwrap();
        assert!(text.starts_with("Airing  ·  episode 8 "), "{text}");
        assert!(live);
        // Fetched a while ago: the date has passed, so it says nothing of it.
        let (text, live) = status_text(&details("RELEASING", Some((8, 500))), 1_000).unwrap();
        assert_eq!(text, "Airing");
        assert!(live);
        assert_eq!(
            status_text(&details("FINISHED", None), 1_000),
            Some(("Finished".to_owned(), false))
        );
        assert_eq!(
            status_text(&pstr_core::metadata::TitleDetails::default(), 0),
            None
        );
    }
}
