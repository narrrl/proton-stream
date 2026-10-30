//! The library page: what to watch, as a wall of stills.

use std::collections::HashSet;

use pstr_core::appearance::Tiles;
use pstr_core::library::{Library, Title, TitleKind};

use crate::app::{Action, Filter, LibraryView, Page, Sort};
use crate::playback::PlaybackTarget;
use crate::theme;
use crate::ui::Art;
use crate::ui::{self, Card};

/// What the page lists, and what it was asked for.
pub struct Shelves<'a> {
    pub library: &'a Library,
    /// The search and the shelf, already worked out — see [`LibraryView`].
    pub view: &'a LibraryView,
    /// Whether the catalog has been read yet.
    pub loaded: bool,
    pub search: &'a str,
    /// The grid's shape.
    pub tiles: Tiles,
    /// What a crawl in progress has found, if one is.
    pub crawl: Option<&'a str>,
    /// Whether to offer posters and synopses: they are off, and the offer has
    /// not been turned down.
    pub offer_metadata: bool,
}

pub fn show(ui: &mut egui::Ui, art: &mut Art<'_>, shelves: Shelves<'_>, actions: &mut Vec<Action>) {
    let Shelves {
        library,
        view,
        loaded,
        search,
        tiles,
        crawl,
        offer_metadata,
    } = shelves;
    let searching = !search.trim().is_empty();
    let featured = if searching {
        Vec::new()
    } else {
        featured(library, view, art.metadata, today())
    };
    // The panel leaves the top margin to this page, for the banner to start
    // under the bar.
    if featured.is_empty() {
        ui.add_space(ui::PAGE_MARGIN.y);
    }
    if !loaded {
        return placeholder(ui);
    }
    if library.is_empty() {
        return empty_state(ui, crawl, actions);
    }

    let pick = |indices: &[usize]| -> Vec<&Title> {
        indices
            .iter()
            .filter_map(|&index| library.titles.get(index))
            .collect()
    };
    let matches = pick(&view.matches);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !featured.is_empty() {
                hero(ui, art, &featured, &view.listed, actions);
                ui.add_space(theme::space::L);
            }
            if !searching && offer_metadata {
                metadata_offer(ui, actions);
                ui.add_space(theme::space::XL);
            }
            if !searching {
                let resumable = pick(&view.resumable);
                if !resumable.is_empty() {
                    ui::section(ui, "Continue watching");
                    continue_row(ui, art, &resumable, view, actions);
                    ui.add_space(theme::space::XL);
                }
                // Only over the whole library: under a filter, a shelf of
                // titles the grid below is hiding reads as the filter not
                // working.
                if view.filter == Filter::All {
                    for (label, indices) in &view.shelves {
                        ui::section(ui, label);
                        shelf_row(ui, art, label, &pick(indices), view, tiles, actions);
                        ui.add_space(theme::space::XL);
                    }
                }
            }

            if searching && !view.found.is_empty() {
                found(ui, library, &view.found, art, actions);
                // Found as an episode or a person but not by name: no empty
                // heading over no titles.
                if matches.is_empty() {
                    return;
                }
            }

            ui.horizontal(|ui| {
                ui::section(
                    ui,
                    &if searching {
                        format!(
                            "{} matching {:?}",
                            plural(matches.len(), "title"),
                            search.trim()
                        )
                    } else {
                        plural(matches.len(), "title")
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    shelf_controls(ui, view, actions);
                });
            });
            ui.add_space(theme::space::S);

            if matches.is_empty() {
                ui.add_space(theme::space::M);
                ui.label(ui::muted(if searching {
                    "Nothing here by that name."
                } else {
                    "Nothing in the library fits that filter."
                }));
                return;
            }

            grid(ui, art, &matches, view, tiles, true, actions);
        });
}

/// What a search found besides titles, above the titles it found: the
/// people, studios and genres it names as chips, then episodes by name.
fn found(
    ui: &mut egui::Ui,
    library: &Library,
    found: &pstr_core::discover::Found,
    art: &Art<'_>,
    actions: &mut Vec<Action>,
) {
    let chips = |ui: &mut egui::Ui,
                 heading: &str,
                 facets: &[(pstr_core::discover::Facet, usize)],
                 actions: &mut Vec<Action>| {
        if facets.is_empty() {
            return;
        }
        ui::section(ui, heading);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for (facet, count) in facets {
                if ui::widgets::link_chip(ui, &format!("{}  ·  {count}", facet.name()))
                    .on_hover_text(format!("{}: {}", facet.kind(), plural(*count, "title")))
                    .clicked()
                {
                    actions.push(Action::Goto(Page::Browse {
                        facet: facet.clone(),
                        from: None,
                    }));
                }
            }
        });
        ui.add_space(theme::space::XL);
    };
    chips(ui, "People and studios", &found.people, actions);
    chips(ui, "Genres", &found.genres, actions);

    if found.episodes.is_empty() {
        return;
    }
    ui::section(ui, "Episodes");
    for hit in &found.episodes {
        let Some(title) = library.get(&hit.title_key) else {
            continue;
        };
        let Some(episode) = title.episodes().find(|episode| {
            episode.node.link_id == hit.link_id && episode.node.share_id == hit.share_id
        }) else {
            continue;
        };
        let text = format!(
            "{}  {}  ·  {}  ·  {}",
            egui_phosphor::regular::PLAY,
            art.name(title),
            hit.label,
            hit.name
        );
        let response = ui
            .add(egui::Button::selectable(false, text).frame_when_inactive(false))
            .on_hover_text("Play this episode");
        if response.clicked() {
            actions.push(Action::Play(PlaybackTarget::from_node(
                title,
                &episode.node,
                None,
            )));
        }
    }
    ui.add_space(theme::space::XL);
}

/// What a chip on a title page opened.
pub struct Browsing<'a> {
    pub facet: &'a pstr_core::discover::Facet,
    /// The title it was opened from, which is where back goes.
    pub from: Option<&'a str>,
}

/// Every title sharing a genre, a studio or a director, as the library grid.
pub fn browse(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    library: &Library,
    browsing: Browsing<'_>,
    view: &LibraryView,
    tiles: Tiles,
    actions: &mut Vec<Action>,
) {
    let Browsing { facet, from } = browsing;
    let back = from
        .and_then(|key| library.get(key))
        .map_or("Library", |title| art.name(title))
        .to_owned();
    if ui::title::back_link(ui, &back).clicked() {
        actions.push(Action::Back);
    }
    ui.add_space(theme::space::S);
    let keys = facet.titles(&library.titles, art.metadata);
    let titles: Vec<&Title> = keys.iter().filter_map(|key| library.get(key)).collect();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(ui::muted(facet.kind()));
            ui.label(theme::Role::Display.rich(facet.name()).color(theme::text()));
            ui.label(ui::muted(plural(titles.len(), "title")));
            ui.add_space(theme::space::L);
            if titles.is_empty() {
                // A recrawl or a new match took the last one away.
                ui.label(ui::muted("Nothing in the library has this any more."));
                return;
            }
            grid(ui, art, &titles, view, tiles, false, actions);
        });
}

/// Days since the epoch: what the featured titles turn over on.
fn today() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() / 86_400)
}

/// How many titles the banner turns through.
const FEATURED: usize = 5;

/// How long each title holds the banner before the next takes it, in seconds.
const HOLD: f64 = 8.0;

/// How long one title takes to give way to the next. Slower than artwork
/// arriving: this is the whole banner changing, and a quick one reads as a
/// cut.
const TURN: f32 = 0.6;

/// The titles the banner turns through: whatever was watched last first, so
/// the top of the page is one click from carrying on, then titles a provider
/// has a backdrop for, starting from a different one each day — the same
/// order every time the page is opened today, rather than a new one per frame.
fn featured<'a>(
    library: &'a Library,
    view: &LibraryView,
    metadata: &std::collections::HashMap<String, pstr_core::metadata::MetadataRecord>,
    day: u64,
) -> Vec<&'a Title> {
    let mut picked: Vec<&Title> = view
        .resumable
        .first()
        .and_then(|&index| library.titles.get(index))
        .into_iter()
        .collect();
    let pictured: Vec<&Title> = library
        .titles
        .iter()
        .filter(|title| {
            metadata
                .get(&title.key)
                .and_then(|record| record.metadata.as_ref())
                .is_some_and(|found| found.wide_art().is_some())
        })
        .collect();
    if pictured.is_empty() {
        return picked;
    }
    let start = (day % pictured.len() as u64) as usize;
    for title in pictured.iter().cycle().skip(start).take(pictured.len()) {
        if picked.len() == FEATURED {
            break;
        }
        if !picked.iter().any(|already| already.key == title.key) {
            picked.push(title);
        }
    }
    picked
}

/// Which of the featured titles the banner is on, and since when. Kept in
/// egui's memory: it is the banner's own business, and gone with the window.
#[derive(Clone, Copy)]
struct Turn {
    shown: usize,
    /// The one it is turning from, while the cross-fade lasts.
    leaving: Option<usize>,
    /// `InputState::time` at the last turn.
    since: f64,
}

/// The banner across the top of the library: one title at a time, large, with
/// what it is and a way to play it, turning to the next every [`HOLD`]
/// seconds unless the pointer is over it.
///
/// Edge to edge rather than inside the page's margin, and square: a banner
/// in the margin with rounded corners read as one more card.
fn hero(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    titles: &[&Title],
    listed: &HashSet<String>,
    actions: &mut Vec<Action>,
) {
    let height = (ui.ctx().content_rect().height() * 0.5).clamp(280.0, 520.0);
    let (column, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let rect = egui::Rect::from_min_max(
        egui::pos2(column.left() - ui::PAGE_MARGIN.x, column.top()),
        egui::pos2(column.right() + ui::PAGE_MARGIN.x, column.bottom()),
    );
    if titles.is_empty() || !ui.is_rect_visible(rect) {
        return;
    }
    let id = ui.id().with("hero");
    let response = ui.interact(rect, id, egui::Sense::click());
    let now = ui.input(|input| input.time);
    let count = titles.len();
    let mut turn = ui.data(|data| data.get_temp::<Turn>(id)).unwrap_or(Turn {
        shown: 0,
        leaving: None,
        since: now,
    });
    // The list is worked out again every frame, and can be shorter than the
    // last time — a title finished, a match removed.
    turn.shown %= count;
    turn.leaving = turn.leaving.filter(|&index| index < count);

    let hovered = ui.rect_contains_pointer(rect);
    let elapsed = now - turn.since;
    if hovered && elapsed > f64::from(TURN) {
        // Held where it is while it is being looked at, and the count starts
        // again once the pointer leaves rather than turning the moment it does.
        turn.since = now - f64::from(TURN);
    } else if count > 1 && elapsed >= HOLD {
        turn = Turn {
            shown: (turn.shown + 1) % count,
            leaving: Some(turn.shown),
            since: now,
        };
    }
    let progress = theme::motion::ease_out(((now - turn.since) / f64::from(TURN)) as f32);
    if progress >= 1.0 {
        turn.leaving = None;
    }
    let title = titles[turn.shown];

    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, egui::CornerRadius::ZERO, theme::card());
    // A measure that reads as a column, not a line across a wide window.
    let text_right = column.left() + (column.width() * 0.45).clamp(360.0, 720.0);
    let mut veil = None;
    if let Some(leaving) = turn.leaving {
        veil = Some(backdrop(
            ui,
            &painter,
            art,
            titles[leaving],
            rect,
            text_right,
            1.0,
        ));
    }
    let arriving = if turn.leaving.is_some() {
        progress
    } else {
        1.0
    };
    let shown_veil = backdrop(ui, &painter, art, title, rect, text_right, arriving);
    let veil = veil.map_or(shown_veil, |from| from.lerp_to_gamma(shown_veil, progress));

    // The same two coats as the title page's band, for the same reason: a
    // still is arbitrary, and the text over it has to read whatever it is.
    // Tinted with the picture's own colour, so each title's banner has a light
    // of its own; the fade at the bottom is the page's, because that is what
    // it hands over to.
    if theme::ramps_on() {
        painter.rect_filled(rect, egui::CornerRadius::ZERO, veil.gamma_multiply(0.3));
        let text_side = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(
                text_right + 0.5 * (rect.right() - text_right),
                rect.bottom(),
            ),
        );
        painter.add(theme::fade_shape(
            ui.ctx(),
            text_side,
            veil,
            theme::Direction::Horizontal,
            false,
        ));
        let bottom =
            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 120.0), rect.max);
        painter.add(theme::fade_shape(
            ui.ctx(),
            bottom,
            theme::background(),
            theme::Direction::Vertical,
            true,
        ));
    } else {
        painter.rect_filled(rect, egui::CornerRadius::ZERO, veil.gamma_multiply(0.82));
    }

    let inner = egui::Rect::from_min_max(
        egui::pos2(column.left(), column.top() + theme::space::XL),
        egui::pos2(text_right, column.bottom() - 40.0),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::bottom_up(egui::Align::Min)),
        |ui| {
            ui.multiply_opacity(arriving);
            hero_text(ui, art, title, listed.contains(&title.key), actions);
        },
    );

    if count > 1
        && let Some(picked) = dots(ui, id, column, titles, turn.shown)
    {
        if picked != turn.shown {
            turn = Turn {
                shown: picked,
                leaving: Some(turn.shown),
                since: now,
            };
        }
    } else if response.clicked() {
        actions.push(Action::Goto(Page::Title(title.key.clone())));
    }
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, art.name(title)));
    ui.data_mut(|data| data.insert_temp(id, turn));

    if turn.leaving.is_some() {
        ui.ctx().request_repaint();
    } else if count > 1 && !hovered {
        let left = (HOLD - (now - turn.since)).max(0.0);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(left));
    }
}

/// One title's picture across the banner, at `alpha`, and the colour its veils
/// are to be — the page's until the picture has arrived, then the picture's
/// own, eased in with it.
fn backdrop(
    ui: &egui::Ui,
    painter: &egui::Painter,
    art: &mut Art<'_>,
    title: &Title,
    rect: egui::Rect,
    text_right: f32,
    alpha: f32,
) -> egui::Color32 {
    let picture = art.wide(title);
    let art_in = ui.ctx().animate_bool_with_time(
        ui.id().with(("hero-art", &title.key)),
        picture.is_some(),
        theme::motion::ARRIVE,
    );
    if art_in < 1.0 {
        ui::placeholder(painter, rect, "", alpha * (1.0 - art_in));
    }
    if let Some((texture, _)) = &picture {
        painter.add(
            egui::epaint::RectShape::filled(
                rect,
                egui::CornerRadius::ZERO,
                egui::Color32::WHITE.gamma_multiply(alpha * art_in),
            )
            .with_texture(texture.id(), ui::cover_uv(texture.size_vec2(), rect.size())),
        );
    }

    // With no wide art the banner is the poster's wash, so the poster itself
    // stands at the right, where wide art keeps its subject — and only where
    // it clears the text.
    if !art.has_wide_art(title)
        && let Some((poster, pstr_core::metadata::ArtShape::Portrait)) = art.poster(title)
    {
        const INSET: f32 = 40.0;
        let height = rect.height() - 2.0 * INSET - 40.0;
        let size = egui::vec2(height * 2.0 / 3.0, height);
        let at = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 2.0 * INSET - size.x, rect.top() + INSET),
            size,
        );
        if at.left() > text_right + INSET {
            painter.add(
                theme::tile_shadow(alpha * art_in)
                    .as_shape(at, egui::CornerRadius::same(theme::radius::MD)),
            );
            painter.add(
                egui::epaint::RectShape::filled(
                    at,
                    egui::CornerRadius::same(theme::radius::MD),
                    egui::Color32::WHITE.gamma_multiply(alpha * art_in),
                )
                .with_texture(
                    poster.id(),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                ),
            );
        }
    }
    theme::background().lerp_to_gamma(theme::mood_veil(art.mood(title)), art_in)
}

/// What the banner says about one title, from the bottom up: how far into it
/// the viewer is, the buttons, the synopsis, the facts and the name.
fn hero_text(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    title: &Title,
    on_list: bool,
    actions: &mut Vec<Action>,
) {
    let found = art
        .metadata
        .get(&title.key)
        .and_then(|record| record.metadata.clone());
    if let Some(episode) = title.resume()
        && let Some(progress) = episode.progress()
    {
        ui.horizontal(|ui| {
            ui::progress_bar(ui, progress as f32, 180.0);
            if let Some(left) = time_left(episode) {
                ui.add_space(theme::space::S);
                ui.label(ui::muted(left));
            }
        });
        ui.add_space(theme::space::M);
    }
    ui.horizontal(|ui| {
        if let Some(episode) = title.next_up() {
            let label = match (title.resume(), episode.numbering()) {
                (Some(_), Some(numbering)) => {
                    format!("{}  Resume {numbering}", egui_phosphor::regular::PLAY)
                }
                (Some(_), None) => format!("{}  Resume", egui_phosphor::regular::PLAY),
                (None, _) => format!("{}  Play", egui_phosphor::regular::PLAY),
            };
            if ui::accent_button(ui, &label).clicked() {
                actions.push(Action::Play(PlaybackTarget::new(title, episode)));
            }
            ui.add_space(theme::space::S);
        }
        let more = format!("{}  More info", egui_phosphor::regular::INFO);
        if ui::tonal_button(ui, &more).clicked() {
            actions.push(Action::Goto(Page::Title(title.key.clone())));
        }
        ui.add_space(theme::space::S);
        ui::list_button(ui, &title.key, on_list, actions);
    });
    ui.add_space(theme::space::L);
    if let Some(overview) = found.as_ref().and_then(|found| found.overview.as_deref()) {
        let mut job = egui::text::LayoutJob::simple(
            overview.to_owned(),
            theme::Role::Body.font(),
            theme::text(),
            ui.available_width(),
        );
        job.wrap.max_rows = 3;
        ui.label(job);
        ui.add_space(theme::space::M);
    }
    let mut facts = vec![subtitle(title)];
    if let Some(found) = &found {
        if let Some(rating) = found.rating {
            facts.push(format!("{} {rating:.1}", egui_phosphor::regular::STAR));
        }
        facts.extend(found.genres.iter().take(3).cloned());
    }
    ui.label(ui::muted(facts.join("  ·  ")));
    ui.add_space(theme::space::XXS);
    if ui::logo_heading(ui, art, title, 120.0).is_none() {
        // Two lines before it gives up: a long romanised name cut to one at
        // this size kept barely more than its first word.
        let mut job = egui::text::LayoutJob::simple(
            art.name(title).to_owned(),
            theme::Role::Hero.font(),
            theme::text(),
            ui.available_width().min(640.0),
        );
        job.wrap.max_rows = 2;
        job.wrap.overflow_character = Some('…');
        ui.label(job);
    }
}

/// "23 min left", for an episode part-way through. `None` when the file's
/// length is not known.
fn time_left(episode: &pstr_core::library::Episode) -> Option<String> {
    let watch = episode.watch?;
    let left = (watch.duration_secs? - watch.position_secs).max(0.0);
    let minutes = (left / 60.0).ceil() as u64;
    Some(if minutes >= 60 {
        format!("{}h {}m left", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min left")
    })
}

/// One dot per featured title at the banner's lower right, the one shown
/// drawn long. Returns the one clicked.
fn dots(
    ui: &mut egui::Ui,
    id: egui::Id,
    column: egui::Rect,
    titles: &[&Title],
    shown: usize,
) -> Option<usize> {
    const DOT: f32 = 8.0;
    const LONG: f32 = 24.0;
    const GAP: f32 = 8.0;
    let mut picked = None;
    // Clear of the scroll bar, which floats over the column's right edge.
    let mut right = column.right() - theme::space::L;
    let centre = column.bottom() - 58.0;
    // Right to left, so the row ends at the column's edge whatever its width.
    for (index, title) in titles.iter().enumerate().rev() {
        let width = ui.ctx().animate_value_with_time(
            id.with(("dot", index)),
            if index == shown { LONG } else { DOT },
            theme::motion::STATE,
        );
        let dot = egui::Rect::from_min_max(
            egui::pos2(right - width, centre - DOT / 2.0),
            egui::pos2(right, centre + DOT / 2.0),
        );
        right -= width + GAP;
        let response = ui
            .interact(
                dot.expand2(egui::vec2(GAP / 2.0, 8.0)),
                id.with(("dot-hit", index)),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::RadioButton,
                true,
                index == shown,
                &title.name,
            )
        });
        let ink = if index == shown {
            theme::text()
        } else if response.hovered() {
            theme::text().gamma_multiply(0.7)
        } else {
            theme::text().gamma_multiply(0.35)
        };
        ui.painter()
            .rect_filled(dot, egui::CornerRadius::same(4), ink);
        if response.clicked() {
            picked = Some(index);
        }
    }
    picked
}

/// The filter pills, the order and the grouping, at the right of the grid's
/// heading.
fn shelf_controls(ui: &mut egui::Ui, view: &LibraryView, actions: &mut Vec<Action>) {
    // Right to left: the order first, so it ends up last.
    let sort = egui::ComboBox::from_id_salt("library-sort")
        .selected_text(view.sort.label())
        .width(150.0)
        .show_ui(ui, |ui| {
            let mut picked = None;
            for sort in Sort::ALL {
                if ui
                    .selectable_label(view.sort == sort, sort.label())
                    .clicked()
                {
                    picked = Some(sort);
                }
            }
            picked
        });
    if let Some(Some(sort)) = sort.inner {
        actions.push(Action::SetShelf(view.filter, sort));
    }
    ui.add_space(theme::space::S);
    let mut grouped = view.grouped;
    let toggle = ui.toggle_value(&mut grouped, egui_phosphor::regular::STACK);
    toggle.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            view.grouped,
            "Group franchises",
        )
    });
    if toggle
        .on_hover_text(if view.grouped {
            "One tile per franchise. Click to show every title"
        } else {
            "Every title. Click to fold each franchise into one tile"
        })
        .changed()
    {
        actions.push(Action::SetGrouped(grouped));
    }
    ui.add_space(theme::space::L);
    let filters = Filter::ALL.map(|filter| (filter, filter.label()));
    if let Some(filter) = ui::widgets::segmented(ui, view.filter, &filters) {
        actions.push(Action::SetShelf(filter, view.sort));
    }
}

/// One shelf: titles that share something, scrolling sideways, in the grid's
/// own shape.
fn shelf_row(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    label: &str,
    titles: &[&Title],
    view: &LibraryView,
    tiles: Tiles,
    actions: &mut Vec<Action>,
) {
    let (width, aspect) = tile_shape(tiles);
    let now = crate::engine::unix_now();
    ui::shelf(ui, ("shelf", label), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::CARD_GAP;
            for title in titles {
                let picture = match tiles {
                    Tiles::Posters => art.poster(title),
                    Tiles::Stills => art.of(title),
                };
                let response = ui::card(
                    ui,
                    Card {
                        art: picture,
                        name: art.name(title),
                        subtitle: subtitle(title),
                        progress: title.resume().and_then(|e| e.progress()).map(|v| v as f32),
                        badge: view
                            .badges
                            .get(&title.key)
                            .cloned()
                            .or_else(|| countdown(art, title, now)),
                        width,
                        aspect,
                    },
                );
                tile_menu(&response, title, false, view, actions);
                if response.clicked() {
                    actions.push(Action::Goto(Page::Title(title.key.clone())));
                }
            }
        });
    });
}

/// `Ep 8 · in 2 d` on a show whose next episode is still to air.
fn countdown(art: &Art<'_>, title: &Title, now: i64) -> Option<String> {
    let (episode, at) = art
        .metadata
        .get(&title.key)?
        .metadata
        .as_ref()?
        .details
        .as_ref()?
        .next_airing(now)?;
    Some(pstr_core::news::countdown(episode, at, now))
}

/// The minimum width and the picture's shape of a tile.
fn tile_shape(tiles: Tiles) -> (f32, f32) {
    match tiles {
        Tiles::Posters => (theme::POSTER_WIDTH, theme::POSTER_ASPECT),
        Tiles::Stills => (theme::CARD_WIDTH, theme::CARD_ASPECT),
    }
}

/// The top shelf: one card per title in progress, scrolling sideways.
fn continue_row(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    titles: &[&Title],
    view: &LibraryView,
    actions: &mut Vec<Action>,
) {
    ui::shelf(ui, "continue", |ui| {
        ui.horizontal(|ui| {
            for title in titles {
                let Some(episode) = title.continue_with() else {
                    continue;
                };
                let remaining = match episode.resume_at() {
                    Some(at) => episode
                        .watch
                        .and_then(|watch| watch.duration_secs)
                        .map(|duration| format!("{} left", ui::format_time(duration - at)))
                        .unwrap_or_else(|| episode.label()),
                    None => format!("Next: {}", episode.label()),
                };

                let picture = art.of(title);
                let response = ui::card(
                    ui,
                    Card {
                        art: picture,
                        name: art.name(title),
                        subtitle: remaining,
                        progress: episode.progress().map(|value| value as f32),
                        badge: episode.numbering(),
                        // A shelf that scrolls sideways has no right edge
                        // to reach, so nothing to flex to. Always a
                        // still: it is a frame from where they stopped.
                        width: theme::CARD_WIDTH,
                        aspect: theme::CARD_ASPECT,
                    },
                );
                tile_menu(&response, title, true, view, actions);

                if response.clicked() {
                    actions.push(Action::Goto(Page::Title(title.key.clone())));
                }
            }
        });
    });
}

/// Every title, wrapped to the window.
/// `titles` as a wall of tiles. `folding` says they are the library grid's
/// own, whose folded franchises `view` counts.
fn grid(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    titles: &[&Title],
    view: &LibraryView,
    tiles: Tiles,
    folding: bool,
    actions: &mut Vec<Action>,
) {
    let (minimum, aspect) = tile_shape(tiles);
    let grid = ui::columns_of(ui.available_width(), minimum);
    let row_height = ui::card_height(grid.width, aspect);
    for (row, chunk) in titles.chunks(grid.columns).enumerate() {
        // A row scrolled out of view is only its height. Laying it out anyway
        // costs a subtitle and an artwork lookup per card per frame, and while
        // a film plays under the transport bar a frame is every frame.
        let slot = egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(ui.available_width(), row_height),
        );
        if !ui.is_rect_visible(slot) {
            // The spacing `horizontal` would have added after itself, too, or
            // the page grows and shrinks under the scroll bar as rows come and
            // go.
            ui.add_space(row_height + ui.spacing().item_spacing.y + theme::CARD_GAP);
            continue;
        }
        ui.horizontal(|ui| {
            // The gap the width was divided around, so what is drawn matches
            // what was measured. egui's default item spacing is narrower, and
            // the difference times the column count is a visible drift towards
            // the left edge.
            ui.spacing_mut().item_spacing.x = theme::CARD_GAP;
            for (column, title) in chunk.iter().enumerate() {
                let picture = match tiles {
                    Tiles::Posters => art.poster(title),
                    Tiles::Stills => art.of(title),
                };
                let folded = view
                    .matches
                    .get(row * grid.columns + column)
                    .filter(|_| folding)
                    .and_then(|index| view.folded.get(index))
                    .copied();
                let response = ui::card(
                    ui,
                    Card {
                        art: picture,
                        name: art.name(title),
                        subtitle: match folded {
                            Some(more) => format!("{}  ·  +{more} more", subtitle(title)),
                            None => subtitle(title),
                        },
                        progress: title.resume().and_then(|e| e.progress()).map(|v| v as f32),
                        // What a film is says itself in the subtitle below the
                        // card; a second `Film` over the poster is noise. A
                        // folded franchise says how much is under it, unless
                        // something new arrived in it.
                        badge: view
                            .badges
                            .get(&title.key)
                            .cloned()
                            .or_else(|| folded.map(|more| format!("+{more}"))),
                        width: grid.width,
                        aspect,
                    },
                );
                tile_menu(&response, title, false, view, actions);

                if response.clicked() {
                    actions.push(Action::Goto(Page::Title(title.key.clone())));
                }
            }
        });
        ui.add_space(theme::CARD_GAP);
    }
}

/// What a right-click on a tile offers: the things otherwise one page away.
///
/// `continuing` is the Continue watching shelf, where the tile can also be
/// taken off the shelf.
fn tile_menu(
    response: &egui::Response,
    title: &Title,
    continuing: bool,
    view: &LibraryView,
    actions: &mut Vec<Action>,
) {
    response.context_menu(|ui| {
        if let Some(episode) = title.next_up() {
            let label = match (title.resume(), episode.numbering()) {
                (Some(_), Some(numbering)) => format!("Resume {numbering}"),
                (Some(_), None) => "Resume".to_owned(),
                (None, _) => "Play".to_owned(),
            };
            if ui.button(label).clicked() {
                actions.push(Action::Play(PlaybackTarget::new(title, episode)));
            }
        }
        if ui.button("Open").clicked() {
            actions.push(Action::Goto(Page::Title(title.key.clone())));
        }
        let on_list = view.listed.contains(&title.key);
        if ui
            .button(if on_list {
                "Remove from My List"
            } else {
                "Add to My List"
            })
            .clicked()
        {
            actions.push(Action::SetOnList {
                key: title.key.clone(),
                on: !on_list,
            });
        }
        ui.separator();

        let all_watched = title.watched_count() == title.episode_count();
        if ui
            .button(if all_watched {
                "Mark unwatched"
            } else {
                "Mark watched"
            })
            .clicked()
        {
            actions.push(Action::SetTitleWatched {
                key: title.key.clone(),
                watched: !all_watched,
            });
        }
        if continuing
            && let Some(episode) = title.continue_with()
            && ui
                .button("Remove from Continue watching")
                .on_hover_text("Forget where you stopped in it")
                .clicked()
        {
            actions.push(Action::ForgetPosition {
                share_id: episode.node.share_id.clone(),
                link_id: episode.node.link_id.clone(),
            });
        }

        let count = title.episode_count();
        let download = if count == 1 {
            "Download".to_owned()
        } else {
            format!("Download all {count}")
        };
        if ui.button(download).clicked() {
            actions.push(Action::MakeOffline(
                title
                    .episodes()
                    .map(|episode| PlaybackTarget::new(title, episode))
                    .collect(),
            ));
        }
        ui.separator();
        if ui.button("Change match…").clicked() {
            actions.push(Action::OpenMatcher(title.key.clone()));
        }
    });
}

/// The grey line under a card: what it is, how much there is, and how much is
/// left.
///
/// A film says so, and says its year after it. The year alone was ambiguous in
/// the one place it mattered: `Ghost in the Shell` over `1995` reads as a
/// season, an episode count, anything — where every series card on the same
/// shelf is counting episodes.
fn subtitle(title: &Title) -> String {
    if title.kind == TitleKind::Film {
        return match title.year {
            Some(year) => format!("Film  ·  {year}"),
            None => "Film".into(),
        };
    }
    let total = title.episode_count();
    let watched = title.watched_count();
    if watched == 0 {
        plural(total, "episode")
    } else if watched >= total {
        format!("{} · watched", plural(total, "episode"))
    } else {
        format!("{watched} of {total} watched")
    }
}

/// The page before the catalog has been read: the shape of a grid, with
/// nothing in it yet.
///
/// Not the empty state. The catalog is on disk and is read in a fraction of a
/// second, and telling every launch "Nothing in the library yet" for that
/// fraction is a flash of something false.
fn placeholder(ui: &mut egui::Ui) {
    let grid = ui::columns(ui.available_width());
    let height = ui::card_height(grid.width, theme::CARD_ASPECT);
    ui::section(ui, "Library");
    for _ in 0..2 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::CARD_GAP;
            for _ in 0..grid.columns {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(grid.width, height), egui::Sense::hover());
                let image = egui::Rect::from_min_size(
                    rect.min,
                    egui::vec2(grid.width, (grid.width * theme::CARD_ASPECT).round()),
                );
                ui.painter().rect_filled(
                    image,
                    egui::CornerRadius::same(theme::radius::MD),
                    theme::card(),
                );
                theme::shimmer(ui.painter(), image);
            }
        });
        ui.add_space(theme::CARD_GAP);
    }
}

/// Posters and synopses, offered once there is a library to put them on and
/// never turned on unasked: switching them on sends title names out.
fn metadata_offer(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(theme::radius::MD))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Add posters and synopses?").strong());
                    ui.label(ui::muted(
                        "Turning it on in Settings sends the titles in your library — not your \
                         files, and nothing about what you have watched — to AniList or TMDB.",
                    ));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Not now").clicked() {
                        actions.push(Action::DismissMetadataOffer);
                    }
                    if ui::accent_button(ui, "Open settings").clicked() {
                        actions.push(Action::Goto(Page::Settings));
                    }
                });
            });
        });
}

fn empty_state(ui: &mut egui::Ui, crawl: Option<&str>, actions: &mut Vec<Action>) {
    ui.vertical_centered(|ui| {
        ui.add_space(120.0);
        // A share just added: say it is being looked through, rather than
        // offer to add the one that is already filling the page.
        if let Some(crawl) = crawl {
            ui.add(egui::Spinner::new().size(28.0));
            ui.add_space(theme::space::L);
            ui.label(
                theme::Role::Title
                    .rich("Looking through the share")
                    .strong(),
            );
            ui.add_space(theme::space::S);
            ui.label(ui::muted(format!(
                "Its titles appear here once it has been listed ({crawl})."
            )));
            return;
        }
        ui.label(
            theme::Role::Title
                .rich("Nothing in the library yet")
                .strong(),
        );
        ui.add_space(theme::space::S);
        ui.label(ui::muted(
            "Add a Proton Drive share link, and everything playable behind it shows up here.",
        ));
        ui.add_space(theme::space::XL);
        if ui::accent_button(ui, "Add a share").clicked() {
            actions.push(Action::OpenShareForm);
        }
    });
}

pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(kind: TitleKind, year: Option<u32>) -> Title {
        Title {
            key: "k".into(),
            name: "Ghost in the Shell".into(),
            year,
            kind,
            seasons: Vec::new(),
            share_ids: Vec::new(),
        }
    }

    /// A film says it is one. The year on its own was read as anything but —
    /// the cards beside it are all counting episodes.
    #[test]
    fn a_film_is_labelled_a_film_and_dated_after_it() {
        assert_eq!(
            subtitle(&title(TitleKind::Film, Some(1995))),
            "Film  ·  1995"
        );
        assert_eq!(subtitle(&title(TitleKind::Film, None)), "Film");
        assert_eq!(
            subtitle(&title(TitleKind::Series, Some(1995))),
            "0 episodes"
        );
    }

    #[test]
    fn the_banner_features_the_title_watched_last_and_nothing_without_a_picture() {
        let mut second = title(TitleKind::Film, None);
        second.key = "second".into();
        let library = Library {
            titles: vec![title(TitleKind::Film, None), second],
        };
        let metadata = std::collections::HashMap::new();
        let mut view = LibraryView::default();
        assert!(featured(&library, &view, &metadata, 0).is_empty());
        view.resumable = vec![1];
        let keys: Vec<&str> = featured(&library, &view, &metadata, 0)
            .iter()
            .map(|title| title.key.as_str())
            .collect();
        assert_eq!(keys, ["second"]);
    }

    fn pictured(key: &str) -> pstr_core::metadata::MetadataRecord {
        use pstr_core::metadata::{MetadataRecord, ProviderId, TitleMetadata};
        MetadataRecord {
            title_key: key.into(),
            provider: ProviderId::AniList,
            metadata: Some(TitleMetadata {
                provider: ProviderId::AniList,
                remote_id: key.into(),
                name: key.into(),
                original_name: None,
                overview: None,
                year: None,
                kind: TitleKind::Film,
                poster_url: None,
                backdrop_url: Some(format!("https://example.com/{key}.jpg")),
                logo_url: None,
                rating: None,
                genres: Vec::new(),
                episodes: None,
                url: None,
                details: None,
            }),
            fetched_at: 0,
            manual: false,
        }
    }

    #[test]
    fn the_banner_turns_through_five_pictured_titles_after_the_one_being_watched() {
        let titles: Vec<Title> = (0..8)
            .map(|index| {
                let mut one = title(TitleKind::Film, None);
                one.key = format!("t{index}");
                one
            })
            .collect();
        let library = Library { titles };
        let metadata: std::collections::HashMap<_, _> = (0..8)
            .map(|index| (format!("t{index}"), pictured(&format!("t{index}"))))
            .collect();
        let mut view = LibraryView::default();
        view.resumable = vec![5];
        let keys = |day| -> Vec<String> {
            featured(&library, &view, &metadata, day)
                .iter()
                .map(|title| title.key.clone())
                .collect()
        };
        assert_eq!(keys(2), ["t5", "t2", "t3", "t4", "t6"]);
        // The same order all day, and a different start the next.
        assert_eq!(keys(2), keys(2));
        assert_eq!(keys(3)[1], "t3");
    }

    #[test]
    fn plurals_read_naturally() {
        assert_eq!(plural(1, "title"), "1 title");
        assert_eq!(plural(0, "title"), "0 titles");
        assert_eq!(plural(12, "episode"), "12 episodes");
    }
}
